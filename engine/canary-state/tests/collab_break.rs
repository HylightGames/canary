//! State-level break-it for the shared durable seams backing `.16`.
//!
//! Same-stem collisions across the three real temp suffixes, plus
//! revision/history horizon edges. No production code is touched.

use std::collections::BTreeMap;

use canary_state::{
    atomic_write, AuthoredDocument, DocumentHistory, LogicalEntityId, ObjectRevision,
    OperationSequence, PendingAccept, ProjectId, SchemaId, SchemaVersion, TRANSFORM_SCHEMA_KEY,
    TRANSFORM_SCHEMA_VERSION,
};

fn scratch_dir(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "canary-state-break-{}-{}-{tag}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn accept_hero(history: &mut DocumentHistory, index: u64) {
    history.accept(PendingAccept {
        actor: 1,
        client_op_id: format!("op-{index}"),
        target: LogicalEntityId::from_local("hero").expect("valid target"),
        expected_revision: ObjectRevision(index),
        schema: SchemaId::new(TRANSFORM_SCHEMA_KEY),
        schema_version: SchemaVersion(TRANSFORM_SCHEMA_VERSION),
        payload: serde_json::json!({}),
    });
}

/// All three durable writers (authored project, snapshot bytes, permission
/// store) side by side on the SAME stem: no sibling temp name is ever
/// shared, and a stale temp from a killed writer is overwritten, never
/// merged into a neighbor.
#[test]
fn all_three_durable_suffixes_keep_same_stem_files_apart() {
    let dir = scratch_dir("triple-suffix");
    // Same stem, three real files: project JSON, snapshot bytes, and the
    // permission store beside them (its real suffix is `.permissions-tmp`).
    let project = dir.join("save.json");
    let snapshot = dir.join("save.bin");
    let permissions = dir.join("save-permissions.json");
    atomic_write(&project, b"{\"v\":1}", ".tmp").expect("project save");
    atomic_write(&snapshot, b"\x00\x01", ".tmp").expect("snapshot save");
    atomic_write(&permissions, b"{\"epoch\":1}", ".permissions-tmp").expect("permission save");
    assert_eq!(std::fs::read(&project).expect("read project"), b"{\"v\":1}");
    assert_eq!(
        std::fs::read(&snapshot).expect("read snapshot"),
        b"\x00\x01"
    );
    assert_eq!(
        std::fs::read(&permissions).expect("read permissions"),
        b"{\"epoch\":1}"
    );

    // Same-stem trap the old extension-replacing form failed: `report.json`
    // vs `report.toml` must land in distinct temps and read back intact.
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

    // A stale temp from a killed writer is fully overwritten by the next
    // save of the same file and never leaks into the neighbor.
    let stale = dir.join("save.json.tmp");
    std::fs::write(&stale, b"{partial").expect("plant stale temp");
    atomic_write(&project, b"{\"v\":2}", ".tmp").expect("overwrite save");
    assert!(!stale.exists(), "rename consumes the temp file");
    assert_eq!(std::fs::read(&project).expect("read again"), b"{\"v\":2}");
    assert_eq!(
        std::fs::read(&snapshot).expect("neighbor untouched"),
        b"\x00\x01"
    );

    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("list dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "report.json",
            "report.toml",
            "save-permissions.json",
            "save.bin",
            "save.json",
        ],
        "no stray temp file may survive: {names:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Checkpoint horizon is inclusive: the cursor AT the horizon replays its
/// contiguous tail, one behind it takes the snapshot path.
#[test]
fn checkpoint_horizon_boundary_is_inclusive_not_off_by_one() {
    let mut history = DocumentHistory::genesis();
    for index in 0..4 {
        accept_hero(&mut history, index);
    }
    let checkpoint = history.trim_retained(2).expect("trim to two");
    assert_eq!(history.accepted.len(), 2);
    // Exactly at the horizon: covered, both retained records after it.
    let tail = history
        .tail_since(checkpoint.last_retained)
        .expect("horizon cursor is covered");
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].sequence, OperationSequence(3));
    // One behind: snapshot path.
    let behind = OperationSequence(checkpoint.last_retained.0 - 1);
    assert_eq!(
        history.tail_since(behind),
        Err(canary_state::TailGap::BehindCheckpoint)
    );
    // At the tip: empty tail, still covered.
    assert!(history
        .tail_since(OperationSequence(4))
        .expect("tip cursor")
        .is_empty());
    // One past the tip: forged cursor, never an empty accept.
    assert_eq!(
        history.tail_since(OperationSequence(5)),
        Err(canary_state::TailGap::AheadOfHistory)
    );
}

