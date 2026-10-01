//! Collaboration revision domains: stable entity targets, revision
//! counters, and the additive in-document operation history.
//!
//! This module is the `.16` WP1 answer to three open definition items from
//! ADR 0028: the target-ID type, the revision/sequence counters and their
//! relation to the [`AuthoredDocument`](crate::authored::AuthoredDocument)
//! human change log, and the prefab-veto location.
//!
//! Selections, recorded as amendment notes in ADR 0028:
//!
//! - [`LogicalEntityId`] is a validated `entity.<local>` section suffix.
//!   There are no renames in `.16`: the local name is the identity.
//! - Operation history is an additive in-document section
//!   ([`DocumentHistory`], carried by `AuthoredDocument::history`), not a
//!   sidecar file. It rides the same canonical JSON and the same atomic
//!   save as the authored state it versions, so one durable commit covers
//!   both. The envelope stays `canary.project` version 1, encoding 1.
//! - The prefab veto lives here as
//!   [`AuthoredDocument::transform_override_allowed`](crate::authored::AuthoredDocument::transform_override_allowed),
//!   invoked by `canary-collab` during validation. It reuses the
//!   one-level prefab resolve/bake rules, never a parallel policy.
//!
//! [`ProjectRevision`], [`ObjectRevision`], and [`OperationSequence`] are
//! distinct `u64` newtypes that are never compared or substituted for one
//! another, and never for the legacy [`AuthoredChange`](crate::authored::AuthoredChange)
//! sequence: the human change log neither seeds nor mirrors the operation
//! counters (see [`DocumentHistory::genesis`]).
//!
//! This module keeps the crate a leaf: only `serde`, `serde_json`, and
//! `sha2` cross the boundary, never another `canary-*` crate.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::authored::AuthoredDocument;
use crate::error::StateError;
use crate::schema::{SchemaId, SchemaVersion};
use crate::spawn_plan::{ENTITY_PREFIX, PREFAB_KEY};

/// Authored schema key holding an entity's local transform component.
///
/// Entity sections map component schema names to field objects; the
/// collaboration operation replaces exactly this entry.
pub const TRANSFORM_SCHEMA_KEY: &str = "canary.transform";

/// Version of the transform component schema this build authors.
pub const TRANSFORM_SCHEMA_VERSION: u32 = 1;

/// The transform component schema this build authors.
#[must_use]
pub fn transform_schema() -> (SchemaId, SchemaVersion) {
    (
        SchemaId::new(TRANSFORM_SCHEMA_KEY),
        SchemaVersion(TRANSFORM_SCHEMA_VERSION),
    )
}

/// Stable authored target of a collaboration operation: the validated
/// `entity.<local>` section suffix.
///
/// There are no renames in `.16` — the local name is the identity, stored
/// verbatim, so a rename is a delete-plus-create, never a silent retarget.
/// Construction validates; there is deliberately no `From<String>`, so an
/// unchecked string can never become a target by accident.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogicalEntityId(String);

impl LogicalEntityId {
    /// Builds an ID from a full authored section name (`entity.<local>`).
    ///
    /// Rejects a missing `entity.` prefix, an empty local name, a local
    /// name containing `.`, `/`, or whitespace, and the reserved `prefab`
    /// section key (which names prefab references, never entities).
    pub fn from_section_name(section: &str) -> Result<Self, StateError> {
        let local = section
            .strip_prefix(ENTITY_PREFIX)
            .ok_or_else(|| StateError::File {
                path: Path::new("<document>").to_path_buf(),
                reason: format!("section '{section}' is not an entity section"),
            })?;
        Self::from_local(local)
    }

    /// Builds an ID from the local name alone (the part after `entity.`).
    ///
    /// Same validation as [`Self::from_section_name`] minus the prefix.
    pub fn from_local(local: &str) -> Result<Self, StateError> {
        let invalid = local.is_empty()
            || local == PREFAB_KEY
            || local.contains('.')
            || local.contains('/')
            || local.chars().any(char::is_whitespace);
        if invalid {
            return Err(StateError::File {
                path: Path::new("<document>").to_path_buf(),
                reason: format!("invalid entity local name '{local}'"),
            });
        }
        Ok(Self(local.to_owned()))
    }

