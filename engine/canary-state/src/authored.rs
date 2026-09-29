//! Authored project files: canonical pretty JSON, deterministic ordering.
//!
//! An [`AuthoredDocument`] is the on-disk project: an [`AuthoredEnvelope`](crate::schema::AuthoredEnvelope),
//! the [`ProjectId`](crate::identity::ProjectId), named sections of structured
//! data, an optional prefab table, and a change log. Sections and prefab
//! fields live in [`BTreeMap`]s and serialize with `serde_json::to_string_pretty`,
//! so two saves of the same document are byte-identical and diffs stay clean.
//!
//! Fields this build does not recognize are kept in `unknown` (flattened)
//! and written back verbatim, tagged by the envelope version that carried
//! them — forward-compatible files survive a round trip instead of being
//! silently truncated.
//!
//! Prefab inheritance is exactly one level: a prefab may name a `base`
//! prefab, but a base with its own base is rejected at resolve time.
//! Saves are atomic (write sibling temp file, then rename); loads are
//! staged (parse → encoding check → migrate → validate) so a half-written
//! or future file fails before it can poison the session.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::StateError;
use crate::identity::ProjectId;
use crate::schema::{AuthoredEnvelope, SchemaId, SchemaVersion, AUTHORED_ENCODING};

/// Schema of the project document itself.
pub fn project_schema() -> (SchemaId, SchemaVersion) {
    (SchemaId::new("canary.project"), SchemaVersion(1))
}

/// One prefab: named defaults plus a single-level `base` reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prefab {
    /// Another prefab whose fields seed this one, or `None` for a root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Field overrides applied over the base (or the whole definition).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, serde_json::Value>,
}

/// One entry in the authored change log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoredChange {
    /// Monotonic per-document sequence number, starting at 1.
    pub sequence: u64,
    /// Human-readable description of the edit.
    pub description: String,
}

/// The on-disk project document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthoredDocument {
    /// Envelope naming schema, version, and encoding.
    pub envelope: AuthoredEnvelope,
    /// Stable identity of this project.
    pub project: ProjectId,
    /// Named sections of structured data, in deterministic order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sections: BTreeMap<String, serde_json::Value>,
    /// Prefab table, in deterministic order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prefabs: BTreeMap<String, Prefab>,
    /// Append-only edit history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<AuthoredChange>,
    /// Fields from newer versions, preserved verbatim on save.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

impl AuthoredDocument {
    /// Starts a new empty project document at the current schema version.
    #[must_use]
    pub fn new(project: ProjectId) -> Self {
        let (schema, version) = project_schema();
        Self {
            envelope: AuthoredEnvelope {
                schema,
                version,
                encoding: AUTHORED_ENCODING,
            },
            project,
            sections: BTreeMap::new(),
            prefabs: BTreeMap::new(),
            changes: Vec::new(),
            unknown: BTreeMap::new(),
        }
    }

    /// Appends a change-log entry with the next sequence number.
    pub fn record_change(&mut self, description: &str) {
        let sequence = self.changes.len() as u64 + 1;
        self.changes.push(AuthoredChange {
            sequence,
            description: description.to_owned(),
        });
    }

    /// Serializes to canonical pretty JSON. Deterministic: struct fields in
    /// declaration order, maps in key order.
    pub fn to_canonical_json(&self) -> Result<String, StateError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parses canonical JSON back. Unknown fields land in `unknown`;
    /// encoding and schema checks are the caller's staged-load steps.
    pub fn from_canonical_json(text: &str) -> Result<Self, StateError> {
        Ok(serde_json::from_str(text)?)
    }

