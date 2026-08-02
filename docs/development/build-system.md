# Build System

The decision behind this is recorded in
[ADR 0005](../decisions/architecture-decision-records/0005-build-system-and-tooling.md).
This document is the day-to-day mechanics.

## Prerequisites

- A stable Rust toolchain (see [`rust-toolchain.toml`](../../rust-toolchain.toml)
  at the repository root — if you use `rustup`, it will pick this up
  automatically, including the `wasm32-wasip2` target and the `rustfmt`/
  `clippy` components).
- No other tools are required for the core workspace. (Individual future
  crates that bind existing C/C++ libraries may add their own build-time
  requirements, documented in that crate's own README when it's added.)

## Common commands

```sh
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Format
cargo fmt --all

# Lint (CI denies warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Run the headless boot-harness binary
cargo run -p canary-runtime

# Check that plugin-facing crates still target wasm32-wasip2
cargo check -p canary-plugin-api --target wasm32-wasip2
```

## Workspace layout

The root [`Cargo.toml`](../../Cargo.toml) defines a Cargo workspace over:

- `engine/*` — the engine crates themselves (see
  [`repository-structure.md`](repository-structure.md) for what lives where).
- `tools/xtask` — the build-orchestration crate described below.

Adding a new crate to the engine means adding it to the workspace
`members` list and giving it a `canary-`-prefixed name, per
[`coding-standards.md`](coding-standards.md#naming).

## The `xtask` pattern

`tools/xtask` is a small, ordinary Rust binary crate, invoked as:

```sh
cargo run -p xtask -- <command>
```

It exists for tasks that aren't "compile a crate" — asset cooking (once
[`docs/architecture/asset-system.md`](../architecture/asset-system.md) has
an implementation to invoke), packaging a distributable build, generating
plugin/WIT bindings, or running the same checks CI runs, locally, in one
command. This foundation ships `xtask` with a minimal `check` command
(runs fmt-check, clippy, and tests in sequence) as a working proof of the
pattern; it is expected to grow real subcommands as the corresponding
subsystems (asset pipeline, plugin bindgen) are built.

Using a Rust binary crate for this — rather than shell scripts or a Makefile
— means orchestration logic gets the same compiler checks, the same
cross-platform behavior (a shell script that works on Linux/macOS and
silently doesn't on Windows is a recurring problem this sidesteps), and the
same testability as engine code itself.

## Cross-compilation

Plugins and gameplay scripts targeting the Tier A sandbox (see
[`docs/architecture/plugin-system.md`](../architecture/plugin-system.md))
compile to `wasm32-wasip2`. Once the Tier A loader exists (tracked in
[`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md)), `xtask`
is the intended home for a `build-plugin` command wrapping the relevant
`cargo build --target wasm32-wasip2` plus component-tooling invocation, so
plugin authors don't need to hand-assemble that command themselves.

## Native (Tier B) plugin builds

Trusted native plugins ([`docs/architecture/plugin-system.md`](../architecture/plugin-system.md#tier-b--trusted-native-c-abi))
are ordinary dynamic libraries. A Rust-authored Tier B plugin is just a
`crate-type = ["cdylib"]` crate; a C/C++-authored one builds however that
ecosystem normally builds a shared library — Canary's build system does not
try to own or standardize that, only the stable C ABI contract at the
boundary (see [`docs/architecture/plugin-system.md`](../architecture/plugin-system.md)).

## Why `Cargo.lock` is committed

Per [ADR 0005](../decisions/architecture-decision-records/0005-build-system-and-tooling.md),
`Cargo.lock` is tracked at the workspace root. Canary ships binaries
(`canary-runtime` today; the editor and game templates later), and
reproducible builds across contributors and CI matter more here than the
flexibility an uncommitted lockfile would give a pure library.