/// Trim clamps a zero max to one: history never trims itself empty, and a
/// trim that evicts nothing returns no checkpoint.
#[test]
fn trim_clamps_zero_max_to_one_and_reports_nothing_when_idle() {
    let mut history = DocumentHistory::genesis();
    for index in 0..3 {
        accept_hero(&mut history, index);
    }
    let checkpoint = history.trim_retained(0).expect("clamped trim evicts");
    assert_eq!(history.accepted.len(), 1, "max 0 clamps to 1");
    assert_eq!(history.accepted[0].sequence, OperationSequence(3));
    assert_eq!(checkpoint.last_retained, OperationSequence(2));
    assert!(history.trim_retained(128).is_none());
}

/// History survives a save/load round trip with the checkpoint and the
/// lifetime evicted count intact, and legacy files still open at genesis.
#[test]
fn history_checkpoint_and_evicted_count_survive_save_load() {
    let mut document = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
    for index in 0..3 {
        document.history.accept(PendingAccept {
            actor: 2,
            client_op_id: format!("op-{index}"),
            target: LogicalEntityId::from_local("hero").expect("valid"),
            expected_revision: ObjectRevision(index),
            schema: SchemaId::new(TRANSFORM_SCHEMA_KEY),
            schema_version: SchemaVersion(TRANSFORM_SCHEMA_VERSION),
            payload: serde_json::json!({"n": index}),
        });
    }
    document.history.trim_retained(1);
    assert_eq!(document.history.evicted, 2);
    let text = document.to_canonical_json().expect("serialize");
    assert!(text.contains("checkpoint"), "checkpoint is durable");
    let again = AuthoredDocument::from_canonical_json(&text).expect("reparse");
    assert_eq!(again.history, document.history);
    assert_eq!(again.history.evicted, 2);

    // Prefab-adjacent state edge: an entity section holding a non-object
    // is an error at the veto gate, not a silent allow.
    let mut odd = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
    odd.sections
        .insert("entity.flat".to_owned(), serde_json::json!([1, 2, 3]));
    let flat = LogicalEntityId::from_local("flat").expect("valid");
    assert!(odd.entity_section_exists(&flat));
    assert!(odd.transform_override_allowed(&flat).is_err());
    // A prefab map the resolve rules reject still vetoes closed.
    let mut chained = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
    chained.prefabs.insert(
        "root".to_owned(),
        canary_state::Prefab {
            base: None,
            overrides: BTreeMap::from([(TRANSFORM_SCHEMA_KEY.to_owned(), serde_json::json!({}))]),
        },
    );
    chained.prefabs.insert(
        "mid".to_owned(),
        canary_state::Prefab {
            base: Some("root".to_owned()),
            overrides: BTreeMap::new(),
        },
    );
    chained.prefabs.insert(
        "leaf".to_owned(),
        canary_state::Prefab {
            base: Some("mid".to_owned()),
            overrides: BTreeMap::new(),
        },
    );
    chained.sections.insert(
        "entity.chained".to_owned(),
        serde_json::json!({"prefab": "leaf"}),
    );
    let id = LogicalEntityId::from_local("chained").expect("valid");
    assert!(!chained.transform_override_allowed(&id).expect("gate"));
}
