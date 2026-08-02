//! Integration test: compiles a tiny plugin **written in C** (see
//! `tests/fixtures/example_plugin.c`) at test time using the system `cc`,
//! then loads it through [`NativePluginLoader`] — proving Tier B's "any
//! language exposing a C ABI" claim with a real, non-Rust example rather
//! than just Rust-loading-Rust. See
//! `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`.
//!
//! **Linux-only.** This foundation's sandbox confirmed a working `cc`
//! (GCC) here and used it to validate this test end to end. The loader
//! logic itself (`src/loader.rs`) is platform-generic and is still built,
//! and unit-tested via the in-process fake-vtable test, on every platform
//! (see `loader::tests` in `src/loader.rs`) — only this specific "shell
//! out to `cc` and `dlopen` the result" fixture is restricted to Linux, to
//! avoid depending on unverified C-compiler availability/flags on the
//! macOS/Windows CI legs (see `.github/workflows/ci.yml`).

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

use canary_plugin_api::{NativePluginLoader, Plugin};

fn compile_fixture() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("tests/fixtures/example_plugin.c");

    let mut output = std::env::temp_dir();
    output.push(format!(
        "canary_example_native_plugin_{}.so",
        std::process::id()
    ));

    let status = Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&output)
        .arg(&source)
        .status()
        .expect("failed to invoke `cc` — is a C toolchain installed?");
    assert!(status.success(), "compiling the C fixture plugin failed");

    output
}

#[test]
fn loads_and_calls_a_plugin_written_in_c() {
    let path = compile_fixture();

    let loader = NativePluginLoader::new();
    let mut plugin = loader
        .load(&path)
        .expect("failed to load the C fixture plugin");

    assert_eq!(plugin.name(), "example-native-plugin (C)");

    // These must not panic/crash — proving the vtable call-through works
    // for a plugin that never saw a line of Rust.
    plugin.on_load();
    plugin.on_unload();

    // `plugin` drops at the end of this scope, which must call the C
    // fixture's `destroy` vtable entry (and then unload the library)
    // without crashing.
    drop(plugin);

    let _ = std::fs::remove_file(&path);
}
