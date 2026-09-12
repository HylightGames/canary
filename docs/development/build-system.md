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

**The floor isn't stuck at 1.75, though — Ubuntu's `apt` archive ships
versioned Rust toolchain packages well past the default `rustc`/`cargo`
that land on 1.75.** Found while investigating the `cargo audit` CI
failure (see the "Plugin loading" pin below): `apt-cache policy
rust-1.91-clippy` (and equivalents for other minor versions) resolves,
and `apt-get install rustc-1.91 cargo-1.91 rust-1.91-clippy
rustfmt-1.91` installs cleanly, all through this sandbox's existing
`archive.ubuntu.com`/`security.ubuntu.com` network allowlist — no
`rustup`, no blocked domain. These install as separate versioned
binaries (`/usr/bin/rustc-1.91`, `/usr/lib/rust-1.91/bin/cargo-clippy`,
etc.), not symlink replacements for `/usr/bin/rustc` — both toolchains
coexist. When invoking the newer one, set `PATH` and `RUSTC`/`RUSTDOC`
explicitly (e.g. `export PATH=/usr/lib/rust-1.91/bin:$PATH; export
RUSTC=/usr/lib/rust-1.91/bin/rustc`), and use the matching
`cargo-1.91`/`rustc-1.91` binaries directly rather than the bare
`cargo`/`rustc` names — otherwise `cargo-1.91 clippy` will silently
exec whatever `cargo-clippy` comes first on `PATH`, which may be the
1.75 one, and choke on a lockfile the newer cargo just wrote (`lock file
version 4 requires -Znext-lockfile-bump`). This doesn't retire the 1.75
floor below — plenty of pins still exist for crates that don't yet need
anything past it, and 1.91 itself isn't unlimited (`wasmtime` 46.x+
needs rustc 1.94, still out of reach) — but it meaningfully raises what
a validation session can attempt before concluding a fix needs real CI
to confirm. Re-check `apt-cache policy rustc-1.9X` for progressively
higher `X` each session; the archive's own ceiling moves forward too.

**One consequence worth knowing before it surprises anyone: regenerating
`Cargo.lock` under `cargo-1.91` moved it to lockfile format version 4**
(`version = 4` in the file's own header), which this sandbox's bare
`/usr/bin/cargo` (1.75) cannot parse at all (`lock file version 4
requires -Znext-lockfile-bump` — the same landmine documented under "Why
`Cargo.lock` is committed" above, now actually hit rather than just
anticipated). This is a one-way door: real CI's `rustup`-managed
`stable` toolchain reads version 4 without issue and would have written
one itself the next time it happened to regenerate the lockfile, but it
means **any** `cargo` invocation in this sandbox going forward — not
just ones touching `wasmtime` or anything else off the 1.75 floor —
needs the versioned `cargo-1.91` (or newer) binary explicitly. Plain
`cargo build` with no version suffix will fail immediately on `cargo
check`-level commands too, since it fails at the lockfile-parse step
before reaching any actual compilation.

**Plugin loading** (`v0.0.3` — see
[`v0.0.3-roadmap.md`](../roadmap/v0.0.3-roadmap.md)) originally pinned
`wasmtime`/`wasmtime-wasi` to `21.0.2` — the newest release this
sandbox's rustc 1.75 could reach at the time, per this section's own
process — plus five now-removed transitive floor pins (`idna`,
`idna_adapter`, `hashbrown`, `indexmap`, `litemap`) that `21.0.2` itself
needed. That pin later accumulated real cost: by the time `cargo audit`
was actually run against it, `21.0.2` carried 18 RustSec advisories,
including two **critical** (9.0) Wasmtime sandbox-escape bugs
(`RUSTSEC-2026-0095`, `RUSTSEC-2026-0096`) and one **high** (8.8)
filesystem sandbox escape (`RUSTSEC-2026-0269`) — serious for the one
crate whose entire job is sandboxing untrusted plugin code.

**`wasmtime = "=36.0.15"` and `wasmtime-wasi = "=36.0.15"`** replace
that pin: the earliest `36.x` patch closing every advisory `cargo
audit` found (checked against each advisory's own patched-version
range, not just the newest available), and — thanks to the rustc-1.91
discovery above — still validatable in this sandbox, since `36.x`
declares `rust-version = "1.86.0"`, comfortably under 1.91 (confirmed
with a real `cargo build`, not just the declared field). This pin is no
longer a *floor* pin the way `21.0.2` was — it's pinned because `wasmtime`/`wasmtime-wasi` must move together in
lockstep (see `.github/dependabot.yml`), not because anything newer
fails to compile here. Bumping past `36.x` (e.g. once a future session
can reach rustc 1.94 for `46.x`+) is a "re-verify against `cargo audit`
and this sandbox's current toolchain ceiling" task, not a security
requirement — `36.0.15` has no known open advisories as of this
writing.

Two real API breaks came with the `21.0.2` → `36.0.15` jump, both in
`tier_a.rs`, both simple signature follows rather than behavior
changes: the bindgen-generated `instantiate()` now returns just the
bindings struct instead of a `(bindings, Instance)` tuple (the discarded
`Instance` was never used downstream anyway), and `add_to_linker` now
takes an extra `HasSelf<_>` type parameter (Wasmtime's `HasData`
pattern, introduced to support concurrent host calls properly). (verified for `v0.0.5` localization — first
during scoping, then re-verified and extended at implementation time,
per this section's own "re-verify, don't assume it still holds"
discipline; see [`v0.0.5-roadmap.md`](../roadmap/v0.0.5-roadmap.md) and
[ADR 0015](../decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md))
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
- **`unic-langid-macros = "=0.9.5"` and `unic-langid-macros-impl =
  "=0.9.5"`, pinned together, for the same reason as the pair above** —
  found only at implementation time, not during scoping: `canary-loc`
  uses `unic-langid`'s `macros` feature (for the `langid!()` compile-time
  locale constant its configured default locale needs), which wasn't
  exercised by the scoping-phase spike and pulls in these two
  additionally.
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

