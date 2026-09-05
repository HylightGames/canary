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

## The `rustc` 1.75 sandbox-validation floor

`rust-toolchain.toml` pins `channel = "stable"`, deliberately left
unpinned to an exact version — see that file's own comment for why. Some
implementation sessions, though, have only had network access to this
project's specific sandboxed environment, which installs Rust via `apt`
from Ubuntu's archive rather than `rustup`, landing on whatever `rustc`
that archive currently carries (`1.75.0` as of this writing). That's
older than most of the ecosystem now assumes, and the gap only grows —
Wasmtime, for one, tracks only the latest three stable Rust releases and
moves its own MSRV forward continuously (see
[`v0.0.3-roadmap.md`](../roadmap/v0.0.3-roadmap.md)).

Where a dependency — or one of *its* transitive dependencies — has moved
past whatever `rustc` a given validation session can reach, the fix has
been to pin that one dependency to the newest release still compatible,
not to relax the workspace's actual `rust-toolchain.toml` floor, which
stays `stable` for real contributors using `rustup` normally. Each pin
carries an inline comment explaining why, pointing back here.

**The `fluent-rs` family** (verified for the `v0.0.5` localization
scoping pass — see [`v0.0.5-roadmap.md`](../roadmap/v0.0.5-roadmap.md)
and [ADR 0015](../decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md))
needed several pins, one of which wasn't the simple single-crate fix the
`idna`/`hashbrown`-style pins above are:

- **`rustc-hash = "=2.1.1"`** — `2.1.3`, the default resolution,
  requires rustc 1.77+.
- **`unic-langid = "=0.9.5"` and `unic-langid-impl = "=0.9.5"`, pinned
  *together*.** `unic-langid-impl 0.9.6` calls `Option::is_none_or`
  directly in its source (stabilized in rustc 1.82) — a real compile
  error, not a `rust-version`-field mismatch Cargo flags up front.
  Pinning `unic-langid-impl` on its own doesn't resolve it: `unic-langid
  0.9.6` hard-requires `unic-langid-impl = "^0.9.6"` exactly, so
  `unic-langid` itself has to drop to `0.9.5` before the impl-crate pin
  can take effect. `tinystr` then resolves to `0.7.6` on its own, via
  `unic-langid-impl 0.9.5`'s own `^0.7.0` requirement — no separate pin
  needed for it once the two above are in place.
- Only relevant if `fluent-templates`, `fluent-fallback`, or
  `i18n-embed` are reconsidered later (`v0.0.5` itself doesn't depend on
  them — see the ADR): **`block-buffer = "=0.10.4"`, `ignore =
  "=0.4.20"`, `globset = "=0.4.14"`**, each because a newer release
  declares `edition = "2024"` outright — a hard parse-time wall under
  Cargo 1.75 (`error: … requires edition2024 …`), distinct from the
  `is_none_or`-style compile error above. And **`rust-embed`,
  `rust-embed-impl`, `rust-embed-utils`, pinned together to
  `"=8.11.0"`**: `8.12.0` bumps to `sha2 ^0.11`, dragging in a newer
  `digest`/`block-buffer` major version with the same `edition2024`
  wall; `8.11.0` still resolves to `sha2 ^0.10.5` → `digest 0.10.7` →
  `block-buffer 0.10.4`, all fine under 1.75.

Two things worth knowing before adding another one:

- **A crate's own declared `rust-version` isn't the whole story.**
  Wasmtime `21.0.2` declares `rust-version = "1.75.0"` and does build
  under it — but several of its *transitive* dependencies (at time of
  writing: `idna`, `idna_adapter`, `hashbrown`, `indexmap`, `litemap`,
  and, only if the optional `wat` text-format feature is enabled, `wat`
  itself) had, by the time this was checked, independently drifted past
  that floor through their own later releases — Cargo resolves each
  dependency to the newest release satisfying its semver range by
  default, regardless of when the crate that declared the range was
  published. Confirming a pin actually works means a real `cargo build`,
  not just reading one `rust-version` field.
- **Bump these pins opportunistically, not on a schedule.** Any session
  with access to a current `stable` toolchain (via `rustup`, or a
  less-constrained sandbox) should feel free to re-verify whether a pin
  is still needed and relax it if the underlying gap has closed — the
  same encouragement the `libloading` pin below already gives.

## Known limitations (added by the August 2026 architecture review)

- **CI previously set a blanket `RUSTFLAGS: "-D warnings"`**, which
  applies to every `rustc` invocation Cargo makes, including third-party
  dependencies — meaning a new compiler release could fail CI over a
  warning in code this project doesn't own. This was fixed as part of
  that review (see [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml));
  the workspace `[lints]` table plus `cargo clippy -- -D warnings`
  (correctly scoped to workspace members only) now carry the enforcement
  instead. See
  [`docs/reviews/2026-08-senior-architecture-review.md`](../reviews/2026-08-senior-architecture-review.md),
  Finding 2.1, and risk register R-02.
- **`xtask check` currently skips `clippy`** (documented reason: the
  component isn't guaranteed installed locally), which means it can pass
  locally while CI's separate clippy gate still fails on the same push.
  Not yet fixed — a small code change, tracked as a follow-up rather than
  made during that review (which scoped itself to no major implementation
  code). See the review, Finding 8.2, and risk register R-18.
- **No CI build-cache strategy exists.** Not urgent at the current crate
  count and contributor count; worth planning for before CI cost/latency
  becomes a visible problem rather than after. See the review, Finding
  8.3, and risk register R-28.