    /// The local name (the section name after `entity.`).
    #[must_use]
    pub fn local(&self) -> &str {
        &self.0
    }

    /// The full authored section name (`entity.<local>`).
    #[must_use]
    pub fn section_name(&self) -> String {
        format!("{ENTITY_PREFIX}{}", self.0)
    }
}

impl Serialize for LogicalEntityId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for LogicalEntityId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let local = String::deserialize(deserializer)?;
        Self::from_local(&local).map_err(serde::de::Error::custom)
    }
}

/// Project-wide revision: bumps once per accepted operation.
///
/// Never compared with [`ObjectRevision`] or [`OperationSequence`], and
/// never derived from the legacy human change log.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct ProjectRevision(pub u64);

/// Per-target revision: the optimistic-concurrency token for one entity.
///
/// Bumps only when that entity is the accepted target. Unrelated edits
/// never move it, so independent targets never conflict.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct ObjectRevision(pub u64);

/// Server-assigned position of one accepted operation in project history.
///
/// Monotonic within the version line and stable across restarts (reloaded
/// from the durable history, never reused). Never a timestamp, tick, or
/// network sequence.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct OperationSequence(pub u64);

/// One accepted operation, as retained in document history.
///
/// Rejected requests never produce one of these. The payload is the
/// canonical authored field object (for the `.16` operation, the
/// `canary.transform` fields), so a replay from history needs no codec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcceptedHistoryRecord {
    /// Server-minted actor that submitted the operation.
    #[serde(default)]
    pub actor: u64,
    /// Client-generated operation ID, scoped to `actor`.
    #[serde(default)]
    pub client_op_id: String,
    /// The entity the operation targeted.
    pub target: LogicalEntityId,
    /// Target revision the client based its edit on.
    #[serde(default)]
    pub expected_revision: ObjectRevision,
    /// Server-assigned history position.
    #[serde(default)]
    pub sequence: OperationSequence,
    /// Project revision resulting from this operation.
    #[serde(default)]
    pub project_revision: ProjectRevision,
    /// Target revision resulting from this operation.
    #[serde(default)]
    pub target_revision: ObjectRevision,
    /// Schema the payload was validated against.
    #[serde(default = "default_transform_schema_id")]
    pub schema: SchemaId,
    /// Schema version the payload was validated against.
    #[serde(default = "default_transform_schema_version")]
    pub schema_version: SchemaVersion,
    /// Canonical authored field object applied by this operation.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Fields for an operation the history has not sequenced yet.
///
/// [`DocumentHistory::accept`] assigns the sequence and both resulting
/// revisions itself, so callers cannot mint a revision by hand.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAccept {
    /// Server-minted actor that submitted the operation.
    pub actor: u64,
    /// Client-generated operation ID, scoped to `actor`.
    pub client_op_id: String,
    /// The entity the operation targeted.
    pub target: LogicalEntityId,
    /// Target revision the client based its edit on.
    pub expected_revision: ObjectRevision,
    /// Schema the payload was validated against.
    pub schema: SchemaId,
    /// Schema version the payload was validated against.
    pub schema_version: SchemaVersion,
    /// Canonical authored field object to apply.
    pub payload: serde_json::Value,
}

fn default_transform_schema_id() -> SchemaId {
    SchemaId::new(TRANSFORM_SCHEMA_KEY)
}

fn default_transform_schema_version() -> SchemaVersion {
    SchemaVersion(TRANSFORM_SCHEMA_VERSION)
}

/// Resync base carried by every history trim (ADR 0028 addendum A3).
///
/// A client behind [`Self::last_retained`] takes the
/// snapshot-plus-checkpoint path, never a partial tail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointEnvelope {
    /// Project revision at the trim.
    #[serde(default)]
    pub project: ProjectRevision,
    /// Sequence of the last evicted operation (`OperationSequence(0)` when
    /// nothing was ever evicted).
    #[serde(default)]
    pub last_retained: OperationSequence,
    /// Hex SHA-256 over the trim state (`project`, `last_retained`, and
    /// the lifetime evicted count), so a checkpoint is self-identifying.
    #[serde(default)]
    pub marker: String,
}

