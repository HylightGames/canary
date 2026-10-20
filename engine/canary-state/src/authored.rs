//! Authored project files: canonical pretty JSON, deterministic ordering.
//!
//! An [`AuthoredDocument`] is the on-disk project: an [`AuthoredEnvelope`],
//! the [`ProjectId`], named sections of structured
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

use std::collections::hash_map::RandomState;
use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// Accepted-operation history for live collaboration (`.16` WP1).
    ///
    /// Additive and defaulted: documents that never saw collaboration
    /// carry genesis counters and serialize without this section, so the
    /// envelope stays `canary.project` version 1, encoding 1. See
    /// [`DocumentHistory`](crate::revisions::DocumentHistory).
    #[serde(
        default,
        skip_serializing_if = "crate::revisions::DocumentHistory::is_empty"
    )]
    pub history: crate::revisions::DocumentHistory,
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
            history: crate::revisions::DocumentHistory::genesis(),
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
    /// ignored by the next save — which draws a fresh unpredictable temp
    /// name — never merged, which is what the interrupted-write recovery
    /// test proves.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let text = self.to_canonical_json()?;
        atomic_write(path, text.as_bytes(), AUTHORED_TEMP_SUFFIX)
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

/// Temp suffix for authored project saves (see [`atomic_write`]).
const AUTHORED_TEMP_SUFFIX: &str = ".tmp";

/// Process-wide counter for temp names. It stays in the name only as a
/// debug hint: the 128-bit nonce, not this value, is what makes a name
/// unguessable.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Builds one candidate sibling temp name for [`atomic_write`]:
/// `<suffix>.<pid>.<counter>.<32 hex digits>`, where the trailing digits
/// are a fresh unpredictable 128-bit nonce.
fn temp_name_suffix(temp_suffix: &str, counter: u64) -> String {
    let nonce = unpredictable_temp_nonce(counter);
    format!(
        "{temp_suffix}.{}.{counter}.{nonce:032x}",
        std::process::id()
    )
}

/// Draws one 128-bit nonce for a temp name.
///
/// `RandomState` carries SipHash keys seeded from OS randomness once per
/// thread; hashing the pid, counter, and current time with those keys is
/// a PRF whose output an attacker cannot derive from earlier temp names
/// or from the public debug hints. Two domain-separated hashes supply the
/// full 128 bits.
fn unpredictable_temp_nonce(counter: u64) -> u128 {
    let pid = u64::from(std::process::id());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let state = RandomState::new();

    let mut low = state.build_hasher();
    low.write_u64(pid);
    low.write_u64(counter);
    low.write_u128(nanos);
    let low = low.finish();

    let mut high = state.build_hasher();
    high.write_u8(0x80); // domain separation from the low half
    high.write_u64(pid);
    high.write_u64(counter);
    high.write_u128(nanos);
    let high = high.finish();

    (u128::from(high) << 64) | u128::from(low)
}

