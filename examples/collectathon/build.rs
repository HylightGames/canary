//! Build-time validation for the Tier A guest: parses
//! `assets/guest.wat` into a component binary with the `wat` crate and
//! stages it under `OUT_DIR`, so an invalid guest fails the build instead
//! of surfacing at plugin-load time.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let wat_path = manifest.join("assets").join("guest.wat");
    println!("cargo:rerun-if-changed=assets/guest.wat");
    let bytes = wat::parse_file(&wat_path).expect("assets/guest.wat parses as a component");
    let out =
        PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set")).join("guest.component.wasm");
    std::fs::write(&out, bytes).expect("staged guest component is writable");
}