/// The additive in-document operation history (the `history` section).
///
/// Present by `#[serde(default)]` on [`AuthoredDocument`], skipped on save
/// while empty — so files that never saw collaboration are byte-identical
/// to pre-history files, and legacy files load with genesis counters.
/// The envelope stays `canary.project` version 1, encoding 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentHistory {
    /// Current project revision (genesis: 0).
    #[serde(default)]
    pub project_revision: ProjectRevision,
    /// Next sequence the server will assign (genesis: 1).
    #[serde(default = "genesis_sequence")]
    pub next_sequence: OperationSequence,
    /// Per-target revisions by local entity name (absent: 0).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, ObjectRevision>,
    /// Retained accepted tail, oldest first, in sequence order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<AcceptedHistoryRecord>,
    /// Resync base from the most recent trim, if any trim ever ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<CheckpointEnvelope>,
    /// Lifetime evicted-operation count; feeds the checkpoint marker.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub evicted: u64,
}

fn genesis_sequence() -> OperationSequence {
    OperationSequence(1)
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl Default for DocumentHistory {
    fn default() -> Self {
        Self::genesis()
    }
}

impl DocumentHistory {
    /// Genesis counters: project 0, every target 0, next sequence 1.
    ///
    /// Derived from nothing — in particular, never from the legacy human
    /// change log: a document with fifty `changes` entries and no history
    /// still opens at project 0 / next-sequence 1.
    #[must_use]
    pub fn genesis() -> Self {
        Self {
            project_revision: ProjectRevision(0),
            next_sequence: OperationSequence(1),
            targets: BTreeMap::new(),
            accepted: Vec::new(),
            checkpoint: None,
            evicted: 0,
        }
    }

    /// Whether this history carries no collaboration state at all.
    ///
    /// An empty history serializes to nothing (`skip_serializing_if`), so
    /// this is also the save gate that keeps collaboration-naive files
    /// byte-stable.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty()
            && self.checkpoint.is_none()
            && self.targets.is_empty()
            && self.evicted == 0
    }

    /// Current revision of one target by local entity name (absent: 0).
    #[must_use]
    pub fn target_revision(&self, local: &str) -> ObjectRevision {
        self.targets.get(local).copied().unwrap_or_default()
    }

    /// Sequences `pending`: assigns its history position and both
    /// resulting revisions, records per-target state, and appends the
    /// retained record. Returns the completed record.
    pub fn accept(&mut self, pending: PendingAccept) -> AcceptedHistoryRecord {
        let sequence = self.next_sequence;
        let project_revision = ProjectRevision(self.project_revision.0.saturating_add(1));
        let target_revision = ObjectRevision(pending.expected_revision.0.saturating_add(1));
        let record = AcceptedHistoryRecord {
            actor: pending.actor,
            client_op_id: pending.client_op_id,
            expected_revision: pending.expected_revision,
            sequence,
            project_revision,
            target_revision,
            schema: pending.schema,
            schema_version: pending.schema_version,
            payload: pending.payload,
            target: pending.target.clone(),
        };
        self.project_revision = project_revision;
        self.next_sequence = OperationSequence(sequence.0.saturating_add(1));
        self.targets
            .insert(pending.target.local().to_owned(), target_revision);
        self.accepted.push(record.clone());
        record
    }

    /// Evicts the oldest retained records until at most `max` remain.
    ///
    /// Each eviction advances the checkpoint envelope (project revision at
    /// the trim, last evicted sequence, fresh SHA-256 marker), so a late
    /// joiner or evicted client always has a defined resync base. Returns
    /// the current checkpoint when at least one record was evicted.
    pub fn trim_retained(&mut self, max: usize) -> Option<CheckpointEnvelope> {
        let max = max.max(1);
        let mut evicted_any = false;
        while self.accepted.len() > max {
            let oldest = self.accepted.remove(0);
            self.evicted = self.evicted.saturating_add(1);
            self.checkpoint = Some(CheckpointEnvelope {
                project: oldest.project_revision,
                last_retained: oldest.sequence,
                marker: checkpoint_marker(oldest.project_revision, oldest.sequence, self.evicted),
            });
            evicted_any = true;
        }
        if evicted_any {
            self.checkpoint.clone()
        } else {
            None
        }
    }

    /// Newest retained sequence, or the checkpoint horizon when the tail
    /// is empty (genesis: sequence 0, meaning "nothing accepted yet").
    #[must_use]
    pub fn newest_retained(&self) -> OperationSequence {
        self.accepted
            .last()
            .map(|record| record.sequence)
            .unwrap_or_else(|| {
                self.checkpoint
                    .as_ref()
                    .map(|checkpoint| checkpoint.last_retained)
                    .unwrap_or_default()
            })
    }

    /// Resolves a client's `last_sequence` cursor against the retained
    /// tail: the contiguous records after `last` when the client is still
    /// covered, or the gap kind (behind the checkpoint horizon, or ahead
    /// of history) when it must resync instead of receiving a partial tail.
    pub fn tail_since(&self, last: OperationSequence) -> Result<&[AcceptedHistoryRecord], TailGap> {
        let horizon = self
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.last_retained)
            .unwrap_or_default();
        if last.0 < horizon.0 {
            return Err(TailGap::BehindCheckpoint);
        }
        let newest = self.newest_retained();
        if last.0 > newest.0 {
            return Err(TailGap::AheadOfHistory);
        }
        let start = self
            .accepted
            .iter()
            .position(|record| record.sequence.0 > last.0)
            .unwrap_or(self.accepted.len());
        Ok(&self.accepted[start..])
    }
}

