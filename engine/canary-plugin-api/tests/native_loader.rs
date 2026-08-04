//! Integration tests: compiles tiny plugins **written in C** (see
//! `tests/fixtures/`) at test time using the system `cc`, then loads them
//! through [`NativePluginLoader`] -- proving Tier B's "any language
//! exposing a C ABI" claim with real, non-Rust examples rather than just
//! Rust-loading-Rust, and proving the ABI version check (ADR 0009)
//! actually rejects a mismatch rather than merely existing in theory. See
//! `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`.
//!
//! **Linux-only.** This foundation's sandbox confirmed a working `cc`
//! (GCC) here and used it to validate these tests end to end. The loader
//! logic itself (`src/loader.rs`) is platform-generic and is still built,
//! and unit-tested via the in-process fake-vtable tests, on every platform
//! (see `loader::tests` in `src/loader.rs`) -- only this specific "shell
//! out to `cc` and `dlopen` the result" fixture is restricted to Linux, to
//! avoid depending on unverified C-compiler availability/flags on the
//! macOS/Windows CI legs (see `.github/workflows/ci.yml`).

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

use canary_plugin_api::{NativePluginLoader, Plugin, PluginError};

fn compile_fixture(source_name: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("tests/fixtures").join(source_name);

    let mut output = std::env::temp_dir();
    output.push(format!(
        "canary_{}_{}.so",
        Path::new(source_name)
            .file_stem()
            .unwrap()
            .to_string_lossy(),
        std::process::id()
    ));

    let status = Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&output)
        .arg(&source)
        .status()
        .expect("failed to invoke `cc` -- is a C toolchain installed?");
    assert!(
        status.success(),
        "compiling the C fixture plugin `{source_name}` failed"
    );

    output
}

#[test]
fn loads_and_calls_a_plugin_written_in_c() {
    let path = compile_fixture("example_plugin.c");

    let loader = NativePluginLoader::new();
    let mut plugin = loader
        .load(&path)
        .expect("failed to load the C fixture plugin");

    assert_eq!(plugin.name(), "example-native-plugin (C)");

    // These must not panic/crash -- proving the vtable call-through works
    // for a plugin that never saw a line of Rust.
    plugin.on_load();
    plugin.on_unload();

    // `plugin` drops at the end of this scope, which must call the C
    // fixture's `destroy` vtable entry (and then unload the library)
    // without crashing.
    drop(plugin);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn rejects_a_plugin_declaring_an_unsupported_abi_version() {
    let path = compile_fixture("wrong_abi_version_plugin.c");

    let loader = NativePluginLoader::new();
    let result = loader.load(&path);

    assert!(
        result.is_err(),
        "loading a plugin with a mismatched ABI version should fail, not succeed"
    );
    match result {
        Err(PluginError::UnsupportedAbiVersion {
            found, expected, ..
        }) => {
            assert_eq!(found, 999);
            assert_eq!(expected, canary_plugin_api::abi::ABI_VERSION);
        }
        Err(other) => panic!(
            "expected UnsupportedAbiVersion, got a different error: {other} -- \
             the version check in NativePluginLoader::load may have regressed"
        ),
        Ok(_) => unreachable!("checked by the assert! above"),
    }

    let _ = std::fs::remove_file(&path);
}
