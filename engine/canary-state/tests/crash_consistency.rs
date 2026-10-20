//! Crash-consistency chaos sweep over the atomic-write path (findings only).
//!
//! Each test pre-plants the leftover a crash between two durable operations
//! would leave behind (stale legacy temp, half-written unique temp, orphaned
//! unique temp with valid bytes), then drives the next save/load and asserts
//! the commit contract: the committed file holds exactly one complete
//! generation (never torn or partial bytes), loads recover to the last good
//! or the new generation, and the directory holds no residue beyond the
//! documented leftovers (the commit plus the pre-planted files).

use std::path::{Path, PathBuf};

use canary_state::{atomic_write, AuthoredDocument, ProjectId, StateError};

/// Unique scratch directory per test: no `tempfile` dependency.
fn scratch_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "canary-chaos-state-{}-{}-{tag}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn doc_with_hp(hp: i64) -> AuthoredDocument {
    let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
    doc.sections
        .insert("entity.hero".to_owned(), serde_json::json!({"hp": hp}));
    doc
}

fn sorted_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
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
    names
}

/// Crash between temp-sync and rename: a stale legacy temp (`project.json.tmp`)
/// from a killed writer sits beside the commit. The next save must overwrite
/// (never merge) it and commit exactly the new bytes.
#[test]
fn stale_legacy_tmp_beside_commit_never_merges() {
    let dir = scratch_dir("legacy-tmp");
    let path = dir.join("project.json");
    let first = doc_with_hp(1);
    first.save(&path).expect("first save");

    std::fs::write(dir.join("project.json.tmp"), b"{partial").expect("plant stale legacy tmp");
    let second = doc_with_hp(2);
    second.save(&path).expect("second save");

    let loaded = AuthoredDocument::load(&path).expect("recover");
    assert_eq!(
        loaded.sections, second.sections,
        "commit must hold exactly the new generation, never a merge with the stale tmp"
    );
    assert_eq!(
        sorted_names(&dir),
        vec!["project.json".to_owned(), "project.json.tmp".to_owned()],
        "no residue beyond the commit plus the documented legacy leftover"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Crash mid-write of a unique temp: a half-written orphan named like a real
/// unique temp (`project.json.tmp.<pid>.<counter>.<nonce>`) sits beside the
/// commit. The next save must take a different unique name, commit exactly,
/// and leave the orphan byte-identical (ignored, never consumed).
#[test]
fn half_written_unique_tmp_is_ignored_not_consumed() {
    let dir = scratch_dir("half-unique");
    let path = dir.join("project.json");
    let first = doc_with_hp(1);
    first.save(&path).expect("first save");

    // Unique-style orphan with torn bytes; the counter value is chosen far
    // above any value this process will reach in one test.
    let orphan = dir.join(format!("project.json.tmp.{}.999991", std::process::id()));
    std::fs::write(&orphan, b"{\"sections\":{\"half\"").expect("plant half-written unique tmp");
    let second = doc_with_hp(2);
    second.save(&path).expect("second save");

    let loaded = AuthoredDocument::load(&path).expect("recover");
    assert_eq!(
        loaded.sections, second.sections,
        "commit must be exactly the new generation, never spliced with orphan bytes"
    );
    assert_eq!(
        std::fs::read(&orphan).expect("orphan still there"),
        b"{\"sections\":{\"half\"",
        "orphan unique tmp is ignored, never consumed or truncated"
    );
    let orphan_name = orphan
        .file_name()
        .expect("orphan name")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        sorted_names(&dir),
        vec!["project.json".to_owned(), orphan_name],
        "no residue beyond the commit plus the planted orphan"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Crash after a full temp write but before rename: the orphan holds COMPLETE
/// valid bytes of an older generation. The next save must still commit exactly
/// the new generation — the valid orphan must not be mistaken for the commit
/// or merged into it.
#[test]
fn orphaned_unique_tmp_with_valid_bytes_never_merges() {
    let dir = scratch_dir("valid-orphan");
    let path = dir.join("project.json");
    let first = doc_with_hp(1);
    first.save(&path).expect("first save");

    let orphan = dir.join(format!("project.json.tmp.{}.999992", std::process::id()));
    let first_text = first.to_canonical_json().expect("serialize v1");
    std::fs::write(&orphan, first_text.as_bytes()).expect("plant valid orphan");
    let second = doc_with_hp(2);
    second.save(&path).expect("second save");

    let loaded = AuthoredDocument::load(&path).expect("recover");
    assert_eq!(
        loaded.sections, second.sections,
        "commit must be v2 even though a valid v1 orphan sits beside it"
    );
    assert_eq!(
        std::fs::read(&orphan).expect("orphan still there"),
        first_text.as_bytes(),
        "valid orphan is left untouched, never promoted to the commit"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Generation-chain invariant across three saves with a planted leftover
/// between every pair: after each step the load equals exactly one complete
/// generation (last good or new), never torn bytes. A directly truncated
/// commit file additionally fails typed instead of yielding partial state.
#[test]
fn generation_chain_never_yields_torn_bytes() {
    let dir = scratch_dir("chain");
    let path = dir.join("project.json");
    let mut previous: Option<AuthoredDocument> = None;

    for (step, hp) in [1_i64, 2, 3].iter().enumerate() {
        let doc = doc_with_hp(*hp);
        // Plant a fresh leftover before every save except the first: the
        // crash could have happened between any pair of operations.
        if step > 0 {
            std::fs::write(dir.join("project.json.tmp"), b"{torn").expect("plant legacy tmp");
            std::fs::write(
                dir.join(format!(
                    "project.json.tmp.{}.99999{}",
                    std::process::id(),
                    step
                )),
                b"{\"torn\"",
            )
            .expect("plant unique orphan");
        }
        atomic_write(
            &path,
            doc.to_canonical_json().expect("serialize").as_bytes(),
            ".tmp",
        )
        .expect("save");
        let loaded = AuthoredDocument::load(&path).expect("recover");
        assert_eq!(
            loaded.sections, doc.sections,
            "step {step}: load must equal exactly generation {hp}, never torn bytes"
        );
        if let Some(prev) = &previous {
            assert_ne!(
                loaded.sections, prev.sections,
                "step {step}: the new generation must actually have committed"
            );
        }
        previous = Some(doc);
    }

    // Every residue entry is a documented leftover: the commit, the legacy
    // fixed-suffix tmp, or a unique-style tmp. Nothing else may appear.
    for name in sorted_names(&dir) {
        let documented = name == "project.json"
            || name == "project.json.tmp"
            || name.starts_with("project.json.tmp.");
        assert!(documented, "undocumented residue beside the commit: {name}");
    }

    // A commit file truncated in place (disk fault, not a temp leftover)
    // fails typed — the loader never yields partial state.
    let bytes = std::fs::read(&path).expect("read commit");
    std::fs::write(&path, &bytes[..bytes.len() / 2]).expect("truncate commit");
    let err = AuthoredDocument::load(&path).expect_err("truncated commit fails");
    assert!(
        matches!(err, StateError::AuthoredJson(_)),
        "typed codec error for torn bytes, got {err:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