/// Why a history cursor cannot be served a contiguous tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailGap {
    /// The cursor predates the checkpoint horizon: the client takes the
    /// snapshot-plus-checkpoint path, never a partial tail.
    BehindCheckpoint,
    /// The cursor claims a sequence newer than anything retained: a stale
    /// or forged cursor, rejected rather than silently accepted.
    AheadOfHistory,
}

/// Builds the checkpoint marker: hex SHA-256 over the trim state.
fn checkpoint_marker(
    project: ProjectRevision,
    last_retained: OperationSequence,
    evicted: u64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"canary-checkpoint:v1:");
    hasher.update(project.0.to_le_bytes());
    hasher.update(b":");
    hasher.update(last_retained.0.to_le_bytes());
    hasher.update(b":");
    hasher.update(evicted.to_le_bytes());
    let digest = hasher.finalize();
    let mut marker = String::with_capacity(digest.len() * 2);
    for byte in digest {
        marker.push_str(&format!("{byte:02x}"));
    }
    marker
}

impl AuthoredDocument {
    /// Whether an entity section exists for `id`.
    #[must_use]
    pub fn entity_section_exists(&self, id: &LogicalEntityId) -> bool {
        self.sections.contains_key(&id.section_name())
    }

    /// Whether a collaboration operation may replace `id`'s local
    /// transform.
    ///
    /// The rule reuses the one-level prefab resolve/bake semantics, never
    /// a parallel policy: a prefab-free entity is always editable; a
    /// prefab instance is vetoed (`Ok(false)`) when its prefab reference
    /// is missing, malformed, unknown, chained, nested, or supplies the
    /// transform the instance never declared its own override for —
    /// introducing a remote override over prefab-driven data must stay an
    /// explicit authored act (declare the override in the document first).
    /// An instance that already carries its own transform entry, or whose
    /// prefab says nothing about transforms, stays editable.
    pub fn transform_override_allowed(&self, id: &LogicalEntityId) -> Result<bool, StateError> {
        let section = self
            .sections
            .get(&id.section_name())
            .ok_or_else(|| StateError::File {
                path: Path::new("<document>").to_path_buf(),
                reason: format!("no entity section '{}'", id.section_name()),
            })?;
        let components = section.as_object().ok_or_else(|| StateError::File {
            path: Path::new("<document>").to_path_buf(),
            reason: format!("section '{}' is not an object", id.section_name()),
        })?;
        let prefab_name = match components.get(PREFAB_KEY) {
            None => return Ok(true),
            Some(serde_json::Value::String(name)) => name.clone(),
            Some(_) => return Ok(false),
        };
        let resolved = match self.resolve_prefab(&prefab_name) {
            Ok(fields) => fields,
            Err(_) => return Ok(false),
        };
        if resolved.contains_key(PREFAB_KEY) {
            return Ok(false);
        }
        let Some(base_transform) = resolved.get(TRANSFORM_SCHEMA_KEY) else {
            return Ok(true);
        };
        if !base_transform.is_object() {
            return Ok(false);
        }
        Ok(components.contains_key(TRANSFORM_SCHEMA_KEY))
    }

