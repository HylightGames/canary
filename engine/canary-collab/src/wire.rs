//! Bounded `postcard` wire codecs for the operation path.
//!
//! Every decode gates the byte length before touching the codec, and every
//! decoded string is re-bounded after parsing: a hostile prefix can never
//! force a large allocation, and a hostile body can never smuggle an
//! unbounded string into the session. `postcard` rejects trailing bytes,
//! so framing is exact.

use canary_state::CheckpointEnvelope;

use crate::actor::MAX_CLIENT_OP_ID_BYTES;
use crate::error::{CollabError, RejectCode};
use crate::op::TransformPayload;

/// Wire protocol version spoken by this build.
pub const COLLAB_PROTOCOL_VERSION: u32 = 1;

/// Bound on one encoded edit request or sync request, in bytes.
///
/// An identity-carrying transform edit is a few hundred bytes; 8 KiB
/// leaves headroom for future fields while keeping hostile prefixes
/// cheap to refuse.
pub const MAX_REQUEST_BYTES: usize = 8_192;

/// Bound on one encoded sync request, in bytes (credential plus cursor).
pub const MAX_SYNC_REQUEST_BYTES: usize = 512;

/// Bound on one encoded response, in bytes.
///
/// Covers checkpointed-snapshot sync replies for the small `.16` proof
/// projects; anything larger is a host-level concern, not a silent
/// truncation.
pub const MAX_RESPONSE_BYTES: usize = 1_048_576;

/// Ceiling on one canonical snapshot payload served on the sync path, in
/// bytes: the `.16`-and-`.1.0` contract.
///
/// A behind-horizon client receives the whole canonical project JSON in
/// one reply, so the snapshot itself is bounded before the reply is
/// built: past this ceiling [`Session::sync`] fails typed
/// ([`CollabError::TooLarge`]) instead of serving a reply the 1 MB
/// response bound would truncate. Small proof projects (kilobytes) sit
/// orders of magnitude below it; the ceiling only ever fires on a
/// project that outgrew the one-shot snapshot design.
///
/// Revisit trigger: the first real project whose canonical snapshot
/// exceeds 512 KiB reopens this ceiling (chunked snapshot transfer wins
/// over silent growth) — not an emergency bump, a design trigger.
pub const MAX_SNAPSHOT_BYTES: usize = 1_048_576;

/// Bound on a decoded credential string, in bytes.
pub const MAX_CREDENTIAL_BYTES: usize = 256;

/// Bound on a decoded target section name, in bytes.
pub const MAX_TARGET_BYTES: usize = 256;

/// Bound on a decoded schema name, in bytes.
pub const MAX_SCHEMA_BYTES: usize = 128;

/// Frame-tag registry: the first byte of every collab frame body selects
/// the request kind, and the reply echoes the same tag so the peer can
/// match it to its request.
///
/// Owner: `canary-collab::wire`, versioned alongside
/// [`COLLAB_PROTOCOL_VERSION`]. The composition owner (`canary-runtime`)
/// moves tagged bytes across the transport but never mints tag values:
/// adding a request kind means a new tag here, a dispatch arm in
/// [`Session::handle_frame_body`](crate::session::Session::handle_frame_body),
/// and a protocol-version consideration — never a host-local constant.
/// Tag values, once shipped, are never reused for a different kind.
///
/// Reserved ranges: `0x00` is never valid (empty/untagged input);
/// `0x01`–`0x02` are the `.16` request kinds below; `0x03`–`0x7F` are
/// reserved for future collab request kinds; `0x80`–`0xFF` (from
/// [`TAG_HOST_RESERVED_MIN`]) are reserved for transport/host-level
/// signals and must never name a collab request.
///
/// [`Session::handle_frame_body`](crate::session::Session::handle_frame_body)
/// owns dispatch over these tags; [`split_tagged_body`] and [`tag_body`]
/// own the byte layout. Registry, layout, and governing version live
/// together here so they cannot drift apart.
///
/// Frame tag selecting an edit request.
pub const TAG_EDIT: u8 = 0x01;
/// Frame tag selecting a sync (reconnect cursor) request.
pub const TAG_SYNC: u8 = 0x02;
/// Lowest tag value reserved for transport/host-level signals.
/// Collab request tags must stay below this line.
pub const TAG_HOST_RESERVED_MIN: u8 = 0x80;

