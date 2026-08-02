# Changelog

All notable changes to Canary Engine are documented here, in the style of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Version numbers
follow the scheme defined in
[ADR 0006](docs/decisions/architecture-decision-records/0006-versioning-scheme.md),
not standard SemVer.

For milestone-level (rather than change-level) progress, see
[`docs/roadmap/milestones.md`](docs/roadmap/milestones.md).

## [v0.0.1-pre1] — Foundation

### Added

- Repository, git history, and governance: MIT license, `CONTRIBUTING.md`,
  `CODE_OF_CONDUCT.md`, `SECURITY.md`, issue/PR templates, CI workflow
  (cross-platform build matrix + `wasm32-wasip2` target check).
- Full documentation architecture under `docs/`: vision, architecture,
  decision records, roadmap, development, UI, and research documents.
- Seven initial Architecture Decision Records covering the ADR process
  itself, primary language selection (Rust), the plugin/modding
  architecture (two-tier: sandboxed WASM + trusted native), the rendering
  abstraction strategy (custom RHI, bootstrapped on `wgpu`), build tooling
  (Cargo workspace + `xtask`), the versioning scheme, and the networking/
  multiplayer model (server-authoritative, QUIC transport).
- Cargo workspace with five engine crates (`canary-core`,
  `canary-platform`, `canary-ecs`, `canary-plugin-api`, `canary-runtime`)
  and the `xtask` build-orchestration crate.
- A minimal, generational-index ECS (`canary-ecs`), explicitly documented
  as a placeholder for the target archetype-based design.
- A `Plugin` trait and working native (Tier B, C-ABI) plugin loader
  (`canary-plugin-api`); the sandboxed WASM (Tier A) loader is designed
  but not yet implemented.
- A headless boot-harness binary (`canary-runtime`) exercising the above
  end to end.

[v0.0.1-pre1]: https://github.com/notthecloudy/canary/releases/tag/v0.0.1-pre1
