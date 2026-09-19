# Changelog

All notable changes to Canary Engine are documented here, following the principles of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Canary's version numbers follow the project's versioning scheme defined in [ADR 0006](docs/decisions/architecture-decision-records/0006-versioning-scheme.md), rather than standard Semantic Versioning.

This file records **meaningful changes between released versions**. It intentionally does not reproduce the full development history of each release. For dated milestones, pre-releases, architecture reviews, and the work that led to each release, see [`docs/roadmap/milestones.md`](docs/roadmap/milestones.md).

## [Unreleased]

`v0.0.3` through `v0.0.8` are all implemented (see
[`docs/roadmap/status.md`](docs/roadmap/status.md) for current,
authoritative status) but not yet formally tagged — each has its own
roadmap document with full scope, verification, and definition-of-done
detail; this section summarizes rather than duplicates them.

### Added

* **Sandboxed WebAssembly Component plugins (Tier A)** — `canary-plugin-api`'s `wasmtime`-backed loader, with structural capability enforcement (`ReadEcsWorld`/`WriteEcsWorld`), a resource budget, and a versioned ECS data ABI across the host/guest boundary. (`v0.0.3`)
* **Real windowing** — `canary-platform`'s `winit`-backed backend, behind an off-by-default `winit-backend` feature; a real OS window, real keyboard/mouse input, and a genuine close signal. (`v0.0.4`)
* **Localization** — `canary-loc`: Fluent-backed message bundles, compile-time-checked `LocKey`/`key!` identifiers, real fallback-chain resolution, and tracing events for missing keys/fallbacks. (`v0.0.5`)
* **Rendering bootstrap** — the `canary-render` RHI trait (confirmed zero dependencies) and its first backend, `canary-render-vulkan` (via `ash`), proven with a real offscreen triangle render: WGSL compiled to SPIR-V via `naga`, rendered and read back on a real (if software) Vulkan device. (`v0.0.6`)
* **Multi-component queries and typed resources** — `World::query2`/`query3`/`query2_mut`, and `World::insert_resource`/`resource`/`resource_mut`, alongside the existing single-component `World::query`. `Tick` widened from `u32` to `u64` to close a real wraparound risk. New `docs/architecture/execution-model.md` design document. (`v0.0.7`)
* **ECS scheduler** — a new crate, `canary-scheduler`: `SystemAccess` (declared component/resource reads and writes) and `Schedule` (stage-based execution, running non-conflicting read-only systems concurrently). (`v0.0.8`)
* **Real-time loop** — `Subsystem::tick` takes a real delta-time and `App::run` drives subsystems off the wall clock (alongside deterministic fixed-`dt` `run_for`), proven with sleep-based timing tests. (`v0.0.9`)
* **Spatial transforms** — a new crate, `canary-transform`: single always-3D `Transform` (see ADR 0017), cached `GlobalTransform`, `Parent`/`Children` hierarchy with a scheduler-registered propagation system; `glam` as the project's graphics math with zero transitive dependencies. (`v0.0.9`)
* **ECS-driven rendering** — a new crate, `canary-render-ecs`, reading `GlobalTransform` plus a minimal `Renderable` out of the `World` and drawing through the unchanged RHI (CPU-bake, proven shader reused verbatim), with real-pixel readback tests gating in a dedicated CI job; `spinning-cube` rewritten on top of it. (`v0.0.9`)

### Changed

* Two external architecture reviews (September 2026) were triaged against the actual codebase — see [`docs/decisions/2026-09-review-triage.md`](docs/decisions/2026-09-review-triage.md) — resolving `v0.0.7`'s and `v0.0.8`'s scope rather than picking from `future-roadmap.md`'s previously-open options.
* [ADR 0016](docs/decisions/architecture-decision-records/0016-native-rendering-backends.md) superseded [ADR 0004](docs/decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md)'s original `wgpu`-backed RHI plan with native, per-graphics-API backend crates instead.

### Fixed

* A real cargo-audit finding: `wasmtime`/`wasmtime-wasi` bumped to close 18 RustSec advisories (including two critical sandbox-escape bugs), and `wayland-scanner` bumped to close two more via a transitive `quick-xml` dependency.
* Several real CI gaps found and fixed during the same pass: a Vulkan-dependent test that ran unguarded on every CI platform, a Windows-only compile failure from unscoped Wayland dependencies, and `canary-plugin-api`'s host-only loader code being incorrectly checked against the `wasm32-wasip2` target.
* A macOS-only CI flake: replaced a wall-clock timing assertion in the scheduler's concurrency test with a deterministic overlap proof after loaded runners exceeded it twice through pure scheduling jitter. (`v0.0.9`)
* Removed the unused `wasmtime-wasi` dependency (zero references in source; the lockstep-pin lesson is preserved in comments for when a real call site lands). (`v0.0.9`)

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
