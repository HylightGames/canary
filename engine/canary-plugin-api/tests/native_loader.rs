// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Integration tests: compiles tiny plugins **written in C** (see
//! `tests/fixtures/`) at test time using the system `cc`, then loads them
//! through a [`NativePluginLoader`] bound to a [`PluginRoot`] -- proving
//! Tier B's "any language exposing a C ABI" claim with real, non-Rust
//! examples rather than just Rust-loading-Rust, and proving the ABI version
//! check (ADR 0009) and the path-confinement/allowlist guardrails actually
//! reject bad input rather than merely existing in theory. See
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

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use canary_plugin_api::{NativePluginLoader, Plugin, PluginAllowlist, PluginError, PluginRoot};

/// Creates a unique temporary plugin root for one test. Tests run in
/// parallel, so the label keeps roots from colliding; a stale directory
/// from an earlier failed run is removed first.
fn temp_root(label: &str) -> PathBuf {
    let mut root = std::env::temp_dir();
    root.push(format!(
        "canary-native-loader-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("failed to create a temporary plugin root");
    root
}

fn compile_fixture(source_name: &str, root: &Path) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("tests/fixtures").join(source_name);

    let output = root.join(format!(
        "canary_{}_{}.so",
        Path::new(source_name)
            .file_stem()
            .expect("fixture source names always have a stem")
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

fn fixture_entry(path: &Path) -> OsString {
    path.file_name()
        .expect("a compiled fixture always has a file name")
        .to_owned()
}

fn write_allowlist(root: &Path, entries: &[&str]) -> PathBuf {
    let manifest = root.join("plugins.allowlist");
    let mut contents = String::from("# Canary native plugin allowlist\n");
    for entry in entries {
        contents.push_str(entry);
        contents.push('\n');
    }
    std::fs::write(&manifest, contents).expect("failed to write the allowlist manifest");
    manifest
}

#[test]
fn loads_and_calls_a_plugin_written_in_c() {
    let root = temp_root("loads-c");
    let path = compile_fixture("example_plugin.c", &root);
    let entry = fixture_entry(&path);

    let loader = NativePluginLoader::new(
        PluginRoot::new(&root).expect("the temporary plugin root must canonicalize"),
    );
    let mut plugin = loader
        .load(&entry)
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

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rejects_a_plugin_declaring_an_unsupported_abi_version() {
    let root = temp_root("wrong-abi");
    let path = compile_fixture("wrong_abi_version_plugin.c", &root);
    let entry = fixture_entry(&path);

    let loader = NativePluginLoader::new(
        PluginRoot::new(&root).expect("the temporary plugin root must canonicalize"),
    );
    let result = loader.load(&entry);

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

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn loads_an_entry_listed_in_the_allowlist_manifest() {
    let root = temp_root("allowlisted");
    let path = compile_fixture("example_plugin.c", &root);
    let entry = fixture_entry(&path);
    let entry_name = entry.to_string_lossy().into_owned();
    let manifest = write_allowlist(&root, &[&entry_name]);

    let allowlist =
        PluginAllowlist::from_file(&manifest).expect("failed to parse the allowlist manifest");
    let loader = NativePluginLoader::with_allowlist(
        PluginRoot::new(&root).expect("the temporary plugin root must canonicalize"),
        allowlist,
    );

    let plugin = loader
        .load(&entry)
        .expect("an allowlisted plugin must still load");
    assert_eq!(plugin.name(), "example-native-plugin (C)");
    drop(plugin);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn refuses_an_entry_missing_from_the_allowlist_manifest() {
    let root = temp_root("not-allowlisted");
    let path = compile_fixture("example_plugin.c", &root);
    let entry = fixture_entry(&path);
    let manifest = write_allowlist(&root, &["some-other-plugin.so"]);

    let allowlist =
        PluginAllowlist::from_file(&manifest).expect("failed to parse the allowlist manifest");
    let loader = NativePluginLoader::with_allowlist(
        PluginRoot::new(&root).expect("the temporary plugin root must canonicalize"),
        allowlist,
    );

    match loader.load(&entry) {
        Err(PluginError::NotAllowlisted { .. }) => {}
        Err(other) => panic!("expected NotAllowlisted, got a different error: {other}"),
        Ok(_) => panic!("an entry absent from the allowlist must not load"),
    }

    let _ = std::fs::remove_dir_all(&root);
}
