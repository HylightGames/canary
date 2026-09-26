# Long-Term Roadmap (Vision Level)

This is the narrative direction of the project, not a date-based delivery
promise. The live inventory and contributor starting point are in
[`docs/roadmap/`](../roadmap/README.md); the dependency-ordered path through
`v0.1.0` is [`v0.1.0-plan.md`](../roadmap/v0.1.0-plan.md), and work beyond
that target is in [`future-roadmap.md`](../roadmap/future-roadmap.md).

## Era 1 — Foundation (complete, `v0.0.1`)

Repository, governance, documentation architecture, ADR process, and a
headless, compiling skeleton: logging, a minimal ECS, plugin trait surface,
and build/test/CI pipeline. The deliverable was a foundation other engineers
can build on without re-litigating the basics. It shipped as `v0.0.1`; see
[`v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md) and
[`v0.0.1 release notes`](../release-notes/v0.0.1.md).

## Era 2 — Core engine foundations (implemented; integration continues)

The archetype ECS, typed resources, stage scheduler, real window backend,
versioned native plugin ABI, and sandboxed WASM component loader have all
landed in `v0.0.2`–`v0.0.8`. The foundation is real; it is not a claim that a
game developer already has a complete reusable runtime or action-input API.
The runtime composition surface, live-world plugin access, and shared
`RawInput → InputAction → SimulationInput` path are current dependencies of
`v0.0.13`. See [`status.md`](../roadmap/status.md) and the
[`v0.1.0 plan`](../roadmap/v0.1.0-plan.md).

## Release cadence and the `v0.1.0` target

The project uses small, dependency-ordered `0.0.x` milestones to make
architectural changes reviewable and correctable. A milestone may include the
minimum adjacent work needed to prove its primary feature end to end; it must
not grow unrelated systems just to fill out a checklist.

The `v0.1.0` bar, recorded at the project owner's direction in September 2026,
is **a competent developer can write a real, small game directly against
Canary's supported runtime without bypassing the engine or implementing
fundamental engine systems**. It is an integration proof, not a promise that
every architecture document is fully implemented or that the API is stable.
The remaining sequence is `.13` windowed `CanaryUI` and runtime composition,
`.14` project state and simulation snapshots, `.15` first networking slice,
`.16` first live-collaboration slice, then the sample-game integration in
`.1.0`. The
[`plan`](../roadmap/v0.1.0-plan.md) contains the acceptance conditions and
explicitly deferred work.

This deliberately leaves the editor, visual scripting, marketplace,
full asset cooking/hot reload, 3D physics, and prediction/rollback beyond
`v0.1.0`. Their absence does not block that release target.

## Era 3 — Rendering and content (in progress)

The native RHI and first Vulkan backend shipped in `v0.0.6`; ECS extraction,
file-loaded meshes/textures, and sound assets followed in `v0.0.9`–`.12`.
The near-term target is a windowed presentation path and an integrated small
game, not a complete AAA renderer. Render graph, broad material/shader
support, second graphics backends, cooking/cache/streaming, and hot reload
remain later work with explicit consumer triggers. The RHI uses native
per-graphics-API backends per
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md),
which superseded the original `wgpu` backend choice in ADR 0004.

## Era 4 — State, networking, and live systems (first slices before `v0.1.0`)

The foundations are recorded, but the state, networking, and collaboration
subsystems do not yet exist in code. The `.14`–`.16` milestones deliberately
pull minimal versions of these capabilities forward so `v0.1.0` proves
integration. They do not attempt to deliver mature rollback netcode or a
complete multi-user production workflow. Stable authored identity and
versioned project data, deterministic snapshots, removal history, canonical
state order, and server authority are the contracts to establish first;
their exact acceptance criteria are in the
[`near-term plan`](../roadmap/v0.1.0-plan.md).

## Era 5 — Editor and authoring tools (after `v0.1.0`)

The editor is a first-party consumer of Canary's game/runtime APIs and a
dogfood plugin host, not a parallel game runtime. Start only after supported
runtime composition, project save/reload, windowed `CanaryUI`, and safe access
to the active game world are proven. The first useful editor slice should open
a project, inspect and edit a scene, show a live viewport, save/reload, and
build/run through a headless command-line path.

Further authoring tools include a mature asset browser/import pipeline,
validation and profiling views, script debugging, and eventually visual
scripting. Visual scripting should author programs for the same WebAssembly
Component Model boundary as textual scripts; it must not create a second
runtime. Resolve fast edit/compile feedback before implementation (risk R-17).
See [`docs/ui/editor-design.md`](../ui/editor-design.md) and the
[`future roadmap`](../roadmap/future-roadmap.md).

## Era 6 — Ecosystem and marketplace (after the editor dogfoods plugins)

The WASM plugin tier already exists internally; public distribution is not
ready merely because a loader exists. Before marketplace work, design package
manifests and compatibility ranges, capability review, provenance/signing,
dependency handling, safe unload/replacement, and conflict rules for multiple
plugins providing the same role. Marketplace tooling should make trust and
permissions visible before executing third-party code. See risks R-08, R-09,
R-19, R-26, and R-35 in the
[`risk register`](../reviews/risk-register.md).

## Era 7 — Hardening toward `v1.0.0`

API stabilization, backward-compatibility audits, performance work against
real workloads, and validation across supported platforms. `v1.0.0` means a
real compatibility commitment under
[ADR 0006](../decisions/architecture-decision-records/0006-versioning-scheme.md),
so this era begins when the APIs and workflows that consumers depend on have
evidence behind them, not on a calendar date.

## Work without an assigned era

AI/ML inference (`canary-ai`), first-party tools for inspecting/driving a live
engine session, GPU-accelerated physics, and additional desktop/mobile/console
platforms are real possibilities, but need a concrete workload, contributor,
or supported platform before they become a milestone. The
[`future roadmap`](../roadmap/future-roadmap.md) records their current triggers
and open questions without inventing dates.
