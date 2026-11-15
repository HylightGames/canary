//! Regression coverage from the 2026-10-24 v0.1.0 final audit (finding `003`).
//!
//! `SnapshotValue` is recursive (`List`/`Map` nest `SnapshotValue`) and is
//! decoded with `postcard`, which has no recursion limit. Two bytes of input
//! buy one level of nesting, so a hostile snapshot/save file can drive the
//! decoder into unbounded recursion. The reproduction runs the decode in a
//! child process so a stack overflow shows up as a failed assertion instead
//! of killing the test harness.
//!
//! The bound is [`canary_state::MAX_SNAPSHOT_VALUE_DEPTH`] levels (counting
//! the value itself); anything deeper is a typed
//! [`canary_state::StateError::NestingTooDeep`] error.

use std::collections::BTreeMap;
use std::process::Command;

use canary_state::{
    decode_snapshot, encode_snapshot, AuthoredDocument, SchemaId, SchemaVersion, SnapshotProfile,
    SnapshotRecord, SnapshotValue, StateError,
};

const CHILD_ENV: &str = "AUDIT_NESTED_SNAPSHOT_DEPTH";
const TEST_NAME: &str = "deeply_nested_snapshot_value_is_an_error_not_a_stack_overflow";

/// postcard layout of `SnapshotValue::List(vec![inner])`: variant index 7
/// (declaration order: Null, Bool, I64, U64, F64, Str, Bytes, List, Map),
/// then a length of 1, then the element.
fn nested_list_bytes(depth: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(depth * 2 + 1);
    for _ in 0..depth {
        v.extend_from_slice(&[7, 1]);
    }
    v.push(0); // SnapshotValue::Null
    v
}

#[test]
fn deeply_nested_snapshot_value_is_an_error_not_a_stack_overflow() {
    if let Ok(depth) = std::env::var(CHILD_ENV) {
        let depth: usize = depth.parse().expect("depth");
        let result = postcard::from_bytes::<SnapshotValue>(&nested_list_bytes(depth));
        // Dropping a deeply nested value is itself recursive; leak it here so
        // only the *decode* is under test.
        std::mem::forget(result);
        return;
    }
    let exe = std::env::current_exe().expect("test exe");
    for depth in [1_000usize, 10_000, 100_000, 1_000_000] {
        let out = Command::new(&exe)
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, depth.to_string())
            .output()
            .expect("spawn child");
        assert!(
            out.status.success(),
            "nesting depth {depth}: decoding an untrusted value crashed the process ({:?})",
            out.status
        );
    }
}

#[test]
fn deeply_nested_snapshot_value_decodes_to_a_typed_error() {
    // In-process proof for the typed arm: the depth gate rejects *before*
    // recursing, so this returns one codec error, never an abort. The
    // child-process test above covers depths that used to overflow.
    let bytes = nested_list_bytes(10_000);
    let result = postcard::from_bytes::<SnapshotValue>(&bytes);
    assert!(result.is_err(), "10,000 levels must be refused");
    // `postcard` reports serde failures as its own error type; the typed
    // arm is asserted end to end by the `decode_snapshot` test below.
    std::mem::forget(result);
}

#[test]
fn decode_snapshot_refuses_deeply_nested_records_end_to_end() {
    // A valid snapshot with one record whose last field value is `Null`,
    // then that trailing value byte is replaced with 10,000 nested lists.
    // The checksum no longer matches, but the depth gate runs during decode
    // — before the checksum gate and before the decoder recurses — so the
    // failure must be the typed depth error.
    let profile = SnapshotProfile::new(
        SchemaId::new("canary.snapshot"),
        SchemaVersion(1),
        vec![SchemaId::new("canary.health")],
    );
    let record = SnapshotRecord {
        id: 0,
        component: SchemaId::new("canary.health"),
        fields: BTreeMap::from([("deep".to_owned(), SnapshotValue::Null)]),
    };
    let (mut bytes, _) = encode_snapshot(&profile, vec![record]).expect("encode");
    assert_eq!(
        bytes.pop(),
        Some(0),
        "canonical encoding must end with the record's Null field value"
    );
    bytes.extend(nested_list_bytes(10_000));
    let err = decode_snapshot(&bytes).expect_err("nested beyond the limit must fail");
    assert!(
        matches!(err, StateError::NestingTooDeep { .. }),
        "typed depth error expected, got {err:?}"
    );
}

#[test]
fn moderately_nested_snapshot_value_round_trips() {
    let mut value = SnapshotValue::Null;
    for _ in 0..32 {
        value = SnapshotValue::List(vec![value]);
    }
    let bytes = postcard::to_allocvec(&value).expect("encode");
    let back: SnapshotValue = postcard::from_bytes(&bytes).expect("decode");
    assert_eq!(back, value);
}

#[test]
fn deeply_nested_authored_json_is_an_error_not_an_abort() {
    // serde_json's default recursion limit (128) protects the authored-JSON path.
    let dir = std::env::temp_dir().join(format!("canary-audit-depth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("deep.json");
    std::fs::write(&path, "[".repeat(1_000_000)).expect("write");
    let result = AuthoredDocument::load(&path);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(result.is_err());
}
