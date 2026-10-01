//! The authoritative session: ordered validation, durable commit, recovery.
//!
//! [`Session`] is transport-agnostic: it consumes decoded requests and
//! emits ordered outcomes. The composition owner moves those bytes across
//! the `canary-net` transport trait and drains
//! [`Session::drain_broadcasts`] in order.
//!
//! Validation runs in a fixed stage order, and every stage that rejects
//! assigns nothing, advances nothing, and records nothing:
//!
//! 1. bounded decode ([`wire::decode_request`], at the `*_bytes` entries);
//! 2. protocol and schema compatibility;
//! 3. credential authentication, then idempotency lookup;
//! 4. role permission (`Forbidden` denies; the connection stays alive
//!    and no trace is recorded);
//! 5. target existence (`UnknownTarget`) and target-scoped
//!    compare-and-set (`RevisionConflict` carries the canonical current
//!    revision and target state);
//! 6. numeric payload validation (finite floats, normalized quaternion;
//!    zero/negative scale is content);
//! 7. prefab veto via the `canary-state` gate;
//! 8. migration/codec rules over the `.14` vocabulary;
//! 9. assign-into-candidate then durable commit: the sequence/revision
//!    assignment (`history.accept`) lands in the candidate first, and the
//!    mutated state plus history record persist as one atomic write
//!    before anything is acknowledged or broadcast (`StorageFailed`
//!    leaves the previous good revision recoverable);
//! 10. swap/record: the committed candidate swaps in and the idempotency
//!     entry is recorded;
//! 11. accept plus ordered broadcast.
//!
//! Idempotency records live exactly as long as the retained-history
//! window: trim evicts entries with their tail, and a replay past
//! eviction follows the resync-or-reject path — never a silent reapply.
//! Restart rebuilds the index from the durable retained tail, so a retry
//! after a kill between commit and ack returns the prior accept without
//! double-applying, and the next fresh operation never reuses a sequence.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use canary_state::{
    transform_schema, AcceptedHistoryRecord, AuthoredDocument, LogicalEntityId, OperationSequence,
    PendingAccept, SnapshotValue, StateError, TailGap, TRANSFORM_SCHEMA_KEY,
    TRANSFORM_SCHEMA_VERSION,
};

use crate::actor::{ActorId, ClientOpId, Role};
use crate::error::{CollabError, RejectCode};
use crate::op::TransformPayload;
use crate::permissions::PermissionStore;
use crate::wire::{
    decode_request, decode_sync_request, encode_response, encode_sync_response, split_tagged_body,
    tag_body, AcceptEnvelope, EditRequest, RejectEnvelope, SyncRequest, SyncResponse, WireResponse,
    COLLAB_PROTOCOL_VERSION, MAX_SNAPSHOT_BYTES, TAG_EDIT, TAG_SYNC,
};

/// Retained accepted-operation bound: the session keeps at most this many
/// records before trimming with a checkpoint envelope.
pub const MAX_RETAINED_OPERATIONS: usize = 128;

/// Bound on queued outbound broadcast envelopes. The proof drains eagerly,
/// so this never fills in practice; on overflow the oldest entries drop
/// (counted in [`Session::dropped_broadcasts`]) and recovery runs through
/// retained history, never through a gap.
pub const MAX_OUTBOX_MESSAGES: usize = 1024;

/// Durable project storage behind the commit.
///
/// The session mutates a candidate document and persists it through this
/// seam before swapping it in: a failed save leaves the in-memory session
/// untouched. File storage uses the authored atomic save; tests inject
/// memory or failing stores.
pub trait DurableProjectStore {
    /// Loads the current durable document.
    fn load(&self) -> Result<AuthoredDocument, StateError>;
    /// Atomically persists a candidate document.
    fn save(&self, document: &AuthoredDocument) -> Result<(), StateError>;
}

/// File-backed project storage using the authored atomic save.
pub struct FileProjectStore {
    /// Project file path.
    path: PathBuf,
}

impl FileProjectStore {
    /// Stores the project file at `path`.
    #[must_use]
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    /// The project file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl DurableProjectStore for FileProjectStore {
    fn load(&self) -> Result<AuthoredDocument, StateError> {
        AuthoredDocument::load(&self.path)
    }

    fn save(&self, document: &AuthoredDocument) -> Result<(), StateError> {
        document.save(&self.path)
    }
}

/// One accepted operation: the canonical envelope broadcast in order.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptedOperation {
    /// The canonical accepted result.
    pub envelope: AcceptEnvelope,
}

/// One rejected request: stable code, current revision, optional refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    /// The wire contract for this rejection.
    pub code: RejectCode,
    /// Current target revision (`0` when the target is unknown).
    pub target_revision: u64,
    /// Canonical current target state on conflicts, when it parses.
    pub refresh: Option<TransformPayload>,
    /// Diagnostic detail. Never a wire contract.
    pub message: String,
}

impl Rejection {
    fn envelope(&self) -> RejectEnvelope {
        RejectEnvelope {
            code: self.code,
            target_revision: self.target_revision,
            refresh: self.refresh,
            message: self.message.clone(),
        }
    }
}

/// The outcome of one submitted edit request.
#[derive(Debug, Clone, PartialEq)]
pub enum SubmitOutcome {
    /// Durably accepted (and queued for broadcast).
    Accepted(AcceptedOperation),
    /// Rejected: nothing assigned, advanced, or recorded.
    Rejected(Rejection),
}

/// A sync call result: tail or snapshot-plus-checkpoint.
pub type SyncResult = Result<SyncResponse, CollabError>;

/// Request fields compared for idempotent replay.
#[derive(Debug, Clone, PartialEq)]
struct RequestFingerprint {
    target: String,
    expected_revision: u64,
    schema: String,
    schema_version: u32,
    payload: TransformPayload,
}

/// One idempotency entry: the fingerprint plus its accepted result.
#[derive(Debug, Clone)]
struct IdempotencyEntry {
    fingerprint: RequestFingerprint,
    envelope: AcceptEnvelope,
}

/// The authoritative collaboration session.
pub struct Session {
    document: AuthoredDocument,
    permissions: PermissionStore,
    authenticator: BTreeMap<String, (ActorId, Role)>,
    idempotency: HashMap<(u64, String), IdempotencyEntry>,
    outbox: VecDeque<AcceptEnvelope>,
    dropped_broadcasts: u64,
}