    /// Reads the canonical current transform field object for `id`, if
    /// the section exists, is an object, and carries the transform entry.
    #[must_use]
    pub fn entity_transform(&self, id: &LogicalEntityId) -> Option<serde_json::Value> {
        self.sections
            .get(&id.section_name())?
            .as_object()?
            .get(TRANSFORM_SCHEMA_KEY)
            .cloned()
    }

    /// Replaces the transform field object of an existing entity section.
    ///
    /// The section must exist and be an object, and `fields` must be an
    /// object — otherwise the document is left untouched and a typed
    /// error is returned. Prefab policy is the caller's check
    /// ([`Self::transform_override_allowed`]); this only writes.
    pub fn set_entity_transform(
        &mut self,
        id: &LogicalEntityId,
        fields: serde_json::Value,
    ) -> Result<(), StateError> {
        if !fields.is_object() {
            return Err(StateError::File {
                path: Path::new("<document>").to_path_buf(),
                reason: format!(
                    "transform fields for '{}' must be an object",
                    id.section_name()
                ),
            });
        }
        let section_name = id.section_name();
        let section = self
            .sections
            .get_mut(&section_name)
            .ok_or_else(|| StateError::File {
                path: Path::new("<document>").to_path_buf(),
                reason: format!("no entity section '{section_name}'"),
            })?;
        let components = section.as_object_mut().ok_or_else(|| StateError::File {
            path: Path::new("<document>").to_path_buf(),
            reason: format!("section '{section_name}' is not an object"),
        })?;
        components.insert(TRANSFORM_SCHEMA_KEY.to_owned(), fields);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authored::Prefab;
    use crate::identity::ProjectId;

    fn doc() -> AuthoredDocument {
        AuthoredDocument::new(ProjectId::generate().expect("os randomness"))
    }

    #[test]
    fn section_suffix_validates_and_round_trips() {
        let id = LogicalEntityId::from_section_name("entity.hero").expect("valid");
        assert_eq!(id.local(), "hero");
        assert_eq!(id.section_name(), "entity.hero");
    }

    #[test]
    fn invalid_locals_are_rejected() {
        for bad in [
            "hero.boss",
            "a/b",
            "with space",
            "with\ttab",
            "",
            PREFAB_KEY,
        ] {
            assert!(
                LogicalEntityId::from_local(bad).is_err(),
                "local name '{bad}' must be rejected"
            );
        }
        assert!(LogicalEntityId::from_section_name("settings").is_err());
        assert!(LogicalEntityId::from_section_name("entity.").is_err());
        assert!(LogicalEntityId::from_section_name("entity.a.b").is_err());
    }

    #[test]
    fn history_ids_do_not_deserialize_unchecked() {
        let parsed: Result<LogicalEntityId, _> = serde_json::from_str("\"a.b\"");
        assert!(
            parsed.is_err(),
            "dotted names must fail typed deserialization"
        );
        let parsed: LogicalEntityId =
            serde_json::from_str("\"hero\"").expect("plain local name parses");
        assert_eq!(parsed.local(), "hero");
    }

    #[test]
    fn revision_counters_are_distinct_types() {
        // This test pins the WP1 selection at the type level: the three
        // counters share a representation but no identity. If any of these
        // assignments ever compiles, the domains have been conflated.
        fn takes_project(_: ProjectRevision) {}
        fn takes_object(_: ObjectRevision) {}
        fn takes_sequence(_: OperationSequence) {}
        takes_project(ProjectRevision(1));
        takes_object(ObjectRevision(1));
        takes_sequence(OperationSequence(1));
        assert_ne!(
            ProjectRevision(1).0,
            ObjectRevision(2).0,
            "sanity: representation overlap is fine, type overlap is not"
        );
    }

    #[test]
    fn genesis_ignores_the_human_change_log() {
        let mut document = doc();
        for index in 0..5 {
            document.record_change(&format!("human edit {index}"));
        }
        assert_eq!(document.changes.len(), 5);
        assert!(document.history.is_empty());
        assert_eq!(document.history.project_revision, ProjectRevision(0));
        assert_eq!(document.history.next_sequence, OperationSequence(1));
        assert_eq!(document.history.target_revision("hero"), ObjectRevision(0));
    }

    #[test]
    fn legacy_files_load_with_genesis_history() {
        let text = r#"{"envelope":{"schema":"canary.project","version":1,"encoding":1},
            "project":"123e4567-e89b-42d3-a456-426614174000",
            "sections":{"entity.hero":{"canary.transform":{"translation":[0,0,0]}}},
            "changes":[{"sequence":3,"description":"old edit"}]}"#;
        let parsed = AuthoredDocument::from_canonical_json(text).expect("parse");
        assert_eq!(parsed.history.next_sequence, OperationSequence(1));
        assert_eq!(parsed.history.project_revision, ProjectRevision(0));
        assert_eq!(parsed.changes.len(), 1);
    }

