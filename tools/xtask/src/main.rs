// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

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

/// Runs `cargo fmt --check`, `cargo clippy` (if available), and `cargo
/// test --workspace`, stopping at the first failure.
///
/// Clippy is detected rather than assumed: it requires the `clippy`
/// component, which isn't guaranteed to be installed locally the way
/// `rustfmt` and the test toolchain are. Earlier versions of this command
/// skipped clippy unconditionally for that reason -- which meant `xtask
/// check` passing locally didn't actually predict whether CI (which does
/// run clippy) would pass, a real gap flagged in
/// `docs/reviews/2026-08-senior-architecture-review.md` (Finding 8.2) and
/// tracked as risk R-18. Detecting availability and running it when
/// present, while still degrading gracefully with a clear message when
/// it's absent, closes that gap without making clippy a hard requirement
/// this command can't run without.
fn run_check() -> ExitCode {
    println!("xtask: running `cargo fmt --all -- --check`");
    if !run(&["fmt", "--all", "--", "--check"]) {
        return ExitCode::FAILURE;
    }

    if clippy_is_available() {
        println!("xtask: running `cargo clippy --workspace --all-targets -- -D warnings`");
        if !run(&[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]) {
            return ExitCode::FAILURE;
        }
    } else {
        eprintln!(
            "xtask: `clippy` component not found locally -- skipping. \
             CI still runs it and will catch anything this local check \
             can't; install it with `rustup component add clippy` to \
             catch the same issues before pushing."
        );
    }

    println!("xtask: running `cargo test --workspace`");
    if !run(&["test", "--workspace"]) {
        return ExitCode::FAILURE;
    }

    println!("xtask: all checks passed");
    ExitCode::SUCCESS
}

/// Runs `cargo <args>`, printing a clear message and returning `false` on
/// any failure (non-zero exit or failure to even launch the process)
/// rather than panicking -- `main` decides what to do with that.
fn run(args: &[&str]) -> bool {
    match Command::new("cargo").args(args).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("xtask: `cargo {}` failed ({status})", args.join(" "));
            false
        }
        Err(error) => {
            eprintln!("xtask: failed to run `cargo {}`: {error}", args.join(" "));
            false
        }
    }
}

/// Whether `cargo clippy` is available in the current environment, probed
/// by actually attempting to invoke it rather than guessing from the
/// toolchain's channel or presence of other components.
fn clippy_is_available() -> bool {
    Command::new("cargo")
        .args(["clippy", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
