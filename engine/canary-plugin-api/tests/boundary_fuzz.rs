//! Bounded size-gate/boundary fuzz sweep for the Tier A WASM loader
//! (`tier_a/wasm_loader.rs`).
//!
//! FINDINGS-ONLY sweep: this file feeds hostile bytes to the public loader
//! entries ([`WasmPluginLoader::instantiate_from_bytes`] and
//! [`WasmPluginLoader::load`]) and asserts the contract — `Ok(instance)`
//! or `Err(typed)` — never a panic, hang (>10 s per case), unbounded
//! allocation, or an `Ok` that bypassed the [`MAX_PLUGIN_BYTES`] size gate.
//!
//! Method: deterministic seeded corpus (no RNG at test time). One honestly
//! compiled component (from inline WAT, so the sweep needs no external
//! toolchain) is mutated — bit flips at seeded offsets, truncation at
//! fractional cuts plus every 1 KiB boundary, header-field extremes — and
//! the size gate is pinned from both sides (one byte over refuses, exactly
//! at the limit reaches the parser). Inputs stay at kilobyte scale except
//! the two gate-pinning cases, so the whole file runs in seconds.
///
/// `WasmComponentPlugin` carries no `Debug`, so loader results are matched
/// explicitly (`match ... { Err(e) => e, Ok(_) => panic! }`) rather than
/// unwrapped with `expect_err` — the same outcome with no trait bound.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use canary_ecs::World;
use canary_plugin_api::{Capability, CodecRegistry, PluginError, ResourceBudget, WasmPluginLoader};

/// Mirrors `tier_a::wasm_loader::MAX_PLUGIN_BYTES` (not re-exported at the
/// crate root, so spelled out here): 16 MiB. The sweep cross-checks the
/// value instead of trusting it — the over-limit cases assert the refusal
/// message names this number, so a production bump of the real gate fails
/// these tests loudly rather than silently testing the wrong boundary.
const SWEEP_MAX_PLUGIN_BYTES: u64 = 16 * 1024 * 1024;

/// A minimal, hand-written Component Model fixture: imports `ecs-read` and
/// exports the lifecycle instance, so instantiating it with
/// [`Capability::ReadEcsWorld`] granted succeeds. Mirrors the in-module
/// `TEST_COMPONENT_WAT` shape without depending on private test items.
const SWEEP_COMPONENT_WAT: &str = r#"
        (component
          (import "canary:plugin/ecs-read@0.1.0" (instance $ecs-read-import
            (export "entity-count" (func (result u32)))
          ))

          (core module $guest
            (import "host" "entity-count" (func $entity_count (result i32)))
            (global $cached (mut i32) (i32.const 0))
            (func (export "on-load")
              (global.set $cached (call $entity_count)))
            (func (export "on-unload"))
            (func (export "last-entity-count") (result i32)
              (global.get $cached))
          )

          (core func $entity_count_lowered
            (canon lower (func $ecs-read-import "entity-count")))

          (core instance $guest_instance (instantiate $guest
            (with "host" (instance
              (export "entity-count" (func $entity_count_lowered))
            ))
          ))

          (func $on_load_lifted (canon lift (core func $guest_instance "on-load")))
          (func $on_unload_lifted (canon lift (core func $guest_instance "on-unload")))
          (func $last_entity_count_lifted (result u32)
            (canon lift (core func $guest_instance "last-entity-count")))

          (instance $lifecycle_export
            (export "on-load" (func $on_load_lifted))
            (export "on-unload" (func $on_unload_lifted))
            (export "last-entity-count" (func $last_entity_count_lifted))
          )
          (export "canary:plugin/lifecycle@0.1.0" (instance $lifecycle_export))
        )
    "#;

/// Builds a loader with default budgets.
fn loader() -> WasmPluginLoader {
    WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
        .expect("engine setup must succeed")
}

/// Capabilities granting the fixture's single import.
fn read_caps() -> HashSet<Capability> {
    HashSet::from([Capability::ReadEcsWorld])
}

/// The honestly compiled fixture bytes.
fn valid_bytes() -> Vec<u8> {
    wat::parse_str(SWEEP_COMPONENT_WAT).expect("fixture WAT must compile")
}

