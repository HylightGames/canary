//! Canary Engine build orchestration.
//!
//! See `docs/development/build-system.md#the-xtask-pattern` and
//! `docs/decisions/architecture-decision-records/0005-build-system-and-tooling.md`
//! for why this exists as an ordinary Rust binary crate (invoked as
//! `cargo run -p xtask -- <command>`) rather than as shell scripts or an
//! external build system.
//!
//! v0.0.1-pre1 ships one real subcommand, `check` (runs the same
//! fmt/test sequence CI does, so a contributor can run it locally before
//! pushing). More subcommands (asset cooking, packaging, plugin bindgen)
//! land as the subsystems they orchestrate are built — see
//! `docs/roadmap/future-roadmap.md`.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let command = std::env::args().nth(1);
    match command.as_deref() {
        Some("check") => run_check(),
        Some(other) => {
            eprintln!("xtask: unknown command `{other}`");
            print_usage();
            ExitCode::FAILURE
        }
        None => {
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!("Usage: cargo run -p xtask -- <command>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  check    Run the same fmt-check + test sequence CI runs");
}

/// Runs `cargo fmt --check` then `cargo test --workspace`, stopping at the
/// first failure. Deliberately does **not** also run `cargo clippy` here:
/// clippy requires the `clippy` component, which isn't guaranteed to be
/// installed locally the way `rustfmt` and the test toolchain are — see
/// `docs/development/build-system.md`. CI runs clippy separately (see
/// `.github/workflows/ci.yml`) regardless of whether it's available in a
/// given contributor's local setup.
fn run_check() -> ExitCode {
    let steps: &[(&str, &[&str])] = &[
        (
            "cargo fmt --all -- --check",
            &["fmt", "--all", "--", "--check"],
        ),
        ("cargo test --workspace", &["test", "--workspace"]),
    ];

    for (label, args) in steps {
        println!("xtask: running `{label}`");
        let status = Command::new("cargo").args(*args).status();
        match status {
            Ok(status) if status.success() => continue,
            Ok(status) => {
                eprintln!("xtask: `{label}` failed ({status})");
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("xtask: failed to run `{label}`: {error}");
                return ExitCode::FAILURE;
            }
        }
    }

    println!("xtask: all checks passed");
    ExitCode::SUCCESS
}