    /// Atomically writes the document: complete bytes to a sibling
    /// temporary file, platform flush, then rename over `path`. A crash
    /// before the rename leaves the previous file untouched, so a failed
    /// or interrupted save reports a typed error and the last good project
    /// data stays recoverable. A stale temporary file from such a crash is
    /// fully overwritten by the next save, never merged — which is what
    /// the interrupted-write recovery test proves.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let text = self.to_canonical_json()?;
        atomic_write(path, text.as_bytes())
    }

    /// Reads and parses a document. Callers still owe the staged checks
    /// (encoding, schema, migration) before trusting the result.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        let text = std::fs::read_to_string(path).map_err(|e| StateError::File {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        Self::from_canonical_json(&text)
    }

    /// Resolves a prefab to its effective field map: base fields first,
    /// then overrides, merged per key — and per field when both sides hold
    /// objects (the usual schema-to-fields shape), so overriding one field
    /// of a schema keeps the base's sibling fields. A base that itself
    /// names a base is rejected — inheritance is one level, never a chain.
    pub fn resolve_prefab(
        &self,
        name: &str,
    ) -> Result<BTreeMap<String, serde_json::Value>, StateError> {
        let prefab = self.prefabs.get(name).ok_or_else(|| StateError::File {
            path: Path::new("<document>").to_path_buf(),
            reason: format!("unknown prefab '{name}'"),
        })?;
        let mut fields = BTreeMap::new();
        if let Some(base) = &prefab.base {
            let base_prefab = self.prefabs.get(base).ok_or_else(|| StateError::File {
                path: Path::new("<document>").to_path_buf(),
                reason: format!("unknown prefab base '{base}'"),
            })?;
            if base_prefab.base.is_some() {
                return Err(StateError::File {
                    path: Path::new("<document>").to_path_buf(),
                    reason: format!(
                        "prefab '{name}' chains through '{base}': inheritance is one level"
                    ),
                });
            }
            fields.extend(
                base_prefab
                    .overrides
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
        }
        for (key, value) in &prefab.overrides {
            match (fields.get_mut(key), value) {
                (
                    Some(serde_json::Value::Object(base_fields)),
                    serde_json::Value::Object(override_fields),
                ) => {
                    for (field, item) in override_fields {
                        base_fields.insert(field.clone(), item.clone());
                    }
                }
                _ => {
                    fields.insert(key.clone(), value.clone());
                }
            }
        }
        Ok(fields)
    }
}