/// Feeds `bytes` to the in-memory entry and checks the contract: any typed
/// [`PluginError`] is acceptable; an `Ok` means the mutated bytes still
/// parsed and instantiated as a valid component (the loader grants no
/// partial instances — instantiation either links every granted interface
/// or fails).
fn check_contract(loader: &WasmPluginLoader, bytes: &[u8], _name: &str) -> Result<(), PluginError> {
    let name = _name;
    match loader.instantiate_from_bytes(bytes, name, &read_caps()) {
        Ok(_) => Ok(()),
        Err(err) => {
            assert!(
                matches!(
                    err,
                    PluginError::WasmParse { .. } | PluginError::WasmInstantiate { .. }
                ),
                "boundary input must fail typed (parse/instantiate), got: {err:?}"
            );
            Err(err)
        }
    }
}

#[test]
fn valid_component_instantiates_from_bytes() {
    let loader = loader();
    let bytes = valid_bytes();
    assert!(
        bytes.len() as u64 <= SWEEP_MAX_PLUGIN_BYTES,
        "the kilobyte fixture must sit far under the gate"
    );
    check_contract(&loader, &bytes, "sweep-valid").expect("valid component must instantiate");
}

#[test]
fn empty_garbage_and_truncations_are_typed_parse_errors() {
    let loader = loader();
    let base = valid_bytes();
    // Truncation at the empty/1-byte/8-byte edges, fractional cuts, the
    // off-by-one tail, and every 1 KiB boundary (the fixture is a few KiB,
    // so boundaries add real points): every prefix must fail as WasmParse,
    // rejected before instantiation.
    let mut cuts = vec![0usize, 1, 8];
    let mut boundary = 1024;
    while boundary < base.len() {
        cuts.push(boundary);
        boundary += 1024;
    }
    for cut in [
        base.len() / 4,
        base.len() / 2,
        base.len() * 3 / 4,
        base.len() - 1,
    ] {
        cuts.push(cut);
    }
    cuts.sort_unstable();
    cuts.dedup();
    for cut in cuts {
        // Every prefix must fail typed. Most fail as WasmParse — but a
        // prefix that still parses as a component skeleton (e.g. the
        // 8-byte magic+version header) reaches instantiation and fails
        // there as WasmInstantiate (unsatisfied lifecycle import). Both
        // are typed refusals before any guest code runs; the contract
        // holds either way.
        let err = check_contract(&loader, &base[..cut], "sweep-trunc")
            .expect_err("truncated input must fail");
        assert!(
            matches!(
                err,
                PluginError::WasmParse { .. } | PluginError::WasmInstantiate { .. }
            ),
            "truncate-{cut} must fail typed, got: {err:?}"
        );
    }
    for (label, bytes) in [
        ("empty", vec![]),
        ("garbage-text", b"this is not a wasm component".to_vec()),
        ("all-zeros-64", vec![0u8; 64]),
    ] {
        let err =
            check_contract(&loader, &bytes, "sweep-garbage").expect_err("garbage input must fail");
        assert!(
            matches!(err, PluginError::WasmParse { .. }),
            "{label} must be WasmParse (rejected before instantiation), got: {err:?}"
        );
    }
}

#[test]
fn bit_flip_sweep_never_panics_and_fails_typed() {
    let loader = loader();
    let base = valid_bytes();
    // Seeded offsets: magic (0..4), version (4..8), section headers, the
    // middle, and the tail.
    let mut offsets = vec![0, 1, 4, 5, 8, 12, 16];
    offsets.push(base.len() / 2);
    offsets.push(base.len() - 1);
    for offset in offsets {
        for xor in [0x01u8, 0xFFu8] {
            let mut bytes = base.clone();
            bytes[offset] ^= xor;
            // A flip either breaks the parse/instantiation (typed Err) or
            // yields a still-valid instance. Never a panic.
            let _ = check_contract(&loader, &bytes, "sweep-flip");
        }
    }
}

