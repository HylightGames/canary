# 0005. Cargo workspace + `xtask` for build orchestration, no external build system

**Status:** Accepted

## Context

A "language-agnostic," "native compilation during builds," multi-crate
engine needs a way to: build the Rust workspace, cross-compile plugins to
`wasm32-wasip2`, eventually cook assets, and orchestrate packaging — while
staying approachable to a new contributor running `git clone` for the first
time. There's a real risk of build-system sprawl in a project with these
ambitions (a temptation to reach for CMake for the native-adjacent pieces,
a separate Node-based task runner for tooling, etc.).

## Decision

**Cargo workspace** is the base build system for all Rust code (see the root
`Cargo.toml`). Cross-cutting orchestration tasks that don't fit "compile
this crate" (asset cooking, packaging, generating plugin bindings, running
the full CI check suite locally) live in an **`xtask` crate**
(`tools/xtask`) invoked as `cargo run -p xtask -- <command>` — a common,
idiomatic Rust convention (rather than a bespoke name/mechanism) precisely
so it needs no explanation beyond "it's the xtask pattern."

No external build system (CMake, Bazel, Make) is introduced for the common
path. A contributor needs `cargo` and nothing else to build, test, and run
the workspace.

## Alternatives considered

**CMake (or another general-purpose build system) as the top-level
orchestrator, with Cargo invoked from within it.** Rejected: this inverts
the natural relationship for a Rust-core project (Cargo should own the
build; CMake belongs, if anywhere, *inside* a specific crate that wraps an
existing C/C++ library, not above Cargo) and adds a second build system's
learning curve to every contributor's onboarding for a project whose core
is Rust. CMake remains entirely appropriate *within* individual crates that
bind existing C/C++ libraries (a `cc`/`cmake`-crate-driven build script is
normal and unremarkable there) — this decision is about the top-level
orchestrator, not a ban on CMake anywhere in the tree.

**A Node.js/JS-based task runner** (common in web tooling, and Node is
already present in this environment) for cross-cutting tasks. Rejected:
would require every contributor to have a Node toolchain installed just to
run engine build tasks, for no benefit over an `xtask` crate that's already
inside the Cargo workspace and requires nothing beyond what building the
engine already requires.

**Bazel or another hermetic, large-scale build system.** Rejected for this
stage: Bazel's strengths (hermetic builds, fine-grained remote caching at
massive scale) solve problems Canary doesn't have yet at this size, at the
cost of a steep adoption curve. Revisit if/when build times or monorepo
scale genuinely justify it — that would be a good candidate for a future
ADR, not a silent migration.

## Consequences

- `cargo build --workspace` / `cargo test --workspace` are the only commands
  a new contributor needs to know to get started (see
  [`CONTRIBUTING.md`](../../../CONTRIBUTING.md)).
- Cross-cutting tasks (asset cooking, packaging, WASM plugin bindgen) get
  implemented once, in Rust, inside `xtask`, instead of as ad hoc shell
  scripts with inconsistent error handling and platform assumptions.
- `Cargo.lock` is committed at the workspace root (Canary ships binaries —
  the example runtime and, later, the editor and game templates — so
  reproducible builds matter more than the flexibility a library would want
  from an uncommitted lockfile).
- CI ([`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml)) is
  itself just `cargo` invocations across a platform matrix, with no
  additional build-system-specific CI logic to maintain.
- See [`docs/development/build-system.md`](../../development/build-system.md)
  for the day-to-day mechanics this decision implies.