/// One edit request: client op ID, stable target, expected revision, and a
/// versioned payload. Identity travels as the pre-shared credential only —
/// there is no actor or role field to forge.
///
/// The [`std::fmt::Debug`] impl redacts [`Self::credential`]: request
/// values are logged on hostile-input paths, and a credential must never
/// reach logs, even in debug builds.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EditRequest {
    /// Must equal [`COLLAB_PROTOCOL_VERSION`].
    pub protocol: u32,
    /// Pre-shared credential; the server maps it to `(actor, role)`.
    pub credential: String,
    /// Client-generated operation ID, scoped to the authenticated actor.
    pub client_op_id: String,
    /// Full authored section name (`entity.<local>`).
    pub target: String,
    /// Target revision the edit is based on (optimistic-concurrency token).
    pub expected_revision: u64,
    /// Payload schema name (`canary.transform` in `.16`).
    pub schema: String,
    /// Payload schema version.
    pub schema_version: u32,
    /// Complete replacement local transform.
    pub payload: TransformPayload,
}

impl std::fmt::Debug for EditRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Credential is always redacted: a fixed marker with no length,
        // prefix, or hash that could aid guessing.
        f.debug_struct("EditRequest")
            .field("protocol", &self.protocol)
            .field("credential", &"[redacted]")
            .field("client_op_id", &self.client_op_id)
            .field("target", &self.target)
            .field("expected_revision", &self.expected_revision)
            .field("schema", &self.schema)
            .field("schema_version", &self.schema_version)
            .field("payload", &self.payload)
            .finish()
    }
}

/// Canonical accepted result, broadcast in sequence order.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcceptEnvelope {
    /// Fencing epoch the operation was sequenced under.
    pub epoch: u64,
    /// Server-minted actor that submitted the operation.
    pub actor: u64,
    /// Echo of the client operation ID.
    pub client_op_id: String,
    /// Full authored section name targeted.
    pub target: String,
    /// Server-assigned history position.
    pub sequence: u64,
    /// Resulting project revision.
    pub project_revision: u64,
    /// Resulting target revision.
    pub target_revision: u64,
    /// Payload schema name.
    pub schema: String,
    /// Payload schema version.
    pub schema_version: u32,
    /// Canonical accepted payload.
    pub payload: TransformPayload,
}

/// Observable rejection: stable code, current revision, and — for
/// revision conflicts — the canonical current target for refresh.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RejectEnvelope {
    /// The wire contract; the message is diagnostic only.
    pub code: RejectCode,
    /// Current target revision (`0` when the target is unknown).
    pub target_revision: u64,
    /// Canonical current target state on conflicts, if it still parses.
    pub refresh: Option<TransformPayload>,
    /// Human-readable detail. Never parsed on the wire.
    pub message: String,
}

/// One session reply: accepted or rejected, never both.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WireResponse {
    /// The operation was durably accepted (and broadcast).
    Accepted(AcceptEnvelope),
    /// The operation was rejected; nothing was assigned or mutated.
    Rejected(RejectEnvelope),
}

/// A reconnect cursor: the last accepted sequence the client holds.
///
/// The [`std::fmt::Debug`] impl redacts [`Self::credential`] for the same
/// reason as [`EditRequest`]: cursors are logged on the sync path and must
/// never carry the credential into logs.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SyncRequest {
    /// Must equal [`COLLAB_PROTOCOL_VERSION`].
    pub protocol: u32,
    /// Pre-shared credential (read-only roles may sync).
    pub credential: String,
    /// Last accepted sequence the client holds (`0` for a fresh join).
    pub last_sequence: u64,
}

impl std::fmt::Debug for SyncRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Same redaction contract as [`EditRequest`]: fixed marker only.
        f.debug_struct("SyncRequest")
            .field("protocol", &self.protocol)
            .field("credential", &"[redacted]")
            .field("last_sequence", &self.last_sequence)
            .finish()
    }
}

