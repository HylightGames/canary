# Changelog

All notable changes to Canary Engine are documented here, in the style of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Version numbers
follow the scheme defined in
[ADR 0006](docs/decisions/architecture-decision-records/0006-versioning-scheme.md),
not standard SemVer. This file records one entry per actual release; for
the more granular, dated story of how a release was built (including the
`-pre1` and architecture-review work that led into this one), see
[`docs/roadmap/milestones.md`](docs/roadmap/milestones.md).

## [v0.0.1] — 2026-08-03

The release that establishes Canary's engineering standards and
architectural core. Deliberately **not** a feature release — see
[`RELEASE_NOTES_v0.0.1.md`](RELEASE_NOTES_v0.0.1.md) for the full account
of what that means and why.

### Added

- Repository, git history, and governance: MIT license, `CONTRIBUTING.md`
  (including a DCO sign-off requirement), `CODE_OF_CONDUCT.md`,
  `SECURITY.md`, `GOVERNANCE.md` (succession/bus-factor planning and
  decision-making process), issue/PR templates, a minimal `CODEOWNERS`,
  and CI (cross-platform build matrix + `wasm32-wasip2` target check).
- Full documentation architecture under `docs/`: vision, architecture,
  decision records, roadmap, development, UI, research, and reviews.
- Twelve Architecture Decision Records: the ADR process itself; primary
  language selection (Rust); the two-tier plugin architecture (sandboxed
  WASM + trusted native); the rendering abstraction strategy; build
  tooling; the versioning scheme; the networking/multiplayer model;
  workspace crate versioning (lockstep); plugin ABI versioning and
  extensibility; component identity across the language boundary
  (`Proposed`, pending the `v0.0.2` archetype ECS work it depends on);
  `CanaryUI`, a native UI abstraction bootstrapped on `egui` and shared
  between the editor and game UI; and project state as an explicit,
  identifiable, versionable graph (`Proposed`), establishing a founding
  principle for Git-friendly, collaboration-ready project data.
- A senior architecture review and a living risk register tracking 31
  findings across the whole workspace.
- A Cargo workspace with five engine crates (`canary-core`,
  `canary-platform`, `canary-ecs`, `canary-plugin-api`, `canary-runtime`)
  and the `xtask` build-orchestration crate.
- A minimal, generational-index ECS (`canary-ecs`), explicitly documented
  as a placeholder for the target archetype-based design, hardened with
  `Send + Sync`-bounded component storage and a 64-bit generation counter.
- A `Plugin` trait and a working, **versioned** native (Tier B, C-ABI)
  plugin loader (`canary-plugin-api`) — an explicit ABI version field and
  a forward-extension hook, validated by a test that compiles a real C
  plugin with a deliberately wrong version and confirms it's rejected.
  The sandboxed WASM (Tier A) loader is designed but not yet implemented.
- A headless boot-harness binary (`canary-runtime`) exercising all of the
  above end to end.
- 16 passing tests across the workspace, including a `proptest` property
  test on ECS invariants and a real cross-language integration test (a C
  plugin, compiled with `gcc` at test time, loaded through the actual
  native ABI).

### Changed

- `xtask check` now detects whether `clippy` is available and runs it
  when present, instead of unconditionally skipping it — closing a gap
  where the local check could pass while CI's clippy gate still failed.
- CI no longer sets a blanket `RUSTFLAGS: "-D warnings"`, which applies to
  dependency compilation too and could fail builds over warnings this
  project doesn't control; lint enforcement is now correctly scoped via
  the workspace `[lints]` table and `cargo clippy -- -D warnings`.
- `rustfmt.toml` no longer specifies nightly-only options that silently
  didn't apply on this project's pinned `stable` toolchain.
- This milestone's own "definition of done" was revised: archetype ECS
  storage, the parallel job scheduler, the Tier A WASM loader, real
  windowing, and change detection are formally `v0.0.2`+ scope, not
  `v0.0.1` — see
  [`docs/roadmap/v0.0.1-roadmap.md`](docs/roadmap/v0.0.1-roadmap.md#definition-of-done-for-the-unqualified-v001--revised)
  for the reasoning.

### Fixed

- A pre-existing documentation/implementation mismatch in
  `docs/architecture/plugin-system.md` (referenced a `register` lifecycle
  hook that was never part of the `Plugin` trait).
- Already-committed code that didn't pass its own `cargo fmt --check`
  (whitespace-only; confirmed via a full rebuild and test pass).

[v0.0.1]: https://github.com/HylightGames/canary/releases/tag/v0.0.1