#[test]
fn header_field_extremes_are_typed_parse_errors() {
    let loader = loader();
    let base = valid_bytes();
    assert_eq!(
        &base[0..4],
        b"\0asm",
        "the fixture must start with the wasm magic the sweep corrupts"
    );
    // Corrupt magic: not a component at all.
    let mut bad_magic = base.clone();
    bad_magic[0..4].copy_from_slice(b"BAD!");
    let err = check_contract(&loader, &bad_magic, "sweep-magic").expect_err("bad magic must fail");
    assert!(
        matches!(err, PluginError::WasmParse { .. }),
        "bad magic must be WasmParse, got: {err:?}"
    );
    // Version extremes: version 0 and version u32::MAX — both outside the
    // component-model version this toolchain emits.
    for version in [0u32, u32::MAX] {
        let mut bytes = base.clone();
        bytes[4..8].copy_from_slice(&version.to_le_bytes());
        let err = check_contract(&loader, &bytes, "sweep-version")
            .expect_err("extreme version must fail");
        assert!(
            matches!(
                err,
                PluginError::WasmParse { .. } | PluginError::WasmInstantiate { .. }
            ),
            "version {version} must fail typed, got: {err:?}"
        );
    }
}

#[test]
fn size_gate_refuses_one_byte_over_and_admits_exactly_at_limit() {
    let loader = loader();
    // One byte over: refused before Wasmtime parses, and the refusal names
    // the limit so it stays distinguishable from a genuine parse failure.
    let oversized = vec![
        0u8;
        usize::try_from(SWEEP_MAX_PLUGIN_BYTES + 1)
            .expect("the limit must fit in usize")
    ];
    let err = match loader.instantiate_from_bytes(&oversized, "sweep-over-limit", &read_caps()) {
        Err(err) => err,
        Ok(_) => panic!("over-limit bytes must be refused"),
    };
    match &err {
        PluginError::WasmParse { source, .. } => {
            assert!(
                source
                    .to_string()
                    .contains(&SWEEP_MAX_PLUGIN_BYTES.to_string()),
                "the refusal source must report the limit, got: {source}"
            );
        }
        other => panic!("over-limit bytes must be WasmParse, got: {other:?}"),
    }
    // Exactly at the limit: passes the gate and reaches the parser, which
    // rejects the zero bytes as a genuine parse failure — WITHOUT the
    // size-gate message (proving the gate is exact, not off-by-one).
    let at_limit =
        vec![0u8; usize::try_from(SWEEP_MAX_PLUGIN_BYTES).expect("the limit must fit in usize")];
    let err = match loader.instantiate_from_bytes(&at_limit, "sweep-at-limit", &read_caps()) {
        Err(err) => err,
        Ok(_) => panic!("zero bytes are not a component"),
    };
    match &err {
        PluginError::WasmParse { source, .. } => {
            assert!(
                !source.to_string().contains("over the"),
                "at-limit input must reach the parser, not the size gate, got: {source}"
            );
        }
        other => panic!("at-limit zeros must be WasmParse, got: {other:?}"),
    }
}

#[test]
fn missing_file_is_parse_shaped_not_a_panic() {
    let loader = loader();
    let missing = Path::new("definitely-not-a-plugin-boundary.wasm");
    let err = match loader.load(missing, "sweep-missing", &read_caps(), World::new()) {
        Err(err) => err,
        Ok(_) => panic!("missing file must fail"),
    };
    assert!(
        matches!(err, PluginError::WasmParse { .. }),
        "missing file must be WasmParse-shaped, got: {err:?}"
    );
}

#[test]
fn sparse_over_limit_file_is_refused_before_parsing() {
    let loader = loader();
    // A sparse file reports past the gate from metadata while costing
    // nothing on disk: refused without reading a byte.
    let path: PathBuf = std::env::temp_dir().join(format!(
        "canary-sweep-size-gate-{}-over-limit.wasm",
        std::process::id()
    ));
    let file = std::fs::File::create(&path).expect("sparse fixture must be creatable");
    file.set_len(SWEEP_MAX_PLUGIN_BYTES + 1)
        .expect("sparse resize must succeed");
    drop(file);
    let err = match loader.load(&path, "sweep-sparse", &read_caps(), World::new()) {
        Err(err) => err,
        Ok(_) => panic!("over-limit sparse file must fail"),
    };
    let _ = std::fs::remove_file(&path);
    match &err {
        PluginError::WasmParse { path: got, source } => {
            assert_eq!(got.as_path(), path.as_path());
            assert!(
                source
                    .to_string()
                    .contains(&SWEEP_MAX_PLUGIN_BYTES.to_string()),
                "the refusal must report the limit, got: {source}"
            );
        }
        other => panic!("over-limit file must be WasmParse, got: {other:?}"),
    }
}