/// Writes complete `bytes` to a sibling temporary file (flushed, then
/// atomically renamed over `path`). Shared by authored saves, snapshot
/// saves, and the collaboration permission store so every durable path
/// offers the same crash semantics: a crash before the rename leaves the
/// previous file untouched.
///
/// `temp_suffix` is appended to the file name (never substituted for the
/// extension): two different files can never share a sibling temp name,
/// which the old extension-replacing form could not promise for files
/// differing only by extension (`report.json` vs `report.toml`). Callers
/// persisting different files alongside each other pass distinct
/// suffixes.
///
/// Durable-commit semantics (F-19):
/// - The temp name is unique per attempt and unpredictable:
///   `<suffix>.<pid>.<counter>.<32 hex digits>` where the hex is a fresh
///   128-bit nonce drawn from OS-seeded SipHash keys (`RandomState`) mixed
///   with the pid, counter, and current time. The pid and counter are
///   debug hints only. Two concurrent savers of one file never share a
///   temp path (the old fixed name let the second `File::create` truncate
///   the first's in-flight bytes), and a local attacker who can write the
///   directory cannot enumerate the next name: pre-creating the old
///   predictable names does not exhaust the retry loop, so planting files
///   cannot deny a save. A symlink pre-planted at a temp path fails
///   closed: the temp is created with `create_new`, which refuses any
///   existing path instead of following it.
/// - After the rename the parent directory is fsynced (Unix), so the
///   rename itself survives a crash — a file `sync_all` alone does not
///   promise that on POSIX filesystems.
///
/// Commit-then-report semantics: the rename above commits the bytes, and
/// the parent-directory sync after it only *verifies* durability. A
/// failure from here on reports `Err` for bytes that are already live
/// under `path` — "committed but unverified", never "nothing was
/// written". Callers must treat such an `Err` as retry-idempotent
/// (re-saving the same bytes converges; the temp name is unique per call
/// so the retry cannot collide) rather than as proof the save failed.
/// The decision point logs through [`committed_but_unverified`] (debug
/// builds only; release stays silent). Pinned by
/// `dir_sync_failure_after_rename_reports_err_with_committed_bytes`.
pub fn atomic_write(path: &Path, bytes: &[u8], temp_suffix: &str) -> Result<(), StateError> {
    use std::io::Write as _;

    #[cfg(unix)]
    let parent = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    // Bounded collision retry: every attempt draws a fresh unpredictable
    // 128-bit nonce, so `AlreadyExists` means something already sits at
    // that exact name (a leftover temp from a killed writer, or an
    // attacker who guessed 128 bits) and the next attempt simply picks a
    // new name. No fixed set of names can be pre-created to exhaust this.
    for _ in 0..8 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let unique = temp_name_suffix(temp_suffix, counter);
        let mut name = path.as_os_str().to_owned();
        name.push(unique.as_str());
        let tmp = PathBuf::from(name);
        // `create_new` refuses any existing path (including a pre-planted
        // symlink) instead of truncating or following it: concurrent
        // savers and symlink plants fail this attempt rather than
        // corrupting each other.
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(StateError::File {
                    path: tmp,
                    reason: error.to_string(),
                });
            }
        };
        let outcome: Result<(), StateError> = (|| {
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
            })?;
            // Persist the rename itself: without a directory fsync a crash
            // may lose the directory entry update even though the file
            // bytes were synced. Either failure below happens AFTER the
            // rename, so the bytes are already live — see the
            // commit-then-report note on this function.
            #[cfg(unix)]
            {
                let dir = std::fs::File::open(&parent).map_err(|error| {
                    committed_but_unverified(path, &error);
                    StateError::File {
                        path: parent.clone(),
                        reason: error.to_string(),
                    }
                })?;
                dir.sync_all().map_err(|error| {
                    committed_but_unverified(path, &error);
                    StateError::File {
                        path: parent.clone(),
                        reason: error.to_string(),
                    }
                })?;
            }
            Ok(())
        })();
        if outcome.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return outcome;
        }
        return Ok(());
    }
    Err(StateError::File {
        path: path.to_path_buf(),
        reason: "atomic_write: temp-name collision after 8 attempts".to_string(),
    })
}