impl Session {
    /// Opens a session over `document` (already loaded through the durable
    /// seam, so restart resumes its sequences). The idempotency index is
    /// rebuilt from the durable retained tail.
    #[must_use]
    pub fn open(
        document: AuthoredDocument,
        permissions: PermissionStore,
        authenticator: BTreeMap<String, (ActorId, Role)>,
    ) -> Self {
        let mut session = Self {
            document,
            permissions,
            authenticator,
            idempotency: HashMap::new(),
            outbox: VecDeque::new(),
            dropped_broadcasts: 0,
        };
        session.rebuild_idempotency();
        session
    }

    /// The current authored document (durable state plus history).
    #[must_use]
    pub fn document(&self) -> &AuthoredDocument {
        &self.document
    }

    /// The fencing epoch of this server generation.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.permissions.epoch.0
    }

    /// Queued broadcast envelopes not yet drained.
    #[must_use]
    pub fn outbox_len(&self) -> usize {
        self.outbox.len()
    }

    /// Broadcast envelopes dropped to overflow, lifetime of the session.
    #[must_use]
    pub fn dropped_broadcasts(&self) -> u64 {
        self.dropped_broadcasts
    }

    /// Decodes wire bytes (stage 1) and submits the request (stages
    /// 2–11), returning the encoded reply.
    pub fn submit_bytes(
        &mut self,
        bytes: &[u8],
        durable: &impl DurableProjectStore,
    ) -> Result<Vec<u8>, CollabError> {
        let request = decode_request(bytes)?;
        let outcome = self.submit(&request, durable);
        let response = match outcome {
            SubmitOutcome::Accepted(accepted) => WireResponse::Accepted(accepted.envelope.clone()),
            SubmitOutcome::Rejected(rejection) => WireResponse::Rejected(rejection.envelope()),
        };
        encode_response(&response)
    }

    /// Submits one decoded edit request through validation stages 2–11.
    pub fn submit(
        &mut self,
        request: &EditRequest,
        durable: &impl DurableProjectStore,
    ) -> SubmitOutcome {
        // Stage 2: protocol and schema compatibility.
        if request.protocol != COLLAB_PROTOCOL_VERSION {
            return Self::reject(
                RejectCode::Malformed,
                0,
                None,
                format!(
                    "protocol {} is not version {COLLAB_PROTOCOL_VERSION}",
                    request.protocol
                ),
            );
        }
        let op_id = match ClientOpId::parse(&request.client_op_id) {
            Ok(id) => id,
            Err(_) => {
                return Self::reject(
                    RejectCode::Malformed,
                    0,
                    None,
                    "client operation id is malformed".to_owned(),
                );
            }
        };
        if request.schema != TRANSFORM_SCHEMA_KEY {
            return Self::reject(
                RejectCode::UnknownSchema,
                0,
                None,
                format!("unknown schema '{}'", request.schema),
            );
        }
        if request.schema_version != TRANSFORM_SCHEMA_VERSION {
            // No older transform version was ever published, so no
            // migration chain is registered: anything but current is
            // unmigratable, never silently coerced.
            return Self::reject(
                RejectCode::UnmigratableSchema,
                0,
                None,
                format!(
                    "schema '{}' v{} has no migration path to v{TRANSFORM_SCHEMA_VERSION}",
                    request.schema, request.schema_version
                ),
            );
        }

        // Stage 3: credential authentication. The wire carries no actor or
        // role field, so there is nothing to forge — only to look up.
        let Some((actor, _)) = self.authenticator.get(&request.credential).copied() else {
            return Self::reject(
                RejectCode::Unauthenticated,
                0,
                None,
                "credential authenticates no provisioned actor".to_owned(),
            );
        };

        // Stage 3b: idempotency. An identical replay returns the prior
        // accept without touching state; a key reuse with a different
        // payload is rejected. Neither path assigns or advances anything.
        // Idempotency precedes permission on purpose: a replay is the same
        // edit returning, not a new edit, so it answers from the record
        // even if the actor's role changed since the accept.
        let fingerprint = RequestFingerprint {
            target: request.target.clone(),
            expected_revision: request.expected_revision,
            schema: request.schema.clone(),
            schema_version: request.schema_version,
            payload: request.payload,
        };
        let idempotency_key = (actor.0, op_id.as_str().to_owned());
        if let Some(entry) = self.idempotency.get(&idempotency_key) {
            if entry.fingerprint == fingerprint {
                return SubmitOutcome::Accepted(AcceptedOperation {
                    envelope: entry.envelope.clone(),
                });
            }
            return Self::reject(
                RejectCode::IdConflict,
                self.document
                    .history
                    .target_revision(&local_or_empty(&request.target))
                    .0,
                None,
                "client operation id is already recorded with a different payload".to_owned(),
            );
        }

        // Stage 4: role permission. Forbidden denies with the connection
        // untouched and no trace recorded — this function has mutated
        // nothing so far, and returns without mutating anything.
        let allowed = self
            .permissions
            .role_of(&actor)
            .is_some_and(|role| role.can_submit());
        if !allowed {
            return Self::reject(
                RejectCode::Forbidden,
                0,
                None,
                "role may not submit edits".to_owned(),
            );
        }

        // Stage 5: target existence and target-scoped compare-and-set.
        let target = match LogicalEntityId::from_section_name(&request.target) {
            Ok(id) => id,
            Err(_) => {
                return Self::reject(
                    RejectCode::UnknownTarget,
                    0,
                    None,
                    format!("target '{}' is not a valid entity section", request.target),
                );
            }
        };
        if !self.document.entity_section_exists(&target) {
            return Self::reject(
                RejectCode::UnknownTarget,
                0,
                None,
                format!("unknown target '{}'", request.target),
            );
        }
        let current = self.document.history.target_revision(target.local());
        if current.0 != request.expected_revision {
            return Self::reject(
                RejectCode::RevisionConflict,
                current.0,
                self.refresh(&target),
                format!(
                    "expected target revision {}, current is {}",
                    request.expected_revision, current.0
                ),
            );
        }

        // Stage 6: numeric payload validation.
        if request.payload.validate().is_err() {
            return Self::reject(
                RejectCode::InvalidPayload,
                current.0,
                None,
                "payload is not finite or its quaternion is denormalized".to_owned(),
            );
        }

        // Stage 7: prefab veto via the `canary-state` gate.
        match self.document.transform_override_allowed(&target) {
            Ok(true) => {}
            Ok(false) => {
                return Self::reject(
                    RejectCode::PrefabOverrideDenied,
                    current.0,
                    None,
                    format!(
                        "prefab instance '{}' disallows a remote transform override",
                        request.target
                    ),
                );
            }
            Err(error) => {
                return Self::reject(
                    RejectCode::PrefabOverrideDenied,
                    current.0,
                    None,
                    format!(
                        "prefab state for '{}' is unresolvable: {error}",
                        request.target
                    ),
                );
            }
        }

        // Stage 8: migration/codec rules over the `.14` vocabulary. The
        // payload already passed numeric validation; this runs it through
        // the canonical snapshot-value codec so the durable record is
        // provably canonical, not merely finite.
        let fields = request.payload.to_fields();
        let canonical = SnapshotValue::from_json(&fields);
        if canonical.validate().is_err() {
            return Self::reject(
                RejectCode::InvalidPayload,
                current.0,
                None,
                "payload has no canonical snapshot encoding".to_owned(),
            );
        }

        // Stage 9: durable commit. Mutate a candidate, persist state plus
        // history in one atomic write, and only then swap it in. A storage
        // failure rejects with the previous good revision intact, nothing
        // broadcast, and no idempotency entry recorded.
        //
        // Clone-ceiling revisit trigger: this full-document clone costs
        // O(project bytes) per accepted op, fine for the small `.16`
        // proof projects. Measured 2026-10-01 (release build, i7-6700):
        // ~125us Noop / ~379us File-backed per-op accept at 100 entities,
        // linear scaling, File-backed crossing 1 ms near 450-500 entities.
        // Revisit — structural sharing or an incremental candidate instead
        // of a whole-document clone — when p99 accept latency on a 1 MB
        // project exceeds 2 ms. Measure on real hardware first; do not
        // "optimize" this on speculation.
        let mut candidate = self.document.clone();
        if candidate
            .set_entity_transform(&target, fields.clone())
            .is_err()
        {
            return Self::reject(
                RejectCode::StorageFailed,
                current.0,
                None,
                "entity section became unwritable".to_owned(),
            );
        }
        let (schema_id, schema_version) = transform_schema();
        let record = candidate.history.accept(PendingAccept {
            actor: actor.0,
            client_op_id: op_id.as_str().to_owned(),
            target: target.clone(),
            expected_revision: current,
            schema: schema_id,
            schema_version,
            payload: fields,
        });
        let checkpoint = candidate.history.trim_retained(MAX_RETAINED_OPERATIONS);
        if let Err(error) = durable.save(&candidate) {
            return Self::reject(
                RejectCode::StorageFailed,
                current.0,
                None,
                format!("durable commit failed: {error}"),
            );
        }

        // Stage 10: swap in, evict idempotency entries with their tail,
        // and record the new entry.
        self.document = candidate;
        if let Some(checkpoint) = checkpoint {
            self.evict_idempotency_through(checkpoint.last_retained);
        }
        let envelope = AcceptEnvelope {
            epoch: self.permissions.epoch.0,
            actor: actor.0,
            client_op_id: op_id.as_str().to_owned(),
            target: request.target.clone(),
            sequence: record.sequence.0,
            project_revision: record.project_revision.0,
            target_revision: record.target_revision.0,
            schema: request.schema.clone(),
            schema_version: request.schema_version,
            payload: request.payload,
        };
        self.idempotency.insert(
            idempotency_key,
            IdempotencyEntry {
                fingerprint,
                envelope: envelope.clone(),
            },
        );

        // Stage 11: accept plus ordered broadcast. The outbox is FIFO and
        // this session sequences single-threaded, so drain order is
        // sequence order.
        while self.outbox.len() >= MAX_OUTBOX_MESSAGES {
            self.outbox.pop_front();
            self.dropped_broadcasts = self.dropped_broadcasts.saturating_add(1);
        }
        self.outbox.push_back(envelope.clone());
        SubmitOutcome::Accepted(AcceptedOperation { envelope })
    }

    /// Decodes a sync request and resolves the cursor: the contiguous
    /// retained tail when covered, or a canonical snapshot plus the
    /// checkpoint when behind the horizon. Never a partial tail.
    pub fn sync_bytes(&self, bytes: &[u8]) -> SyncResult {
        let request = decode_sync_request(bytes)?;
        self.sync(&request)
    }

    /// Resolves one decoded sync cursor.
    ///
    /// The snapshot path is bounded by [`MAX_SNAPSHOT_BYTES`]: a project
    /// that outgrew the one-shot snapshot design fails typed
    /// ([`CollabError::TooLarge`]), never truncated. That content gate is
    /// necessary but not sufficient: servability is decided on the
    /// fully-encoded reply (encode-then-gate in
    /// [`encode_sync_response`](crate::wire::encode_sync_response),
    /// against [`MAX_RESPONSE_BYTES`](crate::wire::MAX_RESPONSE_BYTES)),
    /// so a snapshot under the content ceiling whose framed reply encodes
    /// past the response bound still fails typed [`CollabError::TooLarge`]
    /// instead of being served truncated.
    pub fn sync(&self, request: &SyncRequest) -> SyncResult {
        if request.protocol != COLLAB_PROTOCOL_VERSION {
            return Err(CollabError::Decode(format!(
                "protocol {} is not version {COLLAB_PROTOCOL_VERSION}",
                request.protocol
            )));
        }
        let Some((_, _)) = self.authenticator.get(&request.credential) else {
            return Err(CollabError::Unauthenticated);
        };
        let last = OperationSequence(request.last_sequence);
        match self.document.history.tail_since(last) {
            Ok(tail) => {
                let mut envelopes = Vec::with_capacity(tail.len());
                for record in tail {
                    if let Some(envelope) = self.envelope_for(record) {
                        envelopes.push(envelope);
                    }
                }
                Ok(SyncResponse {
                    epoch: self.permissions.epoch.0,
                    tail: envelopes,
                    checkpoint: self.document.history.checkpoint.clone(),
                    snapshot: None,
                })
            }
            Err(TailGap::BehindCheckpoint) => {
                let snapshot = self
                    .document
                    .to_canonical_json()
                    .map_err(|error| CollabError::SnapshotEncode(error.to_string()))?;
                if snapshot.len() > MAX_SNAPSHOT_BYTES {
                    return Err(CollabError::TooLarge {
                        claimed: snapshot.len(),
                        max: MAX_SNAPSHOT_BYTES,
                    });
                }
                Ok(SyncResponse {
                    epoch: self.permissions.epoch.0,
                    tail: Vec::new(),
                    checkpoint: self.document.history.checkpoint.clone(),
                    snapshot: Some(snapshot.into_bytes()),
                })
            }
            Err(TailGap::AheadOfHistory) => Err(CollabError::InvalidCursor),
        }
    }

    /// Handles one tagged inbound frame body and returns the tagged reply
    /// body: the dispatch arm of the [`wire`](crate::wire) frame-tag
    /// registry. Untaggable or undecodable input yields a generic
    /// `Malformed` reject — the caller's connection stays usable. A size
    /// bound failure ([`CollabError::TooLarge`], from an oversize request
    /// or an unservable oversize reply) keeps its typed code instead:
    /// the reply is a [`RejectCode::TooLarge`](crate::RejectCode::TooLarge)
    /// reject under the echoed tag, never a generic `Malformed`.
    #[must_use]
    pub fn handle_frame_body(
        &mut self,
        body: &[u8],
        durable: &impl DurableProjectStore,
    ) -> Vec<u8> {
        let (tag, payload) = split_tagged_body(body);
        let reply = match tag {
            TAG_EDIT => self.submit_bytes(payload, durable),
            TAG_SYNC => self
                .sync_bytes(payload)
                .and_then(|response| encode_sync_response(&response)),
            _ => Err(CollabError::Decode(format!(
                "unknown collab frame tag {tag:#04x}"
            ))),
        };
        match reply {
            Ok(bytes) => tag_body(tag, &bytes),
            Err(CollabError::TooLarge { claimed, max }) => Self::typed_reject(
                tag,
                RejectCode::TooLarge,
                &format!("collab frame reply claims {claimed} bytes, bound is {max}"),
            ),
            Err(error) => Self::generic_reject(tag, &error.to_string()),
        }
    }

    /// Drains queued broadcast envelopes in sequence order.
    pub fn drain_broadcasts(&mut self) -> Vec<AcceptEnvelope> {
        self.outbox.drain(..).collect()
    }

    /// Builds one tagged typed reject: the [`RejectCode`] is the wire
    /// contract, the detail is diagnostic only. Echoes the inbound tag so
    /// the peer can match the reply to its request kind.
    fn typed_reject(tag: u8, code: RejectCode, detail: &str) -> Vec<u8> {
        let response = WireResponse::Rejected(RejectEnvelope {
            code,
            target_revision: 0,
            refresh: None,
            message: detail.to_owned(),
        });
        let mut tagged = vec![tag];
        if let Ok(bytes) = encode_response(&response) {
            tagged.extend_from_slice(&bytes);
        }
        // Rejects are fixed small content; encoding one cannot realistically
        // fail, and there is no channel left to report that failure on if
        // it did.
        tagged
    }

    /// Builds the generic `Malformed` reject used when the request itself
    /// could not be understood. Echoes the inbound tag so the peer can match
    /// the reply to its request kind.
    fn generic_reject(tag: u8, detail: &str) -> Vec<u8> {
        Self::typed_reject(tag, RejectCode::Malformed, detail)
    }

    fn reject(
        code: RejectCode,
        target_revision: u64,
        refresh: Option<TransformPayload>,
        message: String,
    ) -> SubmitOutcome {
        SubmitOutcome::Rejected(Rejection {
            code,
            target_revision,
            refresh,
            message,
        })
    }

    fn refresh(&self, target: &LogicalEntityId) -> Option<TransformPayload> {
        let fields = self.document.entity_transform(target)?;
        TransformPayload::from_fields(&fields).ok()
    }

    fn envelope_for(&self, record: &AcceptedHistoryRecord) -> Option<AcceptEnvelope> {
        let payload = TransformPayload::from_fields(&record.payload).ok()?;
        Some(AcceptEnvelope {
            epoch: self.permissions.epoch.0,
            actor: record.actor,
            client_op_id: record.client_op_id.clone(),
            target: record.target.section_name(),
            sequence: record.sequence.0,
            project_revision: record.project_revision.0,
            target_revision: record.target_revision.0,
            schema: record.schema.as_str().to_owned(),
            schema_version: record.schema_version.0,
            payload,
        })
    }

    fn rebuild_idempotency(&mut self) {
        self.idempotency.clear();
        for record in &self.document.history.accepted {
            let Some(payload) = TransformPayload::from_fields(&record.payload).ok() else {
                // Only `.16` transform payloads are ever recorded, so a
                // non-transform record here means foreign history the
                // session cannot replay: skip the entry and let the owner
                // resync or reject it, rather than breaking restart for
                // everything else.
                continue;
            };
            self.idempotency.insert(
                (record.actor, record.client_op_id.clone()),
                IdempotencyEntry {
                    fingerprint: RequestFingerprint {
                        target: record.target.section_name(),
                        expected_revision: record.expected_revision.0,
                        schema: record.schema.as_str().to_owned(),
                        schema_version: record.schema_version.0,
                        payload,
                    },
                    envelope: AcceptEnvelope {
                        epoch: self.permissions.epoch.0,
                        actor: record.actor,
                        client_op_id: record.client_op_id.clone(),
                        target: record.target.section_name(),
                        sequence: record.sequence.0,
                        project_revision: record.project_revision.0,
                        target_revision: record.target_revision.0,
                        schema: record.schema.as_str().to_owned(),
                        schema_version: record.schema_version.0,
                        payload,
                    },
                },
            );
        }
    }

    fn evict_idempotency_through(&mut self, last_retained: OperationSequence) {
        self.idempotency
            .retain(|_, entry| entry.envelope.sequence > last_retained.0);
    }
}

