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
//! v0.0.1-pre1 ships one real subcommand, `check` (runs the workspace's
//! four core quality gates locally, against the committed lockfile).
//! More subcommands (asset cooking, packaging, plugin bindgen)
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
    eprintln!("  check    Run the four core workspace quality gates");
}

/// Runs the four core workspace quality gates with the committed lockfile,
/// stopping at the first failure. Clippy and rustfmt are required local
/// toolchain components; a missing component is a failed check, not a
/// successful partial run.
fn run_check() -> ExitCode {
    println!("xtask: running `cargo fmt --all -- --check`");
    if !run(&["fmt", "--all", "--", "--check"]) {
        return ExitCode::FAILURE;
    }

    println!("xtask: running `cargo build --locked --workspace --all-targets`");
    if !run(&["build", "--locked", "--workspace", "--all-targets"]) {
        return ExitCode::FAILURE;
    }

    println!("xtask: running `cargo test --locked --workspace`");
    if !run(&["test", "--locked", "--workspace"]) {
        return ExitCode::FAILURE;
    }

    println!("xtask: running `cargo clippy --locked --workspace --all-targets -- -D warnings`");
    if !run(&[
        "clippy",
        "--locked",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]) {
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