/// Debug-only notice for the commit-then-report decision point in
/// [`atomic_write`]: the rename already committed `path`'s bytes and the
/// post-rename directory sync failed, so the accompanying `Err` means
/// "committed but unverified" and a retry of the same bytes is
/// idempotent. This leaf crate carries no logging facade (adding one
/// would be a dependency decision, not a durability fix), so the notice
/// goes to stderr in debug builds only; release builds stay silent.
#[cfg(unix)]
fn committed_but_unverified(path: &Path, error: &std::io::Error) {
    // Function parameters (unlike locals) never trip `unused_variables`,
    // so the release build needs no placeholder for the debug-only uses.
    #[cfg(debug_assertions)]
    std::eprintln!(
        "canary-state: atomic_write committed {} but the post-rename directory sync failed \
         ({error}); bytes may be live, retry is idempotent",
        path.display()
    );
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
        // Temp names are unpredictable per call
        // (`project.json.tmp.<pid>.<counter>.<nonce>`), so a stale temp
        // from a killed writer is simply never read back; the old
        // fixed-suffix name below stands in for any such leftover.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, b"{partial").expect("plant partial tmp");
        let recovered = AuthoredDocument::load(&path).expect("recover v1");
        assert_eq!(recovered.sections, first.sections);

        // The next save writes a fresh unique temp and renames it; the
        // committed file loads as v2 and the unique temp leaves no
        // residue behind (only the planted legacy leftover, which no save
        // ever reads, remains).
        let mut second = first.clone();
        second
            .sections
            .insert("entity.hero".to_owned(), serde_json::json!({"hp": 2}));
        second.save(&path).expect("second save");
        // Exact sorted listing: the old `.all()` allowlist accepted a dir
        // missing its commit (or hiding extra files), so a subset check
        // could pass while proving nothing — only this equality proves the
        // unique temp left no residue beyond the planted legacy leftover.
        let mut residue: Vec<String> = std::fs::read_dir(&dir)
            .expect("list dir")
            .map(|entry| {
                entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        residue.sort();
        assert_eq!(
            residue,
            vec!["project.json".to_owned(), "project.json.tmp".to_owned()],
            "the dir must hold exactly the commit plus the planted legacy leftover"
        );
        let loaded = AuthoredDocument::load(&path).expect("recover v2");
        assert_eq!(loaded.sections, second.sections);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn appended_suffix_keeps_same_stem_files_apart() {
        // Proof for the shared-helper unification: the old
        // extension-replacing form aimed both of these files at the same
        // `report.tmp`. The appended form must keep them apart, and a
        // stale temp from a killed writer must be ignored, never merged.
        let dir = scratch_dir("suffix");
        let first = dir.join("report.json");
        let second = dir.join("report.toml");
        atomic_write(&first, b"one", ".tmp").expect("first save");
        atomic_write(&second, b"two", ".tmp").expect("second save");
        assert_eq!(std::fs::read(&first).expect("read first"), b"one");
        assert_eq!(std::fs::read(&second).expect("read second"), b"two");
        assert!(
            !dir.join("report.tmp").exists(),
            "no extension-replaced temp may ever appear"
        );

        // A stale fixed-suffix temp from a killed writer is never read or
        // merged: the next save writes its own unique temp, and the
        // committed file holds exactly the new bytes.
        let stale = dir.join("report.json.tmp");
        std::fs::write(&stale, b"{partial").expect("plant stale tmp");
        atomic_write(&first, b"uno", ".tmp").expect("overwrite save");
        assert_eq!(std::fs::read(&first).expect("read again"), b"uno");
        assert_eq!(std::fs::read(&second).expect("neighbor untouched"), b"two");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn concurrent_savers_never_share_a_temp_path() {
        // Proof for unique temp names: N threads saving one file
        // concurrently must all succeed, and the committed file must hold
        // exactly one writer's complete bytes (never a truncation splice
        // of two in-flight writes, which the old fixed temp name allowed).
        let dir = scratch_dir("concurrent");
        let path = dir.join("shared.json");
        let payloads: Vec<Vec<u8>> = (0..8u8)
            .map(|i| format!("{{\"writer\":{i}}}").into_bytes())
            .collect();
        std::thread::scope(|scope| {
            for payload in &payloads {
                scope.spawn(|| {
                    atomic_write(&path, payload, ".tmp").expect("concurrent save");
                });
            }
        });
        let committed = std::fs::read(&path).expect("read committed");
        assert!(
            payloads.contains(&committed),
            "committed bytes must be one complete write: {committed:?}"
        );
        let residue: Vec<_> = std::fs::read_dir(&dir)
            .expect("list dir")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        assert_eq!(
            residue,
            vec![std::ffi::OsString::from("shared.json")],
            "unique temps leave no residue: {residue:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn temp_names_are_unpredictable_not_a_pid_counter_sequence() {
        // Regression proof for the predictable-name DoS: the old name space
        // was `<suffix>.<pid>.<counter>` with counters `0..8` per process,
        // fully enumerable by a local attacker before a save. Every attempt
        // must now carry a fresh 128-bit hex nonce; pid and counter survive
        // only as debug hints, never as the deciding component.
        let pid_prefix = format!(".tmp.{}.", std::process::id());
        let mut names = std::collections::BTreeSet::new();
        let mut nonces = std::collections::BTreeSet::new();
        for counter in 0..64 {
            let name = temp_name_suffix(".tmp", counter);
            let hints = name
                .strip_prefix(&pid_prefix)
                .unwrap_or_else(|| panic!("pid debug hint missing from {name}"));
            let (counter_hint, nonce) = hints
                .split_once('.')
                .unwrap_or_else(|| panic!("counter/nonce split missing from {name}"));
            assert_eq!(counter_hint, counter.to_string());
            assert_eq!(nonce.len(), 32, "128-bit nonce is 32 hex digits: {name}");
            assert!(
                nonce.chars().all(|c| c.is_ascii_hexdigit()),
                "nonce must be hex: {name}"
            );
            assert!(
                nonces.insert(nonce.to_owned()),
                "two calls produced the same nonce: {name}"
            );
            assert!(names.insert(name), "two calls produced the same temp name");
        }
        assert_eq!(names.len(), 64, "every generated temp name is distinct");
        assert!(
            !names.contains(&format!(".tmp.{}.0", std::process::id())),
            "no name may be the old pid+counter-only shape"
        );

        // The nonce is fresh per attempt, not a function of the debug
        // hints: the same counter cannot reproduce the same name.
        assert_ne!(
            temp_name_suffix(".tmp", 7),
            temp_name_suffix(".tmp", 7),
            "a second call must not reproduce the first name"
        );
    }

    #[test]
    fn preplanted_legacy_symlink_and_old_style_names_do_not_block_saves() {
        // Threat model: a local attacker who can write the target directory
        // pre-creates every temp name the writer could pick, exhausting the
        // bounded retry loop and forcing a hard failure (DoS). Under the old
        // predictable scheme those names were enumerable up front:
        // `<suffix>.<pid>.<counter>` for counter 0..8. The unpredictable
        // nonce makes each attempt land elsewhere, so the save must succeed
        // and both plants must stay byte-identical (never followed, never
        // consumed).
        let dir = scratch_dir("preplanted-temps");
        let path = dir.join("project.json");
        let legacy = dir.join("project.json.tmp");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/definitely/not/here", &legacy).expect("plant symlink");
        #[cfg(not(unix))]
        std::fs::write(&legacy, b"{partial").expect("plant legacy tmp");

        let planted: Vec<std::path::PathBuf> = (0..8)
            .map(|counter| {
                let predicted = dir.join(format!(
                    "project.json.tmp.{}.{}",
                    std::process::id(),
                    counter
                ));
                std::fs::write(&predicted, b"attacker-owned").expect("plant predicted temp");
                predicted
            })
            .collect();

        atomic_write(&path, b"{\"ok\":true}", ".tmp")
            .expect("pre-planted predictable temps must not block the save");
        assert_eq!(
            std::fs::read(&path).expect("read committed"),
            b"{\"ok\":true}"
        );
        #[cfg(unix)]
        assert!(
            std::fs::symlink_metadata(&legacy)
                .expect("legacy metadata")
                .file_type()
                .is_symlink(),
            "the legacy temp path must stay the planted symlink"
        );
        for predicted in &planted {
            assert_eq!(
                std::fs::read(predicted).expect("planted temp untouched"),
                b"attacker-owned",
                "a pre-planted predictable temp must be ignored, not consumed"
            );
        }
        let mut residue: Vec<String> = std::fs::read_dir(&dir)
            .expect("list dir")
            .map(|entry| {
                entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        residue.sort();
        let mut expected: Vec<String> = std::iter::once("project.json".to_owned())
            .chain(
                (0..8).map(|counter| format!("project.json.tmp.{}.{counter}", std::process::id())),
            )
            .chain(std::iter::once("project.json.tmp".to_owned()))
            .collect();
        expected.sort();
        assert_eq!(
            residue, expected,
            "the dir must hold exactly the commit plus the planted leftovers"
        );
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

    #[cfg(unix)]
    #[test]
    fn dir_sync_failure_after_rename_reports_err_with_committed_bytes() {
        use std::os::unix::fs::PermissionsExt;

        // Proof of the commit-then-report ambiguity: the rename commits the
        // new bytes, then the parent-directory fsync fails, so the caller
        // sees `Err` for bytes that are already live. Retry is idempotent —
        // the same bytes save cleanly once the directory is readable again.
        let dir = scratch_dir("dirsync-ambiguity");
        let path = dir.join("project.json");
        let mut first = doc();
        first
            .sections
            .insert("entity.hero".to_owned(), serde_json::json!({"hp": 1}));
        first.save(&path).expect("first save");
        let second_text = {
            let mut second = first.clone();
            second
                .sections
                .insert("entity.hero".to_owned(), serde_json::json!({"hp": 2}));
            second.to_canonical_json().expect("serialize")
        };

        // Write+execute without read: temp creation and the rename still
        // succeed, but opening the parent for its post-rename fsync fails.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300)).expect("chmod 300");
        let result = atomic_write(&path, second_text.as_bytes(), ".tmp");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod back");

        assert!(
            result.is_err(),
            "parent dir-sync failure must surface, got: {result:?}"
        );
        assert_eq!(
            std::fs::read(&path).expect("read committed"),
            second_text.as_bytes(),
            "the rename committed the new bytes before the dir-sync failed: \
             Err does NOT mean uncommitted"
        );
        // Retry is idempotent: the same bytes save cleanly now.
        atomic_write(&path, second_text.as_bytes(), ".tmp").expect("idempotent retry");
        assert_eq!(
            std::fs::read(&path).expect("read again"),
            second_text.as_bytes()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exact_listing_rejects_a_dir_missing_its_commit() {
        // Regression guard for the residue-subset weakness the interrupted
        // and concurrent save tests used to share: a directory holding ONLY
        // the planted legacy leftover — committed file deleted — satisfied
        // the old `.all()` allowlist. The exact sorted listings those tests
        // now assert must reject this directory.
        let dir = scratch_dir("subset-weakness");
        std::fs::write(dir.join("project.json.tmp"), b"{partial").expect("plant legacy tmp");

        let mut residue: Vec<String> = std::fs::read_dir(&dir)
            .expect("list dir")
            .map(|entry| {
                entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        residue.sort();
        assert_ne!(
            residue,
            vec!["project.json".to_owned(), "project.json.tmp".to_owned()],
            "exact listing must reject a dir missing its commit: {residue:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
