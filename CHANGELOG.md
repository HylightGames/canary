# Changelog

All notable changes to Canary Engine are documented here, following the principles of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Canary's version numbers follow the project's versioning scheme defined in [ADR 0006](docs/decisions/architecture-decision-records/0006-versioning-scheme.md), rather than standard Semantic Versioning.

This file records **meaningful changes between released versions**. It intentionally does not reproduce the full development history of each release. For dated milestones, pre-releases, architecture reviews, and the work that led to each release, see [`docs/roadmap/milestones.md`](docs/roadmap/milestones.md).

## [v0.0.2] — 2026-08-18

### Archetype ECS foundation

`v0.0.2` replaces the placeholder ECS storage from `v0.0.1` with Canary's intended archetype-based foundation.

The release is deliberately focused: establish the ECS storage and query model needed for the next stage of engine development without prematurely expanding into unrelated subsystems.

### Added

* **Archetype-based ECS storage** — entities with the same component signature are stored together in contiguous archetypes with packed component columns.
* **Cached queries** — queries use maintained archetype indexes instead of rescanning every archetype on each invocation.
* **Change detection** — queries can filter components changed since a specific world tick, including across archetype transitions and row relocation.
* **Component identity across the language boundary** — the first implementation of [ADR 0010](docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md), introducing stable schema identity through `CanaryComponent::SCHEMA_ID` and component registration.
* **Expanded ECS validation** — the `canary-ecs` test suite grew from 6 to 19 tests, including archetype transition edge cases and property-based testing of arbitrary insert, remove, and despawn sequences.

### Changed

* The ECS architecture documented in [`docs/architecture/core-runtime.md`](docs/architecture/core-runtime.md) was updated to reflect the implemented archetype model rather than the previous placeholder design.
* The `v0.0.2` roadmap was updated to record the completed scope.
* [ADR 0010](docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md) moved from **Proposed** to **Accepted**.

### Fixed

* Resolved the ECS limitations previously tracked for change detection and component identity.
* No existing ECS API changes were required; the public `World::insert`, `get`, and `query` interfaces remain compatible with the previous implementation.

[v0.0.2]: https://github.com/HylightGames/canary/releases/tag/v0.0.2

## [v0.0.1] — 2026-08-03

### Engineering and architectural foundation

`v0.0.1` established the repository, engineering standards, architectural decision process, and initial runtime foundations for Canary.

It was intentionally **not a feature-complete engine release**. Its purpose was to establish the structure from which later engine systems could be built.

### Added

* **Project governance and contribution infrastructure** — MIT licensing, contribution and conduct policies, security reporting, governance and succession planning, issue and pull request templates, `CODEOWNERS`, and CI.
* **Documentation architecture** — vision, architecture, ADRs, roadmap, development guidance, UI documentation, research, reviews, and the project risk register.
* **Architecture Decision Records** — the initial ADR set covering project fundamentals including Rust, plugin architecture, rendering abstraction, build tooling, versioning, networking, workspace versioning, plugin ABI design, component identity, `CanaryUI`, and versionable project state.
* **Architecture review and risk tracking** — a senior architecture review and living risk register covering 31 findings across the workspace.
* **Initial Cargo workspace** — five engine crates (`canary-core`, `canary-platform`, `canary-ecs`, `canary-plugin-api`, and `canary-runtime`) plus the `xtask` build-orchestration crate.
* **Generational ECS foundation** — a minimal `canary-ecs` implementation with generational entity IDs and thread-safe component storage, explicitly serving as the placeholder for the later archetype design.
* **Versioned native plugin ABI** — a working Tier B C-ABI plugin loader with explicit ABI versioning and forward-extension support, including cross-language rejection testing.
* **Headless runtime boot harness** — `canary-runtime` exercising the initial engine foundations end to end.
* **Workspace test coverage** — 16 passing tests, including ECS property testing and a real C-plugin integration test compiled and loaded through the native ABI.
* **WASM plugin architecture design** — the Tier A sandboxed plugin model was defined, although its loader was not yet implemented.

### Changed

* `xtask check` now detects and runs Clippy when available instead of silently skipping it.
* CI linting was narrowed from a global `RUSTFLAGS: "-D warnings"` policy to explicit workspace Clippy enforcement, avoiding warnings originating in dependencies the project does not control.
* `rustfmt.toml` was aligned with the project's pinned stable toolchain by removing nightly-only configuration.
* Several originally planned systems were explicitly moved to `v0.0.2` and later scope, including archetype ECS storage, the parallel job scheduler, the Tier A WASM loader, real windowing, and change detection.

### Fixed

* Corrected a documentation mismatch in [`docs/architecture/plugin-system.md`](docs/architecture/plugin-system.md) referencing a `register` lifecycle hook that did not exist in the `Plugin` trait.
* Corrected pre-existing formatting failures so the repository passes its own formatting checks.

[v0.0.1]: https://github.com/HylightGames/canary/releases/tag/v0.0.1