/// Catch-up result: either the contiguous retained tail, or a canonical
/// snapshot plus the checkpoint when the client is behind the horizon.
/// Both carry the checkpoint when one exists, so the client always learns
/// its resync base.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SyncResponse {
    /// Fencing epoch of the serving session generation.
    pub epoch: u64,
    /// Contiguous accepted operations after the client's cursor. Empty
    /// when the client is already at the tip or is taking the snapshot
    /// path.
    pub tail: Vec<AcceptEnvelope>,
    /// Resync base from the most recent trim, if any trim ever ran.
    pub checkpoint: Option<CheckpointEnvelope>,
    /// Canonical project JSON when the client is behind the horizon;
    /// `None` on the incremental path.
    pub snapshot: Option<Vec<u8>>,
}

/// Encodes one edit request for the wire.
pub fn encode_request(request: &EditRequest) -> Result<Vec<u8>, CollabError> {
    postcard::to_allocvec(request).map_err(|error| CollabError::Decode(error.to_string()))
}

/// Decodes one edit request behind the length gate (stage 1).
pub fn decode_request(bytes: &[u8]) -> Result<EditRequest, CollabError> {
    gate_length(bytes, MAX_REQUEST_BYTES)?;
    let (request, rest) = postcard::take_from_bytes::<EditRequest>(bytes)
        .map_err(|error| CollabError::Decode(error.to_string()))?;
    gate_exact(rest, "edit request")?;
    bound_string(&request.credential, MAX_CREDENTIAL_BYTES, "credential")?;
    bound_string(
        &request.client_op_id,
        MAX_CLIENT_OP_ID_BYTES,
        "client_op_id",
    )?;
    bound_string(&request.target, MAX_TARGET_BYTES, "target")?;
    bound_string(&request.schema, MAX_SCHEMA_BYTES, "schema")?;
    Ok(request)
}

/// Encodes one session reply behind the response bound.
pub fn encode_response(response: &WireResponse) -> Result<Vec<u8>, CollabError> {
    let bytes =
        postcard::to_allocvec(response).map_err(|error| CollabError::Decode(error.to_string()))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(CollabError::TooLarge {
            claimed: bytes.len(),
            max: MAX_RESPONSE_BYTES,
        });
    }
    Ok(bytes)
}

/// Decodes one session reply behind the response bound.
pub fn decode_response(bytes: &[u8]) -> Result<WireResponse, CollabError> {
    gate_length(bytes, MAX_RESPONSE_BYTES)?;
    let (response, rest) = postcard::take_from_bytes::<WireResponse>(bytes)
        .map_err(|error| CollabError::Decode(error.to_string()))?;
    gate_exact(rest, "collab response")?;
    Ok(response)
}

/// Encodes one sync request for the wire.
pub fn encode_sync_request(request: &SyncRequest) -> Result<Vec<u8>, CollabError> {
    postcard::to_allocvec(request).map_err(|error| CollabError::Decode(error.to_string()))
}

/// Decodes one sync request behind the length gate.
pub fn decode_sync_request(bytes: &[u8]) -> Result<SyncRequest, CollabError> {
    gate_length(bytes, MAX_SYNC_REQUEST_BYTES)?;
    let (request, rest) = postcard::take_from_bytes::<SyncRequest>(bytes)
        .map_err(|error| CollabError::Decode(error.to_string()))?;
    gate_exact(rest, "sync request")?;
    bound_string(&request.credential, MAX_CREDENTIAL_BYTES, "credential")?;
    Ok(request)
}

/// Encodes one sync reply behind the response bound.
pub fn encode_sync_response(response: &SyncResponse) -> Result<Vec<u8>, CollabError> {
    let bytes =
        postcard::to_allocvec(response).map_err(|error| CollabError::Decode(error.to_string()))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(CollabError::TooLarge {
            claimed: bytes.len(),
            max: MAX_RESPONSE_BYTES,
        });
    }
    Ok(bytes)
}