**Rendering** (`v0.0.6` — see
[`v0.0.6-roadmap.md`](../roadmap/v0.0.6-roadmap.md) and
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md))
needed far less than localization did:

- **`ash = "0.38"`** (the Vulkan bindings `canary-render-vulkan` uses)
  — **no pin needed at all.** Compiles clean against this sandbox's
  rustc 1.75 out of the box, the cleanest dependency check run so far.
- **`naga = "=22.1.0"` and `indexmap = "=2.11.4"`**, for standalone
  `naga` (`wgpu`'s shader cross-compiler, used here without `wgpu`
  itself as a dependency — see ADR 0016). `naga`'s newer majors declare
  a `rust-version` above 1.75 (`30.0.1`, the default resolution,
  requires 1.87); `22.1.0` is the newest release still under that floor.
  `indexmap = "=2.11.4"` is its own separate pin for the same reason
  (`naga`'s dependency tree, not wasmtime's — wasmtime's own indexmap
  resolution is no longer floor-pinned; see "Plugin loading" above).

**Windowing** (`v0.0.4` — see
[`v0.0.4-roadmap.md`](../roadmap/v0.0.4-roadmap.md)) needed five pins
for `winit`, discovered across two separate passes — the second because
the ecosystem had genuinely moved between scoping and implementation,
confirmed by re-running the same verification rather than trusting the
first pass:

- **`wayland-protocols = "0.32"`** and **`wayland-scanner = "0.31.11"`**
  (originally exact-pinned to `"=0.32.9"`/`"=0.31.10"` — see below):
  found while scoping this release. Newer releases at the time each
  declared `edition = "2024"` outright — the same hard parse-time wall
  as localization's `ignore`/`globset` pins above, not an
  `is_none_or`-style compile error.
- **`quick-xml`, previously pinned to `"=0.39.4"`, is no longer an
  explicit dependency here at all.** It was `wayland-scanner`'s own
  build-time dependency, not something this crate's source used
  directly, and it carried two real RustSec advisories
  (`RUSTSEC-2026-0194`, `RUSTSEC-2026-0195`) once `cargo audit` was
  actually run against it. Fixed by raising the `wayland-scanner` floor
  to `"0.31.11"` above — that patch bumped its own internal `quick-xml`
  requirement to `0.41`, which isn't affected, and dropped it from this
  workspace's dependency declarations entirely.
- **`wayland-protocols-plasma = "0.3"` and `wayland-protocols-wlr =
  "0.3"`** (originally exact-pinned to `"=0.3.9"` each): found only at
  implementation time, re-verifying the pins above rather than assuming
  they still held. Both hard-require `wayland-protocols =
  "^0.32.<their-own-patch>"` in lockstep with their own version number,
  so pinning `wayland-protocols` alone wasn't sufficient at the time —
  these two had to be pinned to the specific patch release whose own
  requirement matched. All four `wayland-*` pins above were relaxed off
  exact versions once the rustc-1.91 discovery (above) made the
  `edition2024` wall irrelevant for this stack specifically; Cargo's own
  resolver keeps the lockstep requirement satisfied without a human
  pinning each patch release by hand.

Two things worth knowing before adding another one:

- **A crate's own declared `rust-version` isn't the whole story.**
  Wasmtime `21.0.2` (the original `v0.0.3` pin, since replaced — see
  "Plugin loading" above) declared `rust-version = "1.75.0"` and did
  build under it — but several of its *transitive* dependencies (at the
  time: `idna`, `idna_adapter`, `hashbrown`, `indexmap`, `litemap`, and,
  only with the optional `wat` text-format feature enabled, `wat`
  itself) had independently drifted past that floor through their own
  later releases — Cargo resolves each dependency to the newest release
  satisfying its semver range by default, regardless of when the crate
  that declared the range was published. Confirming a pin actually
  works means a real `cargo build`, not just reading one `rust-version`
  field. All five of those transitive pins are gone now too, along with
  the `wasmtime`/`wasmtime-wasi` pin that needed them — but the lesson
  applies to whatever pin gets added next.
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
- **Fixed, not from that review:** `canary-render-vulkan`'s
  `hello_triangle` test shipped with `v0.0.6` as a plain `#[test]`, not
  `#[ignore]`d — unlike `canary-platform`'s equivalent windowing
  integration test, no CI job provisioned a Vulkan driver for it, so it
  failed unconditionally on every OS in the `lint-and-test` matrix
  (`ubuntu-latest`, `macos-latest`, `windows-latest`, none of which have
  a real GPU or a software Vulkan ICD by default). Fixed the same way
  the windowing test already was: `#[ignore]`d, with a dedicated
  `rendering-integration` CI job that installs `mesa-vulkan-drivers`
  and runs it explicitly with `--ignored`. A second, independent cause
  of the same three job failures: `loader.rs`'s test code used a manual
  `b"...\0".as_ptr()` byte-string pattern that a newer `clippy` (this
  sandbox's local 1.75 clippy predates the lint; real CI's much newer
  one doesn't) flags as `clippy::manual_c_str_literals` under `-D
  warnings` — fixed by switching to a `c"..."` literal.