    #[test]
    fn empty_history_stays_out_of_the_bytes() {
        let first = doc();
        let text = first.to_canonical_json().expect("serialize");
        assert!(
            !text.contains("history"),
            "genesis history is skipped: {text}"
        );
        let again = AuthoredDocument::from_canonical_json(&text).expect("reparse");
        assert_eq!(again, first);
    }

    #[test]
    fn accept_assigns_sequence_and_both_revisions() {
        let mut history = DocumentHistory::genesis();
        let target = LogicalEntityId::from_local("hero").expect("valid");
        let first = history.accept(PendingAccept {
            actor: 7,
            client_op_id: "op-1".to_owned(),
            target: target.clone(),
            expected_revision: ObjectRevision(0),
            schema: SchemaId::new(TRANSFORM_SCHEMA_KEY),
            schema_version: SchemaVersion(TRANSFORM_SCHEMA_VERSION),
            payload: serde_json::json!({"translation": [1, 0, 0]}),
        });
        assert_eq!(first.sequence, OperationSequence(1));
        assert_eq!(first.project_revision, ProjectRevision(1));
        assert_eq!(first.target_revision, ObjectRevision(1));
        assert_eq!(history.next_sequence, OperationSequence(2));
        assert_eq!(history.target_revision("hero"), ObjectRevision(1));
        assert_eq!(history.target_revision("other"), ObjectRevision(0));

        let second = history.accept(PendingAccept {
            actor: 7,
            client_op_id: "op-2".to_owned(),
            target,
            expected_revision: ObjectRevision(1),
            schema: SchemaId::new(TRANSFORM_SCHEMA_KEY),
            schema_version: SchemaVersion(TRANSFORM_SCHEMA_VERSION),
            payload: serde_json::json!({"translation": [2, 0, 0]}),
        });
        assert_eq!(second.sequence, OperationSequence(2));
        assert_eq!(second.project_revision, ProjectRevision(2));
    }

    #[test]
    fn trim_evicts_oldest_first_and_carries_a_checkpoint() {
        let mut history = DocumentHistory::genesis();
        for index in 0..4 {
            history.accept(PendingAccept {
                actor: 1,
                client_op_id: format!("op-{index}"),
                target: LogicalEntityId::from_local("hero").expect("valid"),
                expected_revision: ObjectRevision(index),
                schema: SchemaId::new(TRANSFORM_SCHEMA_KEY),
                schema_version: SchemaVersion(TRANSFORM_SCHEMA_VERSION),
                payload: serde_json::json!({}),
            });
        }
        assert!(
            history.trim_retained(128).is_none(),
            "no trim under the bound"
        );
        let checkpoint = history.trim_retained(2).expect("trim to two");
        assert_eq!(history.accepted.len(), 2);
        assert_eq!(history.accepted[0].sequence, OperationSequence(3));
        assert_eq!(checkpoint.last_retained, OperationSequence(2));
        assert_eq!(checkpoint.project, ProjectRevision(2));
        assert_eq!(checkpoint.marker.len(), 64, "hex SHA-256");
        assert_eq!(history.evicted, 2);

        // The checkpoint survives a save/load round trip with the document.
        let mut document = doc();
        document.history = history;
        let text = document.to_canonical_json().expect("serialize");
        assert!(text.contains("checkpoint"), "checkpoint is durable: {text}");
        let again = AuthoredDocument::from_canonical_json(&text).expect("reparse");
        assert_eq!(again.history, document.history);
    }