/// Decodes one sync reply behind the response bound.
pub fn decode_sync_response(bytes: &[u8]) -> Result<SyncResponse, CollabError> {
    gate_length(bytes, MAX_RESPONSE_BYTES)?;
    let (response, rest) = postcard::take_from_bytes::<SyncResponse>(bytes)
        .map_err(|error| CollabError::Decode(error.to_string()))?;
    gate_exact(rest, "sync response")?;
    Ok(response)
}

/// Splits a tagged frame body into its tag and codec bytes. Empty input
/// yields the never-valid `0x00` tag with the whole (empty) body as
/// payload, so dispatch answers it with the generic `Malformed` reject
/// instead of panicking on an empty slice.
#[must_use]
pub fn split_tagged_body(body: &[u8]) -> (u8, &[u8]) {
    body.split_first()
        .map_or((0x00, body), |(tag, rest)| (*tag, rest))
}

/// Prefixes codec `bytes` with `tag` for the wire.
#[must_use]
pub fn tag_body(tag: u8, bytes: &[u8]) -> Vec<u8> {
    let mut tagged = Vec::with_capacity(bytes.len() + 1);
    tagged.push(tag);
    tagged.extend_from_slice(bytes);
    tagged
}

/// Builds one tagged edit frame body for tests and thin clients: the
/// [`TAG_EDIT`] tag plus the encoded request.
#[must_use]
pub fn tag_edit_body(request: &EditRequest) -> Vec<u8> {
    let mut body = vec![TAG_EDIT];
    if let Ok(bytes) = encode_request(request) {
        body.extend_from_slice(&bytes);
    }
    body
}

fn gate_length(bytes: &[u8], max: usize) -> Result<(), CollabError> {
    if bytes.len() > max {
        return Err(CollabError::TooLarge {
            claimed: bytes.len(),
            max,
        });
    }
    Ok(())
}

fn gate_exact(rest: &[u8], what: &str) -> Result<(), CollabError> {
    if rest.is_empty() {
        Ok(())
    } else {
        Err(CollabError::Decode(format!(
            "{what} has {} trailing bytes",
            rest.len()
        )))
    }
}

