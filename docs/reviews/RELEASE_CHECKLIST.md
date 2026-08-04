# v0.0.1 Release Checklist

Produced while completing `v0.0.1`, per the release process this
document itself establishes as a precedent for future milestones. Every
item below was actually checked, not assumed — where a check involved
running something, the command and result are noted.

## 1. Audit the entire repository

- [x] Full directory tree reviewed against
  [`docs/development/repository-structure.md`](../development/repository-structure.md);
  no orphaned or undocumented top-level paths.
- [x] `git status` clean; `git log` reviewed for commit-message quality.
- [x] `grep -rn "TODO\|FIXME\|XXX"` across `engine/` and `tools/` — no
  matches. No unaddressed inline markers exist to lose track of.

## 2. Review every ADR; update any that no longer reflect reality

- [x] All 12 ADRs' status lines reviewed (`0001`–`0012`; `0011` and `0012`
  were added during this same session, after two significant
  architectural principles — a native UI abstraction and explicit,
  versionable project state — were raised and judged foundational enough
  to document now rather than defer).
- [x] [ADR 0009](0009-plugin-abi-versioning-and-extensibility.md) updated
  from "accepted, not implemented" to "accepted and implemented," with
  its Consequences section corrected to match.
- [x] [ADR 0003](0003-plugin-and-modding-architecture.md) and
  [ADR 0006](0006-versioning-scheme.md) already cross-referenced the ADRs
  that amend them (added during the architecture review); still accurate.
- [x] [ADR 0010](0010-component-identity-across-language-boundary.md)
  correctly remains `Proposed` — it depends on the archetype ECS
  migration, which is `v0.0.2`+ scope; marking it `Accepted` now would be
  premature.
- [x] ADRs `0001`, `0002`, `0004`, `0005`, `0007`, `0008` reviewed; no
  drift found, no changes needed.

## 3. Ensure all documentation matches the implementation

- [x] [`docs/architecture/plugin-system.md`](../architecture/plugin-system.md)
  corrected: referenced a `register` lifecycle hook that was never in the
  `Plugin` trait (only `name`, `on_load`, `on_unload` exist) — a
  pre-existing drift caught during this audit, not introduced by it.
- [x] "Known limitations" sections in
  [`core-runtime.md`](../architecture/core-runtime.md) and
  [`plugin-system.md`](../architecture/plugin-system.md) updated to move
  now-fixed items (Send + Sync, generation width, ABI versioning) out of
  "known limitation" and into "resolved," rather than leaving stale
  warnings about problems that no longer exist.
- [x] Every `v0.0.1-pre1` reference describing *current* implementation
  state updated to `v0.0.1` across affected docs
  (`docs/architecture/{physics,asset-system,networking,scripting-system,platform-abstraction}.md`,
  `docs/vision/project-goals.md`, `README.md`, `examples/README.md`).
  References describing *historical* facts (what the foundation session
  or the architecture review specifically produced) were deliberately
  left as `v0.0.1-pre1`, since changing those would make the history
  inaccurate, not more current.
- [x] [`README.md`](../../README.md) status banner and "what's actually
  implemented" section rewritten for the real release.
- [x] [`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md)'s
  definition of done revised and the reasoning for narrowing it recorded
  explicitly (not silently).

## 4. Verify formatting, linting, tests, and CI

- [x] `cargo build --workspace` — clean, zero warnings.
- [x] `cargo fmt --all -- --check` — clean.
- [x] `cargo test --workspace` — 16 tests, all passing (3 `canary-core`,
  6 `canary-ecs` including a `proptest` property test, 2 `canary-platform`,
  3 `canary-plugin-api` unit + 2 integration, including a real C plugin
  compiled with `gcc` at test time and a version-mismatch rejection test).
- [x] `cargo run -p xtask -- check` — the full local gate (fmt, clippy if
  available, test) passes.
- [ ] **`clippy` was not runnable in this sandbox** (not installable via
  `apt`, and `rustup` cannot reach its host from this network — see
  [`docs/development/build-system.md`](../development/build-system.md)).
  CI (`.github/workflows/ci.yml`) runs it via `rustup` on GitHub-hosted
  runners and is the actual gate for this; **this item can only be
  verified by CI actually running once pushed**, and is the one release
  checklist item this session could not close out itself. Flagged
  honestly rather than assumed clean.
- [x] CI's cross-platform matrix (Linux/macOS/Windows) and
  `wasm32-wasip2` target check reviewed for correctness; not itself
  runnable here (no GitHub Actions runner in this sandbox), same caveat
  as above.

## 5. Remove dead code and unnecessary complexity

- [x] No dead code found by the compiler (`cargo build` surfaces
  `dead_code` at `warn` by default; zero warnings — see item 4).
- [x] No unnecessary dependencies added during this session; the only new
  code (ABI versioning, `Send + Sync` bounds, generation widening, xtask
  clippy detection) used only `std` and already-present dependencies.
- [x] Reviewed for premature abstraction: no speculative API surface was
  added for capabilities that don't exist yet (e.g., no safe wrapper was
  added for `get_extension` beyond the raw vtable slot, since there is
  nothing real to call it for until the first extension is designed —
  see [ADR 0009](0009-plugin-abi-versioning-and-extensibility.md)).

## 6. Identify anything that should be postponed to `v0.0.2` or later

Recorded explicitly in
[`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md#definition-of-done-for-the-unqualified-v001--revised):
archetype ECS storage and the parallel job-stealing scheduler, the
Wasmtime-backed Tier A plugin loader, a real windowing backend, and
change-detection query filters. See that document for the reasoning, and
[`docs/roadmap/future-roadmap.md`](../roadmap/future-roadmap.md) for
everything with a real dependency on them.

Lower-priority findings from the architecture review that remain `Open`
in [`risk-register.md`](risk-register.md) (manifest format, Tier B
trust/signing, WIT versioning, hot-reload contract, entity hierarchy,
system ordering, and others) are correctly not part of `v0.0.1` — each is
tied to a subsystem (marketplace, Tier A, archetype ECS) that is itself
out of scope for this release.

## 7. Produce a release checklist and ensure every item is complete

This document. One item (automated `clippy`/CI verification) could not
be closed from within this sandbox and is flagged rather than assumed —
see item 4. Every other item is complete.