fn local_or_empty(section: &str) -> String {
    LogicalEntityId::from_section_name(section)
        .map(|id| id.local().to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use crate::wire::{
        decode_response, encode_request, encode_sync_request, encode_sync_response,
        MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
    };
    use canary_state::ProjectId;

    use crate::actor::MAX_CLIENT_OP_ID_BYTES;
    use crate::op::TransformPayload as Payload;
    use crate::permissions::CredentialBinding;

    /// In-memory durable seam: succeeds, so the session's swap-in path is
    /// what the tests exercise. [`FailStore`] covers the failure path.
    struct MemStore {
        doc: RefCell<AuthoredDocument>,
    }

    impl DurableProjectStore for MemStore {
        fn load(&self) -> Result<AuthoredDocument, StateError> {
            Ok(self.doc.borrow().clone())
        }

        fn save(&self, document: &AuthoredDocument) -> Result<(), StateError> {
            *self.doc.borrow_mut() = document.clone();
            Ok(())
        }
    }

    /// Durable seam that refuses every write with a typed error.
    struct FailStore;

    impl DurableProjectStore for FailStore {
        fn load(&self) -> Result<AuthoredDocument, StateError> {
            Err(StateError::File {
                path: Path::new("<test>").to_path_buf(),
                reason: "injected failure".to_owned(),
            })
        }

        fn save(&self, _document: &AuthoredDocument) -> Result<(), StateError> {
            Err(StateError::File {
                path: Path::new("<test>").to_path_buf(),
                reason: "injected atomic-write failure".to_owned(),
            })
        }
    }

    fn credentials() -> BTreeMap<String, CredentialBinding> {
        BTreeMap::from([
            (
                "owner-secret".to_owned(),
                CredentialBinding {
                    actor: ActorId(1),
                    role: Role::Owner,
                },
            ),
            (
                "editor-secret".to_owned(),
                CredentialBinding {
                    actor: ActorId(2),
                    role: Role::Editor,
                },
            ),
            (
                "reader-secret".to_owned(),
                CredentialBinding {
                    actor: ActorId(3),
                    role: Role::Reader,
                },
            ),
        ])
    }

    fn hero_fields(x: f64) -> serde_json::Value {
        serde_json::json!({
            "translation": [x, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        })
    }

    fn payload_at(x: f32) -> Payload {
        Payload {
            translation: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }

    fn base_document() -> AuthoredDocument {
        let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
        doc.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({
                "canary.transform": hero_fields(0.0),
                "test.health": {"hp": 10},
            }),
        );
        doc.sections.insert(
            "entity.sidekick".to_owned(),
            serde_json::json!({"canary.transform": hero_fields(0.0)}),
        );
        doc
    }

    fn harness() -> (Session, MemStore) {
        let doc = base_document();
        let (permissions, authenticator) =
            PermissionStore::provision(&credentials()).expect("distinct test actors provision");
        let store = MemStore {
            doc: RefCell::new(doc.clone()),
        };
        (Session::open(doc, permissions, authenticator), store)
    }

    fn edit(credential: &str, op: &str, target: &str, expected: u64) -> EditRequest {
        EditRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: credential.to_owned(),
            client_op_id: op.to_owned(),
            target: target.to_owned(),
            expected_revision: expected,
            schema: TRANSFORM_SCHEMA_KEY.to_owned(),
            schema_version: TRANSFORM_SCHEMA_VERSION,
            payload: payload_at(1.0),
        }
    }

    fn accepted(outcome: SubmitOutcome) -> AcceptEnvelope {
        match outcome {
            SubmitOutcome::Accepted(accepted) => accepted.envelope,
            SubmitOutcome::Rejected(rejection) => {
                panic!(
                    "expected accept, got {:?}: {}",
                    rejection.code, rejection.message
                )
            }
        }
    }

    fn rejected(outcome: SubmitOutcome) -> Rejection {
        match outcome {
            SubmitOutcome::Rejected(rejection) => rejection,
            SubmitOutcome::Accepted(accepted) => {
                panic!(
                    "expected reject, got accept at seq {}",
                    accepted.envelope.sequence
                )
            }
        }
    }

    #[test]
    fn owner_accept_assigns_sequence_and_revisions() {
        let (mut session, store) = harness();
        let envelope =
            accepted(session.submit(&edit("owner-secret", "op-1", "entity.hero", 0), &store));
        assert_eq!(envelope.sequence, 1);
        assert_eq!(envelope.project_revision, 1);
        assert_eq!(envelope.target_revision, 1);
        assert_eq!(envelope.actor, 1);
        assert_eq!(envelope.epoch, 1);
        // Durable state and history committed together.
        assert_eq!(
            store.doc.borrow().sections["entity.hero"]["canary.transform"]["translation"],
            serde_json::json!([1.0, 0.0, 0.0])
        );
        assert_eq!(store.doc.borrow().history.accepted.len(), 1);
        // Broadcast queued exactly once.
        let broadcasts = session.drain_broadcasts();
        assert_eq!(broadcasts.len(), 1);
        assert_eq!(broadcasts[0].sequence, 1);
        assert!(session.drain_broadcasts().is_empty());
    }

    #[test]
    fn independent_target_edit_survives_a_project_revision_advance() {
        let (mut session, store) = harness();
        accepted(session.submit(&edit("owner-secret", "op-1", "entity.hero", 0), &store));
        // Project is now at revision 1, but sidekick is untouched: its
        // target revision is still 0, so this must not conflict.
        let envelope =
            accepted(session.submit(&edit("editor-secret", "op-2", "entity.sidekick", 0), &store));
        assert_eq!(envelope.sequence, 2);
        assert_eq!(envelope.project_revision, 2);
        assert_eq!(envelope.target_revision, 1);
    }

    #[test]
    fn reader_submit_is_forbidden_and_leaves_the_connection_usable() {
        let (mut session, store) = harness();
        let rejection =
            rejected(session.submit(&edit("reader-secret", "op-1", "entity.hero", 0), &store));
        assert_eq!(rejection.code, RejectCode::Forbidden);
        assert!(session.drain_broadcasts().is_empty());
        // The denial recorded nothing: the next sequence is still 1, and a
        // privileged submit on the same session succeeds.
        assert_eq!(session.document.history.next_sequence.0, 1);
        let envelope =
            accepted(session.submit(&edit("owner-secret", "op-2", "entity.hero", 0), &store));
        assert_eq!(envelope.sequence, 1);
    }

    #[test]
    fn same_target_race_yields_one_accept_and_one_conflict_with_refresh() {
        let (mut session, store) = harness();
        let first = edit("owner-secret", "op-1", "entity.hero", 0);
        let mut second = edit("editor-secret", "op-2", "entity.hero", 0);
        second.payload = payload_at(9.0);
        let won = accepted(session.submit(&first, &store));
        assert_eq!(won.sequence, 1);
        let lost = rejected(session.submit(&second, &store));
        assert_eq!(lost.code, RejectCode::RevisionConflict);
        assert_eq!(lost.target_revision, 1);
        assert_eq!(
            lost.refresh,
            Some(payload_at(1.0)),
            "conflict carries the canonical current target"
        );
        // Retry against the refreshed revision is accepted.
        let mut retry = edit("editor-secret", "op-3", "entity.hero", 1);
        retry.payload = payload_at(9.0);
        let envelope = accepted(session.submit(&retry, &store));
        assert_eq!(envelope.sequence, 2);
        assert_eq!(envelope.target_revision, 2);
    }

    #[test]
    fn identical_replay_is_idempotent_but_key_reuse_is_rejected() {
        let (mut session, store) = harness();
        let request = edit("owner-secret", "op-1", "entity.hero", 0);
        let first = accepted(session.submit(&request, &store));
        let replay = accepted(session.submit(&request, &store));
        assert_eq!(replay, first, "identical replay returns the prior result");
        assert_eq!(
            session.document.history.target_revision("hero").0,
            1,
            "no double apply"
        );
        assert_eq!(session.document.history.next_sequence.0, 2);
        assert!(
            session.drain_broadcasts().len() == 1,
            "replay queues no second broadcast"
        );

        let mut reuse = edit("owner-secret", "op-1", "entity.hero", 1);
        reuse.payload = payload_at(7.0);
        let conflict = rejected(session.submit(&reuse, &store));
        assert_eq!(conflict.code, RejectCode::IdConflict);
    }

    #[test]
    fn every_rejection_assigns_and_advances_nothing() {
        let (mut session, store) = harness();
        let mut bad_schema = edit("owner-secret", "r-1", "entity.hero", 0);
        bad_schema.schema = "canary.nope".to_owned();
        let cases: Vec<EditRequest> = vec![
            bad_schema,
            edit("forged-credential", "r-2", "entity.hero", 0),
            edit("reader-secret", "r-3", "entity.hero", 0),
            edit("owner-secret", "r-4", "entity.ghost", 0),
            edit("owner-secret", "r-5", "entity.hero", 99),
            {
                let mut unmigratable = edit("owner-secret", "r-6", "entity.hero", 0);
                unmigratable.schema_version = 0;
                unmigratable
            },
        ];
        for request in &cases {
            let rejection = rejected(session.submit(request, &store));
            assert!(
                !matches!(rejection.code, RejectCode::StorageFailed),
                "unexpected code {:?}",
                rejection.code
            );
        }
        assert_eq!(session.document.history.next_sequence.0, 1);
        assert_eq!(session.document.history.project_revision.0, 0);
        assert!(session.document.history.accepted.is_empty());
        assert!(session.drain_broadcasts().is_empty());
        assert_eq!(
            session.document.sections["entity.hero"]["canary.transform"]["translation"],
            serde_json::json!([0.0, 0.0, 0.0]),
            "rejected requests never mutate state"
        );
    }

    #[test]
    fn stage_gates_reject_with_stable_codes() {
        let (mut session, store) = harness();
        // Stage 1 is covered by the wire oversize test; here each case
        // pins its stage's code through `submit`.
        let mut protocol = edit("owner-secret", "s-1", "entity.hero", 0);
        protocol.protocol = 999;
        assert_eq!(
            rejected(session.submit(&protocol, &store)).code,
            RejectCode::Malformed
        );

        let mut op_id = edit("owner-secret", "", "entity.hero", 0);
        op_id.client_op_id = String::new();
        assert_eq!(
            rejected(session.submit(&op_id, &store)).code,
            RejectCode::Malformed
        );

        let mut schema = edit("owner-secret", "s-3", "entity.hero", 0);
        schema.schema = "canary.other".to_owned();
        assert_eq!(
            rejected(session.submit(&schema, &store)).code,
            RejectCode::UnknownSchema
        );

        let mut version = edit("owner-secret", "s-4", "entity.hero", 0);
        version.schema_version = TRANSFORM_SCHEMA_VERSION + 1;
        assert_eq!(
            rejected(session.submit(&version, &store)).code,
            RejectCode::UnmigratableSchema
        );

        assert_eq!(
            rejected(session.submit(&edit("nope", "s-5", "entity.hero", 0), &store)).code,
            RejectCode::Unauthenticated
        );
        assert_eq!(
            rejected(session.submit(&edit("reader-secret", "s-6", "entity.hero", 0), &store)).code,
            RejectCode::Forbidden
        );
        assert_eq!(
            rejected(session.submit(&edit("owner-secret", "s-7", "entity.ghost", 0), &store)).code,
            RejectCode::UnknownTarget
        );
        assert_eq!(
            rejected(session.submit(&edit("owner-secret", "s-8", "not-an-entity", 0), &store)).code,
            RejectCode::UnknownTarget
        );

        for (tag, payload) in [
            (
                "nan",
                Payload {
                    translation: [f32::NAN, 0.0, 0.0],
                    ..payload_at(0.0)
                },
            ),
            (
                "inf",
                Payload {
                    scale: [1.0, f32::INFINITY, 1.0],
                    ..payload_at(0.0)
                },
            ),
            (
                "flat-quat",
                Payload {
                    rotation: [0.0, 0.0, 0.0, 0.0],
                    ..payload_at(0.0)
                },
            ),
            (
                "long-quat",
                Payload {
                    rotation: [1.0, 1.0, 0.0, 0.0],
                    ..payload_at(0.0)
                },
            ),
        ] {
            let mut numeric = edit("owner-secret", &format!("s-9-{tag}"), "entity.hero", 0);
            numeric.payload = payload;
            assert_eq!(
                rejected(session.submit(&numeric, &store)).code,
                RejectCode::InvalidPayload,
                "{tag} must fail numeric validation"
            );
        }
        assert_eq!(session.document.history.next_sequence.0, 1);
    }

    #[test]
    fn prefab_instance_without_its_own_transform_is_vetoed() {
        let mut doc = base_document();
        doc.prefabs.insert(
            "goblin".to_owned(),
            canary_state::Prefab {
                base: None,
                overrides: std::collections::BTreeMap::from([(
                    TRANSFORM_SCHEMA_KEY.to_owned(),
                    hero_fields(0.0),
                )]),
            },
        );
        doc.sections.insert(
            "entity.grunt".to_owned(),
            serde_json::json!({"prefab": "goblin"}),
        );
        let (permissions, authenticator) =
            PermissionStore::provision(&credentials()).expect("distinct test actors provision");
        let store = MemStore {
            doc: RefCell::new(doc.clone()),
        };
        let mut session = Session::open(doc, permissions, authenticator);
        let rejection =
            rejected(session.submit(&edit("owner-secret", "op-1", "entity.grunt", 0), &store));
        assert_eq!(rejection.code, RejectCode::PrefabOverrideDenied);
        assert!(session.drain_broadcasts().is_empty());
    }

    #[test]
    fn injected_storage_failure_rejects_without_broadcast_or_record() {
        let (mut session, _mem) = harness();
        let failing = FailStore;
        let request = edit("owner-secret", "op-1", "entity.hero", 0);
        let rejection = rejected(session.submit(&request, &failing));
        assert_eq!(rejection.code, RejectCode::StorageFailed);
        assert!(session.drain_broadcasts().is_empty());
        assert!(session.document.history.accepted.is_empty());
        assert_eq!(session.document.history.next_sequence.0, 1);
        assert_eq!(
            session.document.sections["entity.hero"]["canary.transform"]["translation"],
            serde_json::json!([0.0, 0.0, 0.0]),
            "failed commit leaves in-memory state untouched"
        );

        // The operation was never recorded, so a healthy retry sequences
        // it as new — no gap, no ghost entry.
        let store = MemStore {
            doc: RefCell::new(session.document.clone()),
        };
        let envelope = accepted(session.submit(&request, &store));
        assert_eq!(envelope.sequence, 1);
    }

    #[test]
    fn submit_bytes_round_trips_through_the_wire() {
        let (mut session, store) = harness();
        let bytes =
            encode_request(&edit("owner-secret", "op-1", "entity.hero", 0)).expect("encode");
        assert!(bytes.len() < MAX_REQUEST_BYTES);
        let reply = session.submit_bytes(&bytes, &store).expect("submit");
        let response = decode_response(&reply).expect("decode reply");
        match response {
            WireResponse::Accepted(envelope) => assert_eq!(envelope.sequence, 1),
            WireResponse::Rejected(rejection) => {
                panic!("expected accept, got {:?}", rejection.code)
            }
        }
        assert!(session.submit_bytes(&[0xFF; 16], &store).is_err());
    }

    #[test]
    fn sync_serves_covered_tails_and_rejects_forged_cursors() {
        let (mut session, store) = harness();
        accepted(session.submit(&edit("owner-secret", "op-1", "entity.hero", 0), &store));
        accepted(session.submit(&edit("editor-secret", "op-2", "entity.sidekick", 0), &store));

        let ask = |last: u64| SyncRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: "reader-secret".to_owned(),
            last_sequence: last,
        };
        let full = session.sync(&ask(0)).expect("full tail");
        assert_eq!(full.tail.len(), 2);
        assert!(full.snapshot.is_none());
        assert!(full.checkpoint.is_none());

        let partial = session.sync(&ask(1)).expect("partial tail");
        assert_eq!(partial.tail.len(), 1);
        assert_eq!(partial.tail[0].sequence, 2);

        let tip = session.sync(&ask(2)).expect("at tip");
        assert!(tip.tail.is_empty());
        assert!(tip.snapshot.is_none());

        assert!(
            matches!(session.sync(&ask(9)), Err(CollabError::InvalidCursor)),
            "ahead-of-history cursors are rejected, never served empty"
        );
        let forged = SyncRequest {
            credential: "forged".to_owned(),
            ..ask(0)
        };
        assert!(
            matches!(session.sync(&forged), Err(CollabError::Unauthenticated)),
            "sync authenticates like submit"
        );
        let sync_bytes = encode_sync_request(&ask(1)).expect("encode sync");
        let reply = session.sync_bytes(&sync_bytes).expect("sync bytes");
        assert_eq!(reply.tail.len(), 1);
    }

    #[test]
    fn eviction_carries_a_checkpoint_and_replay_resyncs_or_rejects() {
        let (mut session, store) = harness();
        let total = MAX_RETAINED_OPERATIONS + 2;
        for index in 0..total {
            let mut request = edit(
                "owner-secret",
                &format!("op-{index}"),
                "entity.hero",
                index as u64,
            );
            request.payload = payload_at(index as f32);
            let envelope = accepted(session.submit(&request, &store));
            assert_eq!(envelope.sequence, index as u64 + 1);
        }
        let checkpoint = session
            .document
            .history
            .checkpoint
            .clone()
            .expect("bound enforced with a checkpoint");
        assert_eq!(
            checkpoint.last_retained.0,
            (total - MAX_RETAINED_OPERATIONS) as u64
        );
        assert_eq!(checkpoint.marker.len(), 64);
        assert_eq!(
            session.document.history.accepted.len(),
            MAX_RETAINED_OPERATIONS
        );

        // Behind the horizon: snapshot plus checkpoint, never a partial tail.
        let behind = session
            .sync(&SyncRequest {
                protocol: COLLAB_PROTOCOL_VERSION,
                credential: "reader-secret".to_owned(),
                last_sequence: 0,
            })
            .expect("behind-horizon sync");
        assert!(behind.tail.is_empty());
        assert_eq!(behind.checkpoint, Some(checkpoint.clone()));
        let snapshot = behind.snapshot.expect("snapshot path carries bytes");
        let reparsed = AuthoredDocument::from_canonical_json(
            &String::from_utf8(snapshot).expect("canonical JSON is UTF-8"),
        )
        .expect("snapshot parses");
        assert_eq!(reparsed.history.project_revision.0, total as u64);

        // Covered cursor still replays its contiguous tail.
        let covered = session
            .sync(&SyncRequest {
                protocol: COLLAB_PROTOCOL_VERSION,
                credential: "reader-secret".to_owned(),
                last_sequence: checkpoint.last_retained.0,
            })
            .expect("covered sync");
        assert!(covered.snapshot.is_none());
        assert_eq!(covered.tail.len(), MAX_RETAINED_OPERATIONS);

        // Post-eviction replay of an evicted op ID: the key is gone, so
        // this is a new request against a moved target — conflict, never
        // a silent reapply.
        let mut replay = edit("owner-secret", "op-0", "entity.hero", 0);
        replay.payload = payload_at(0.0);
        let rejection = rejected(session.submit(&replay, &store));
        assert_eq!(rejection.code, RejectCode::RevisionConflict);
        assert_eq!(rejection.target_revision, total as u64);
    }

    #[test]
    fn oversize_snapshot_behind_the_horizon_fails_typed_toolarge() {
        use crate::wire::MAX_SNAPSHOT_BYTES;

        // A project past the 1 MB snapshot ceiling: padding bulk that the
        // operation path never touches, so every accept still succeeds and
        // only the behind-horizon snapshot trips the ceiling.
        let (mut session, store) = harness();
        session.document.sections.insert(
            "entity.bulk".to_owned(),
            serde_json::json!({"blob": "x".repeat(MAX_SNAPSHOT_BYTES + 1)}),
        );
        let total = MAX_RETAINED_OPERATIONS + 2;
        for index in 0..total {
            let mut request = edit(
                "owner-secret",
                &format!("op-{index}"),
                "entity.hero",
                index as u64,
            );
            request.payload = payload_at(index as f32);
            accepted(session.submit(&request, &store));
        }

        let err = session
            .sync(&SyncRequest {
                protocol: COLLAB_PROTOCOL_VERSION,
                credential: "reader-secret".to_owned(),
                last_sequence: 0,
            })
            .expect_err("oversize snapshot must not be served");
        match err {
            CollabError::TooLarge { claimed, max } => {
                assert_eq!(max, MAX_SNAPSHOT_BYTES);
                assert!(
                    claimed > max,
                    "claimed {claimed} must exceed the ceiling {max}"
                );
            }
            other => panic!("oversize snapshot must fail TooLarge, got {other:?}"),
        }

        // A covered cursor still replays its tail: the ceiling binds only
        // the one-shot snapshot path, never incremental sync.
        let checkpoint = session
            .document
            .history
            .checkpoint
            .clone()
            .expect("bound enforced with a checkpoint");
        let covered = session
            .sync(&SyncRequest {
                protocol: COLLAB_PROTOCOL_VERSION,
                credential: "reader-secret".to_owned(),
                last_sequence: checkpoint.last_retained.0,
            })
            .expect("covered sync still serves");
        assert!(covered.snapshot.is_none());
        assert_eq!(covered.tail.len(), MAX_RETAINED_OPERATIONS);
    }

    #[test]
    fn kill_and_restart_between_commit_and_ack_loses_nothing_and_reuses_nothing() {
        let dir =
            std::env::temp_dir().join(format!("canary-collab-restart-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let project_path = dir.join("project.json");
        let permission_path = dir.join("permissions.json");
        base_document().save(&project_path).expect("genesis save");

        // Generation one: accept seq 1, then die before delivering the ack.
        let (permissions, authenticator) =
            PermissionStore::open_or_provision(&permission_path, &credentials())
                .expect("provision");
        assert_eq!(permissions.epoch.0, 1);
        let durable = FileProjectStore::new(&project_path);
        let mut first = Session::open(durable.load().expect("load"), permissions, authenticator);
        let request = edit("owner-secret", "op-1", "entity.hero", 0);
        let accepted_once = accepted(first.submit(&request, &durable));
        assert_eq!(accepted_once.sequence, 1);
        drop(first); // kill: the ack never reaches the client.

        // Generation two: new epoch, sequences resume from durable history.
        let (permissions, authenticator) =
            PermissionStore::open_or_provision(&permission_path, &credentials()).expect("reopen");
        assert_eq!(permissions.epoch.0, 2, "restart bumps the fencing epoch");
        let durable = FileProjectStore::new(&project_path);
        let mut second = Session::open(durable.load().expect("reload"), permissions, authenticator);

        // Client retry of the unacknowledged op: the rebuilt idempotency
        // index returns the prior accept — applied once, never twice.
        let retried = accepted(second.submit(&request, &durable));
        assert_eq!(retried.sequence, 1, "no sequence reuse on retry");
        assert_eq!(
            second.document.history.target_revision("hero").0,
            1,
            "no double apply"
        );
        // Fresh work sequences after the durable tip.
        let fresh = accepted(second.submit(
            &edit("editor-secret", "op-2", "entity.sidekick", 0),
            &durable,
        ));
        assert_eq!(fresh.sequence, 2, "no sequence gap or reuse");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn client_op_id_bound_is_visible_to_callers() {
        assert_eq!(MAX_CLIENT_OP_ID_BYTES, 128);
    }

    #[test]
    fn encoded_reply_past_the_response_bound_fails_typed_toolarge_on_every_path() {
        fn drive(pad: usize) -> (Session, MemStore) {
            let mut doc = base_document();
            doc.sections.insert(
                "entity.bulk".to_owned(),
                serde_json::json!({"blob": "x".repeat(pad)}),
            );
            let (permissions, authenticator) =
                PermissionStore::provision(&credentials()).expect("provision");
            let store = MemStore {
                doc: RefCell::new(doc.clone()),
            };
            let mut session = Session::open(doc, permissions, authenticator);
            let total = MAX_RETAINED_OPERATIONS + 2;
            for index in 0..total {
                let mut request = edit(
                    "owner-secret",
                    &format!("enc-{index}"),
                    "entity.hero",
                    index as u64,
                );
                request.payload = payload_at(index as f32);
                accepted(session.submit(&request, &store));
            }
            (session, store)
        }

        // `x` needs no JSON escaping, so one pad byte is one snapshot byte.
        // Calibrate 16 bytes under the content ceiling: the content gate
        // passes, but the reply framing (option tags, length prefixes, the
        // checkpoint envelope) pushes the encoded reply past the 1 MiB
        // response bound.
        let (probe, _) = drive(0);
        let probe_len = probe
            .document()
            .to_canonical_json()
            .expect("serialize probe")
            .len();
        assert!(
            probe_len < MAX_SNAPSHOT_BYTES,
            "probe {probe_len} must sit below the ceiling"
        );
        let (mut session, store) = drive(MAX_SNAPSHOT_BYTES - 16 - probe_len);
        let content_len = session
            .document()
            .to_canonical_json()
            .expect("serialize near-ceiling")
            .len();
        assert_eq!(content_len, MAX_SNAPSHOT_BYTES - 16);

        let behind = SyncRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: "reader-secret".to_owned(),
            last_sequence: 0,
        };
        // The content gate passes: `sync` still resolves the snapshot reply.
        let response = session.sync(&behind).expect("content gate passes");
        assert!(
            response
                .snapshot
                .as_ref()
                .expect("snapshot path carries bytes")
                .len()
                <= MAX_SNAPSHOT_BYTES
        );
        // Encode-then-gate: the ENCODED reply exceeds the response bound,
        // so serving it fails typed TooLarge — never truncated.
        match encode_sync_response(&response) {
            Err(CollabError::TooLarge { claimed, max }) => {
                assert_eq!(max, MAX_RESPONSE_BYTES);
                assert!(claimed > max, "claimed {claimed} must exceed {max}");
            }
            other => {
                panic!("encoded reply past the bound must fail TooLarge, got {other:?}")
            }
        }

        // Frame path: the same cursor answers typed TooLarge, never a
        // generic Malformed.
        let mut frame = vec![TAG_SYNC];
        frame.extend_from_slice(&encode_sync_request(&behind).expect("encode sync"));
        let reply = session.handle_frame_body(&frame, &store);
        assert_eq!(reply.first().copied().unwrap_or(0xFF), TAG_SYNC);
        match decode_response(&reply[1..]).expect("typed reject decodes") {
            WireResponse::Rejected(rejection) => {
                assert_eq!(
                    rejection.code,
                    RejectCode::TooLarge,
                    "oversize reply must stay typed, never Malformed"
                );
            }
            WireResponse::Accepted(_) => panic!("oversize snapshot reply was accepted"),
        }
    }
}