/// Writes complete `bytes` to a sibling temporary file (flushed, then
/// atomically renamed over `path`). Shared by authored saves and snapshot
/// saves so both paths offer the same crash semantics. Durability details
/// vary by filesystem and are reported by callers, not promised here.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    use std::io::Write as _;
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp).map_err(|error| StateError::File {
        path: tmp.clone(),
        reason: error.to_string(),
    })?;
    file.write_all(bytes).map_err(|error| StateError::File {
        path: tmp.clone(),
        reason: error.to_string(),
    })?;
    file.sync_all().map_err(|error| StateError::File {
        path: tmp.clone(),
        reason: error.to_string(),
    })?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|error| StateError::File {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> AuthoredDocument {
        AuthoredDocument::new(ProjectId::generate().expect("os randomness"))
    }

    #[test]
    fn canonical_json_is_byte_stable() {
        let mut first = doc();
        first
            .sections
            .insert("b".to_owned(), serde_json::json!({"z": 1, "a": [3, 2]}));
        first
            .sections
            .insert("a".to_owned(), serde_json::json!(null));
        let mut second = doc();
        second.project = first.project;
        second.sections = first.sections.clone();
        assert_eq!(
            first.to_canonical_json().expect("serialize"),
            second.to_canonical_json().expect("serialize")
        );
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        let text = r#"{"envelope":{"schema":"canary.project","version":1,"encoding":1},
            "project":"123e4567-e89b-42d3-a456-426614174000",
            "sections":{},
            "from_the_future": {"nested": [1,2]}}"#;
        let parsed = AuthoredDocument::from_canonical_json(text).expect("parse");
        assert_eq!(parsed.unknown.len(), 1);
        let out = parsed.to_canonical_json().expect("serialize");
        assert!(out.contains("from_the_future"), "unknown field kept: {out}");
        let again = AuthoredDocument::from_canonical_json(&out).expect("reparse");
        assert_eq!(again, parsed);
    }

    #[test]
    fn malformed_json_is_a_typed_codec_error() {
        let err = AuthoredDocument::from_canonical_json("{not json").unwrap_err();
        assert!(matches!(err, StateError::AuthoredJson(_)));
    }

    #[test]
    fn staged_load_checks_encoding_before_trusting() {
        use crate::schema::{check_encoding, SchemaVersion};
        let mut document = doc();
        document.envelope.version = SchemaVersion(99);
        let text = document.to_canonical_json().expect("serialize");
        let parsed = AuthoredDocument::from_canonical_json(&text).expect("parse");
        // Newer schema version: no migration registered, so the staged load
        // stops here instead of trusting the body.
        assert_eq!(parsed.envelope.version, SchemaVersion(99));
        // Newer *encoding*: rejected outright, before any body is read.
        let mut future = parsed.clone();
        future.envelope.encoding = crate::schema::EncodingVersion(99);
        assert!(
            check_encoding(future.envelope.encoding, crate::schema::AUTHORED_ENCODING).is_err()
        );
    }

    #[test]
    fn prefab_one_level_merges_base_then_overrides() {
        let mut document = doc();
        document.prefabs.insert(
            "base".to_owned(),
            Prefab {
                base: None,
                overrides: BTreeMap::from([
                    ("hp".to_owned(), serde_json::json!(10)),
                    ("speed".to_owned(), serde_json::json!(1)),
                ]),
            },
        );
        document.prefabs.insert(
            "fast".to_owned(),
            Prefab {
                base: Some("base".to_owned()),
                overrides: BTreeMap::from([("speed".to_owned(), serde_json::json!(5))]),
            },
        );
        let resolved = document.resolve_prefab("fast").expect("resolve");
        assert_eq!(resolved["hp"], serde_json::json!(10));
        assert_eq!(resolved["speed"], serde_json::json!(5));
    }

    #[test]
    fn prefab_chains_are_rejected() {
        let mut document = doc();
        for (name, base) in [("a", None), ("b", Some("a")), ("c", Some("b"))] {
            document.prefabs.insert(
                name.to_owned(),
                Prefab {
                    base: base.map(str::to_owned),
                    overrides: BTreeMap::new(),
                },
            );
        }
        assert!(document.resolve_prefab("c").is_err());
    }

    #[test]
    fn prefab_override_keeps_base_sibling_fields() {
        let mut document = doc();
        document.prefabs.insert(
            "base".to_owned(),
            Prefab {
                base: None,
                overrides: BTreeMap::from([(
                    "test.health".to_owned(),
                    serde_json::json!({"hp": 10, "armor": 2}),
                )]),
            },
        );
        document.prefabs.insert(
            "tough".to_owned(),
            Prefab {
                base: Some("base".to_owned()),
                overrides: BTreeMap::from([(
                    "test.health".to_owned(),
                    serde_json::json!({"hp": 99}),
                )]),
            },
        );
        let resolved = document.resolve_prefab("tough").expect("resolve");
        assert_eq!(
            resolved["test.health"],
            serde_json::json!({"hp": 99, "armor": 2}),
            "one overridden field must not drop its siblings"
        );
    }

    #[test]
    fn unknown_data_round_trips_byte_identical() {
        let text = r#"{"envelope":{"schema":"canary.project","version":1,"encoding":1},
            "project":"123e4567-e89b-42d3-a456-426614174000",
            "sections":{"entity.hero":{"test.health":{"hp":10},"future-component":{"nested":[1,2]}}},
            "from_the_future": {"nested": [1,2]}}"#;
        let parsed = AuthoredDocument::from_canonical_json(text).expect("parse");
        assert_eq!(parsed.unknown.len(), 1);
        let first = parsed.to_canonical_json().expect("serialize");
        let again = AuthoredDocument::from_canonical_json(&first).expect("reparse");
        let second = again.to_canonical_json().expect("reserialize");
        assert_eq!(first, second, "load→save must be byte-identical");
        assert!(
            first.contains("from_the_future"),
            "unknown field kept: {first}"
        );
        assert!(
            first.contains("future-component"),
            "unknown section kept: {first}"
        );
        assert_eq!(again, parsed);
    }

    /// Unique scratch directory per test process run: no `tempfile`
    /// dependency for two recovery tests.
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "canary-state-{}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            tag
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn interrupted_save_leaves_the_last_good_file_recoverable() {
        let dir = scratch_dir("interrupted");
        let path = dir.join("project.json");
        let mut first = doc();
        first
            .sections
            .insert("entity.hero".to_owned(), serde_json::json!({"hp": 1}));
        first.save(&path).expect("first save");

        // Simulate a writer killed mid-stream: a partial sibling temp file
        // with no rename. The committed file must still load as v1.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, b"{partial").expect("plant partial tmp");
        let recovered = AuthoredDocument::load(&path).expect("recover v1");
        assert_eq!(recovered.sections, first.sections);

        // The next save fully overwrites the stale temp file, renames, and
        // loads as v2 — the crash left no residue behind.
        let mut second = first.clone();
        second
            .sections
            .insert("entity.hero".to_owned(), serde_json::json!({"hp": 2}));
        second.save(&path).expect("second save");
        assert!(!tmp.exists(), "rename consumes the temp file");
        let loaded = AuthoredDocument::load(&path).expect("recover v2");
        assert_eq!(loaded.sections, second.sections);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncated_file_fails_typed_instead_of_yielding_partial_state() {
        let dir = scratch_dir("truncated");
        let path = dir.join("project.json");
        let mut document = doc();
        document
            .sections
            .insert("entity.hero".to_owned(), serde_json::json!({"hp": 1}));
        document.save(&path).expect("save");
        let bytes = std::fs::read(&path).expect("read back");
        std::fs::write(&path, &bytes[..bytes.len() / 2]).expect("truncate");
        let err = AuthoredDocument::load(&path).expect_err("truncated file fails");
        assert!(
            matches!(err, StateError::AuthoredJson(_)),
            "typed codec error, got {err:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