    #[test]
    fn tail_cursor_rules_never_yield_a_partial_tail() {
        let mut history = DocumentHistory::genesis();
        for index in 0..3 {
            history.accept(PendingAccept {
                actor: 1,
                client_op_id: format!("op-{index}"),
                target: LogicalEntityId::from_local("hero").expect("valid"),
                expected_revision: ObjectRevision(index),
                schema: SchemaId::new(TRANSFORM_SCHEMA_KEY),
                schema_version: SchemaVersion(TRANSFORM_SCHEMA_VERSION),
                payload: serde_json::json!({}),
            });
        }
        history.trim_retained(2);
        // Genesis cursor is behind the horizon: snapshot, not a partial tail.
        assert_eq!(
            history.tail_since(OperationSequence(0)),
            Err(TailGap::BehindCheckpoint)
        );
        let tail = history.tail_since(OperationSequence(2)).expect("covered");
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].sequence, OperationSequence(3));
        assert!(history
            .tail_since(OperationSequence(3))
            .expect("at tip")
            .is_empty());
        assert_eq!(
            history.tail_since(OperationSequence(9)),
            Err(TailGap::AheadOfHistory)
        );
    }

    #[test]
    fn prefab_free_entities_allow_transform_override() {
        let mut document = doc();
        document.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({"canary.transform": {"translation": [0, 0, 0]}}),
        );
        let id = LogicalEntityId::from_local("hero").expect("valid");
        assert!(document.entity_section_exists(&id));
        assert!(document.transform_override_allowed(&id).expect("gate"));
    }

    #[test]
    fn prefab_instances_without_their_own_transform_are_vetoed() {
        let mut document = doc();
        document.prefabs.insert(
            "goblin".to_owned(),
            Prefab {
                base: None,
                overrides: BTreeMap::from([(
                    TRANSFORM_SCHEMA_KEY.to_owned(),
                    serde_json::json!({"translation": [0, 0, 0]}),
                )]),
            },
        );
        document.sections.insert(
            "entity.grunt".to_owned(),
            serde_json::json!({"prefab": "goblin"}),
        );
        let id = LogicalEntityId::from_local("grunt").expect("valid");
        assert!(document.entity_section_exists(&id));
        assert!(
            !document.transform_override_allowed(&id).expect("gate"),
            "instance would override prefab-driven data it never declared"
        );
    }

    #[test]
    fn prefab_instances_with_their_own_transform_stay_editable() {
        let mut document = doc();
        document.prefabs.insert(
            "goblin".to_owned(),
            Prefab {
                base: None,
                overrides: BTreeMap::from([(
                    TRANSFORM_SCHEMA_KEY.to_owned(),
                    serde_json::json!({"translation": [0, 0, 0]}),
                )]),
            },
        );
        document.sections.insert(
            "entity.chief".to_owned(),
            serde_json::json!({
                "prefab": "goblin",
                "canary.transform": {"translation": [5, 0, 0]},
            }),
        );
        let id = LogicalEntityId::from_local("chief").expect("valid");
        assert!(document.transform_override_allowed(&id).expect("gate"));
    }

    #[test]
    fn unknown_or_chained_prefabs_veto_closed() {
        let mut document = doc();
        document.sections.insert(
            "entity.lost".to_owned(),
            serde_json::json!({"prefab": "no-such-prefab"}),
        );
        let lost = LogicalEntityId::from_local("lost").expect("valid");
        assert!(!document.transform_override_allowed(&lost).expect("gate"));

        let missing = LogicalEntityId::from_local("missing").expect("valid");
        assert!(!document.entity_section_exists(&missing));
        assert!(document.transform_override_allowed(&missing).is_err());
    }

    #[test]
    fn set_entity_transform_replaces_only_the_transform_entry() {
        let mut document = doc();
        document.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({
                "canary.transform": {"translation": [0, 0, 0]},
                "test.health": {"hp": 10},
            }),
        );
        let id = LogicalEntityId::from_local("hero").expect("valid");
        document
            .set_entity_transform(&id, serde_json::json!({"translation": [1, 2, 3]}))
            .expect("write");
        let section = &document.sections["entity.hero"];
        assert_eq!(
            section["canary.transform"],
            serde_json::json!({"translation": [1, 2, 3]})
        );
        assert_eq!(section["test.health"], serde_json::json!({"hp": 10}));
        assert_eq!(
            document.entity_transform(&id),
            Some(serde_json::json!({"translation": [1, 2, 3]}))
        );
    }
}
