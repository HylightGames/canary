# Release Checklist — historical v0.0.1 record plus v0.1.0 gate (2026-10-23)

> Historical note: sections 1–N below are the v0.0.1 record, preserved as history. The v0.1.0 closure gate follows at the end and is not yet fully checked — do not treat historical checks as v0.1.0 evidence.

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

---

# `v0.1.0` Release Checklist (appended; `v0.0.1` history above untouched)

The `v0.1.0` bar is the integration proof in
`docs/roadmap/v0.1.0-plan.md` WP4: a developer on the documented
consumer path builds and plays `examples/collectathon/` without private
harness APIs, and CI plus this checklist clearly distinguish automated
evidence from required live-device evidence. `v0.1.0` is NOT released —
rows below record the gate as documented, not as passed.

## A. Automated evidence (CI / deterministic local)

- [x] `cargo build --workspace`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace` green on the release commit
  (CI `lint-and-test`, all three OSes). Evidence: CI run 40101053516
  (2026-10-24) on commit `9c5bcb5` — Build/lint/test ubuntu + macos +
  windows, Format check, Rustdoc, all success.
- [x] Headless twin: `cargo run -p collectathon --bin
  collectathon_headless` prints the deterministic score line (90
  scripted frames; decode-only audio). Evidence: local run prints
  `score 1 collected 1/4 player (-116.0, 0.0) after 90 frames`
  (code-identical tree — only CI YAML changed since); CI release-gate
  job (run 40101053516) green.
- [x] Server/client replication run converges (snapshot → delta → input
  → ack, clean disconnect). Evidence: local loopback run —
  `snapshot converged on 6 entries across 5 entities`, `delta applied —
  5 changes, 1 removals`, `ack seq=1`, `clean disconnect` (code-identical
  tree); CI release-gate job (run 40101053516) green.
- [x] Two-client collaboration run converges
  (accept/deny/conflict/sync). Evidence: local run prints
  `two-client run converged` (owner accept, reader deny, stale conflict
  with refresh, rebased accept, sync tail); CI release-gate job (run
  40101053516) green.
- [x] `cargo test -p collectathon --lib plugin` (Tier A guest probe)
  and `cargo test -p collectathon --lib state` (snapshot determinism)
  pass. Evidence: 2 plugin tests + `capture_restore_recapture_is_byte_stable`
  pass; input-retention tests (`event_only_press_and_ui_intent_deliver_once_on_next_sim_pass`,
  `press_and_release_across_event_only_frames_both_survive`) pass and fail
  pre-fix.
- [x] CI Xvfb/llvmpipe legs green (`windowing-integration`,
  `rendering-integration`, `render-ecs-integration`). Evidence: CI run
  40101053516 on `9c5bcb5` — all three jobs success (incl. the new
  `WindowGone` destroyed-window refusal and cross-device readback
  misuse tests alongside `present_clear`, `render_ecs_readback` +
  `physics_game_proof`).
- [x] `docs/release-notes/v0.1.0.md` exists and matches this checklist;
  `examples/collectathon/README.md` setup/launch/controls plus the
  first-game guide are current; Linux native packages and the
  git/path consumer route are documented with no `crates.io`
  implication (`canary-runtime` is `publish = false`; no facade crate).
  Verified 2026-10-24: release note present with
  `Not a crates.io consumer release`; both READMEs state the git/path
  route only; remaining `crates.io` mentions are explicit disclaimers.

## B. Manual live-device evidence (unchecked until a real-machine run)

- [x] Windowed playthrough on Arch Linux 7.2.11-arch1-1 (Vulkan):
  `cargo run -p collectathon --bin collectathon`; evidence: 2026-10-21
  live run finished at `frame=1634 presented=1634 skipped=0 x=46.0
  score=4` — collected quads disappeared as touched, HUD reached `4/4`,
  and reset restored them. Full table in `docs/release-notes/v0.1.0.md`
  ("Manual (live device — recorded 2026-10-21)").
- [x] Audio-device playback on Arch Linux 7.2.11-arch1-1 (PipeWire/ALSA):
  `audio device opened (audible backend)` in the log and `pickup.wav`
  audible on each of the four collections; evidence: 2026-10-21 live
  run. One transient `audio stream error: Buffer underrun/overrun
  occurred.` appeared with playback intact — accepted live-test
  observation (see `docs/release-notes/v0.1.0.md`).
  (decode-only `continuing headless` on silent machines is supported,
  not a failure).

## C. Release mechanics (policy stated, drift not resolved here)

- Branch/tag policy is `feature/<name> --PR--> dev --release--> stable`
  per `docs/development/git-workflow.md`; pushing the tag publishes
  `docs/release-notes/<tag>.md` via `.github/workflows/release.yml`.
  Missing notes now fail the workflow (no autogenerated fallback); the tag
  must match the workspace version and be reachable from `origin/stable`.

## D. v0.1.0 closure gate (2026-10-24, dev worktree)

- [x] Format, workspace build/tests, Clippy `-D warnings` green on dev (see Phase 9 verification).
- [x] `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --all-features` green locally and as the CI `rustdoc` gate.
- [x] Headless, server/client, collab, plugin, snapshot determinism covered by `release-gate` and unit tests.
- [x] Windowed playthrough recorded 2026-10-21 Arch Linux Vulkan (`frame=1634 presented=1634 skipped=0 x=46.0 score=4`).
- [x] Audio playback recorded with one transient `Buffer underrun/overrun` qualified as accepted observation.
- [ ] Manual window/audio re-evidence on the current candidate: the candidate differs behaviorally from the Oct 21 run (input retention, presenter lease), so the plan calls for a re-run — no display/audio device in this environment; needs a real-machine run.
- [x] Xvfb, llvmpipe Vulkan/render-ECS, and exact-commit evidence recorded on the release candidate: CI run 40101053516 on commit `9c5bcb5` (all 12 jobs success). Tag + stable-ancestry steps intentionally pending — no `v0.1.0` tag cut and nothing pushed to `stable` per maintainer direction; when cut, `v0.1.0` is the intended tag (existing `v0.0.1`/`v0.0.2` tags predate the release workflow). Do not cut `v0.1.0` until the maintainer gives the release go-ahead.