fn bound_string(value: &str, max: usize, field: &str) -> Result<(), CollabError> {
    if value.len() > max {
        return Err(CollabError::Decode(format!(
            "field '{field}' exceeds {max} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::TransformPayload;

    fn sync_request() -> SyncRequest {
        SyncRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: "s3cr3t-sync".to_owned(),
            last_sequence: 0,
        }
    }

    fn edit_request_with_secret() -> EditRequest {
        EditRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: "s3cr3t-edit".to_owned(),
            client_op_id: "op-1".to_owned(),
            target: "entity.hero".to_owned(),
            expected_revision: 0,
            schema: canary_state::TRANSFORM_SCHEMA_KEY.to_owned(),
            schema_version: canary_state::TRANSFORM_SCHEMA_VERSION,
            payload: TransformPayload::identity(),
        }
    }

    #[test]
    fn edit_request_debug_redacts_credential() {
        let rendered = format!("{:?}", edit_request_with_secret());
        assert!(
            !rendered.contains("s3cr3t-edit"),
            "EditRequest Debug must not leak the credential: {rendered}"
        );
        assert!(
            rendered.contains("[redacted]"),
            "EditRequest Debug must mark the credential redacted: {rendered}"
        );
    }

    #[test]
    fn sync_request_debug_redacts_credential() {
        let rendered = format!("{:?}", sync_request());
        assert!(
            !rendered.contains("s3cr3t-sync"),
            "SyncRequest Debug must not leak the credential: {rendered}"
        );
        assert!(
            rendered.contains("[redacted]"),
            "SyncRequest Debug must mark the credential redacted: {rendered}"
        );
    }
    fn request() -> EditRequest {
        EditRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: "owner-secret".to_owned(),
            client_op_id: "op-1".to_owned(),
            target: "entity.hero".to_owned(),
            expected_revision: 0,
            schema: canary_state::TRANSFORM_SCHEMA_KEY.to_owned(),
            schema_version: canary_state::TRANSFORM_SCHEMA_VERSION,
            payload: TransformPayload::identity(),
        }
    }

    #[test]
    fn request_round_trips_through_postcard() {
        let bytes = encode_request(&request()).expect("encode");
        assert!(bytes.len() < MAX_REQUEST_BYTES);
        assert_eq!(decode_request(&bytes).expect("decode"), request());
    }

    #[test]
    fn oversize_bytes_fail_before_decode() {
        let huge = vec![0xFFu8; MAX_REQUEST_BYTES + 1];
        assert!(matches!(
            decode_request(&huge),
            Err(CollabError::TooLarge { .. })
        ));
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut bytes = encode_request(&request()).expect("encode");
        bytes.push(0x00);
        assert!(matches!(
            decode_request(&bytes),
            Err(CollabError::Decode(_))
        ));
    }

    #[test]
    fn overlong_strings_fail_after_decode() {
        let mut bad = request();
        bad.credential = "c".repeat(MAX_CREDENTIAL_BYTES + 1);
        let bytes = encode_request(&bad).expect("encode of overlong still fits");
        assert!(matches!(
            decode_request(&bytes),
            Err(CollabError::Decode(_))
        ));
    }

    #[test]
    fn response_variants_round_trip() {
        let accept = WireResponse::Accepted(AcceptEnvelope {
            epoch: 1,
            actor: 1,
            client_op_id: "op-1".to_owned(),
            target: "entity.hero".to_owned(),
            sequence: 1,
            project_revision: 1,
            target_revision: 1,
            schema: canary_state::TRANSFORM_SCHEMA_KEY.to_owned(),
            schema_version: canary_state::TRANSFORM_SCHEMA_VERSION,
            payload: TransformPayload::identity(),
        });
        let bytes = encode_response(&accept).expect("encode");
        assert_eq!(decode_response(&bytes).expect("decode"), accept);

        let reject = WireResponse::Rejected(RejectEnvelope {
            code: RejectCode::RevisionConflict,
            target_revision: 1,
            refresh: Some(TransformPayload::identity()),
            message: "stale".to_owned(),
        });
        let bytes = encode_response(&reject).expect("encode");
        assert_eq!(decode_response(&bytes).expect("decode"), reject);
    }

    #[test]
    fn sync_codecs_round_trip_behind_bounds() {
        let ask = SyncRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: "reader-secret".to_owned(),
            last_sequence: 0,
        };
        let bytes = encode_sync_request(&ask).expect("encode");
        assert!(bytes.len() < MAX_SYNC_REQUEST_BYTES);
        assert_eq!(decode_sync_request(&bytes).expect("decode"), ask);

        let reply = SyncResponse {
            epoch: 1,
            tail: Vec::new(),
            checkpoint: None,
            snapshot: Some(b"{}".to_vec()),
        };
        let bytes = encode_sync_response(&reply).expect("encode");
        assert_eq!(decode_sync_response(&bytes).expect("decode"), reply);
    }

    #[test]
    fn tag_registry_holds_two_kinds_below_the_host_line() {
        // Compile-time registry pins: a reused or out-of-range tag must
        // fail the build, not a test run.
        const { assert!(TAG_EDIT != TAG_SYNC, "request kinds need distinct tags") };
        const {
            assert!(
                TAG_EDIT != 0x00 && TAG_SYNC != 0x00,
                "0x00 is never a valid tag"
            )
        };
        const {
            assert!(
                TAG_EDIT < TAG_HOST_RESERVED_MIN && TAG_SYNC < TAG_HOST_RESERVED_MIN,
                "collab request tags stay below the host-reserved line"
            )
        };
    }

    #[test]
    fn tagged_bodies_split_and_rebuild_exactly() {
        let tagged = tag_edit_body(&request());
        let (tag, body) = split_tagged_body(&tagged);
        assert_eq!(tag, TAG_EDIT);
        assert_eq!(decode_request(body).expect("decode"), request());
        assert_eq!(tag_body(tag, body), tagged);

        // Empty input yields the never-valid tag, never a panic: dispatch
        // answers it with the generic `Malformed` reject.
        assert_eq!(split_tagged_body(&[]), (0x00, [].as_slice()));
        assert_eq!(split_tagged_body(&[0x7F]), (0x7F, [].as_slice()));
    }
}
