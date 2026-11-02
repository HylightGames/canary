# Capability-growth roadmap principles

> **Status: proposed planning direction, not an accepted ADR.** Recorded from the
> project owner's direction on 2026-10-24 during the
> [`v0.1.0` final review](../reviews/2026-10-24-v0.1.0-final-review.md). It amends
> how future milestones are *planned and accepted*; it does not change any
> accepted architectural decision and it does not mark any work implemented.
> Older milestone history in [`milestones.md`](milestones.md) and
> [`status.md`](status.md) is deliberately left as written.

## The principle

**Every release must expand what developers can actually build, while
reliability, usability, performance, security, and production maturity grow
alongside those capabilities.**

Canary must not spend its early life becoming an immaculate little engine that
can only make little games. Reliability matters because capabilities must work;
it is not a substitute for capability growth. Equally, a long list of shallow
features is not a capable engine.

The previous planning style put most of its weight on polishing the current
foundations and unlocking features progressively later. That produced a very
well-tested integration proof (`v0.1.0`) and also the gaps recorded in the
[`v0.2.0` roadmap](v0.2.0-roadmap.md#what-v010-does-and-does-not-give-a-developer):
no engine-owned renderer pipeline or camera, no collision events or queries, no
animation, no hot reload, no inspector or profiler, and loopback-only
networking. The rules below exist to prevent the next five releases from
repeating that shape.

### Rules every milestone plan must satisfy

1. **Every release grows developer-facing capability.** A plan whose visible
   result is only "more reliable" or "better documented" is incomplete.
2. **Reliability, usability, performance, security, and production maturity
   advance in the same release as the capability they protect**, not in a later
   hardening phase.
3. **Major releases advance several capability pillars at once**, and the
   pillars are planned as one connected system (see the three roadmaps below).
4. **Capabilities mature through four levels** (Exists, Integrated, Usable,
   Production-capable). Placeholders do not count as capabilities.
5. **High-end and studio workflows are designed for early.** Large worlds,
   advanced animation and rendering, serious asset pipelines, profiling, and
   production workflows get their boundaries and dependencies (ADRs, data
   models, extension points) planned a release or two before their
   implementation is due, without building them prematurely.
6. **Release acceptance rests on demonstrated capabilities and evidence**, never
   on feature counts or on "all tasks merged".
7. **Distinguish proposal from decision.** Planned work is never recorded as
   implemented; accepted ADRs are never silently contradicted (amend or
   supersede them).

## Seven capability pillars

The pillars are not silos. Their dependencies are what make a capability useful.
The pillar list may be extended or adjusted when repository evidence justifies it.

| # | Pillar | Covers | Current anchors |
|---|--------|--------|-----------------|
| 1 | Gameplay and world systems | ECS, lifecycle, scheduling, world composition, reusable objects, scene hierarchy, save/load, events, deterministic simulation, scripting and extensions | `canary-ecs`, `canary-scheduler`, `canary-transform`, `canary-runtime`, `canary-state` |
| 2 | Rendering and graphics | 2D/3D pipelines, geometry, materials, cameras, lighting, shadows, render graphs, shader tooling, skeletal rendering, particles, post-processing, GPU resource management, scalable quality | `canary-render`, `canary-render-vulkan`, `canary-render-ecs` |
| 3 | Physics, animation, and simulation | 2D/3D collision, queries, events, rigid bodies, joints, character controllers, skeletal animation, state machines, blending, IK, ragdolls, soft bodies, navigation | `canary-physics` (2D only) |
| 4 | Assets and content production | Stable identities, dependency tracking, import pipelines, validation, hot reload, versioning, cooking, compression, streaming, async loading, memory budgets, platform builds, extensible importers | `canary-assets` |
| 5 | Editor, tooling, and iteration | Editor foundations, scene/prefab workflows, inspector, viewport, asset browser, live inspection, logging, debugging, profiling, automated game testing, collaboration, diagnostics | `canary-collab` (first slice), `xtask`, `tracing` |
| 6 | Audio, UI, input, and networking | Gameplay audio, UI layout and interaction, localization, accessibility, input across devices, authoritative multiplayer, replication, reconnect and failure behavior, runtime diagnostics | `canary-audio`, `canary-ui-core`, `canary-ui-egui`, `canary-loc`, `canary-input`, `canary-net`, `canary-platform` |
| 7 | Production engineering and platform support | Stable APIs, plugin compatibility, crash diagnostics, build and packaging, memory and performance budgets, regression detection, migration, reproducible builds, supported-platform criteria, long-term maintenance | CI workflows, `canary-plugin-api`, `docs/reviews/`, release workflow |

## Four maturity levels

Track every important capability at one of four levels. A capability may enter at
level 1 and mature over several releases.

| Level | Name | Meaning | Evidence required to claim it |
|-------|------|---------|-------------------------------|
| 1 | **Exists** | The basic implementation is present and works under defined conditions. | Passing tests for the defined conditions; documented limits. |
| 2 | **Integrated** | It composes naturally with the other engine subsystems. | A cross-subsystem proof (sample or integration test) using only public APIs; no private harness. |
| 3 | **Usable** | Developers can configure, author, inspect, debug, and use it through documented workflows. | A documented workflow a new developer followed successfully; diagnostics and error messages that name the cause; no example-owned engine work required. |
| 4 | **Production-capable** | Correctness, performance, failure behavior, compatibility, and maintainability have been exercised against representative workloads. | Budgeted benchmarks on representative content; fault-injection and soak results; compatibility and migration tests; a threat model where untrusted input exists. |

Never count a placeholder, a stub trait, or a type that nothing consumes as
level 1. "Skeletal data type exists" is not "skeletal animation exists": imported
skeletons, playback, blending, character integration, rendering, inspection and
performance must connect through a usable workflow.

## Three connected roadmaps

Planning is done on three linked views, and **every release advances all three**.

- **Capability roadmap:** what developers will be able to build, and which major
  subsystems or features make that possible.
- **Integration roadmap:** the APIs, dependencies, runtime composition, content
  flows, and cross-system work needed to make those capabilities useful together
  (for example: asset reference, loading, lifetime, then rendering or audio use).
- **Production-readiness roadmap:** correctness, safety, security, compatibility,
  performance, testing, documentation, debugging, asset pipeline, packaging, and
  professional workflow.

Each milestone document should therefore contain a capability section, an
integration section, and a production-readiness section, with explicit links
between them. The [`v0.2.0` roadmap](v0.2.0-roadmap.md) is the first to do so.

## Guiding capability targets

These are targets that guide prioritization, not inflexible promises that a
feature must land at a particular version.

| Version | Target |
|---------|--------|
| `v0.1.0` | A real, minimal game engine: a developer can build and run a complete small game. The editor may be absent and visuals basic, but essential gameplay systems exist and work together. |
| `v0.2.0` | A broadly useful game-development platform; the first major breadth expansion across gameplay, physics, rendering, assets, animation foundations, audio, UI, input, world composition, and developer workflows. Several kinds of games are practical, not variations of one demo. |
| `v0.3.0` | A capable, cohesive engine: systems connect into a stronger workflow; advanced capabilities expand; non-trivial games stop requiring workarounds for missing fundamentals. |
| `v0.4.0` | An advanced engine: more capable rendering and animation, large-world techniques, stronger profiling, simulation, networking, production asset workflows, and scalable architecture. |
| `v0.5.0` | A production-capable platform: a serious team can build, test, profile, package, ship, and maintain a substantial game; remaining gaps are mostly project-specific. |
| `v0.6.0` | A studio-dependable, high-end engine: a studio can seriously consider Canary for a demanding commercial game, potentially AAA-scale. Requires advanced systems, performance tools, stable workflows, scalable architecture, professional tooling and real production evidence. A feature checklist alone is insufficient. |
| `v0.7.0`–`v1.0.0` | Advanced studio workflows, complex simulation, VR, high-end production, cinematic capability, platform support, API stability, migration, compatibility, long-term dependability. `v1.0.0` means the core platform is mature, not that development has ended. |

## Maturity snapshot at the `v0.1.0` candidate

Levels are assigned conservatively from the evidence gathered in the
[final review](../reviews/2026-10-24-v0.1.0-final-review.md) (audited commit
`ebc8f7f4103de6a8adfa3ae8f7bc25f778ed97c5`). "Not present" means no engine-owned
implementation was found; example-owned code does not count. No capability is
Production-capable yet: no representative workload has been exercised against
declared performance budgets.

| Capability | Pillar | Level | Evidence | What moves it up |
|------------|--------|-------|----------|------------------|
| ECS worlds, queries, resources | 1 | Integrated | Used by the sample; reference-model chaos test (proposed) found no divergence; `Entity::from_raw_parts` forgery is rejected | Declared budgets for the existing 1,000 and 10,000 entity benchmarks; ergonomic change filters on the existing tick baseline; events |
| Stage scheduler | 1 | Integrated | Registration order is the ordering mechanism; access declarations exist | Explicit ordering constraints; parallel execution |
| Transform hierarchy | 1 | Integrated | `canary-transform`, used by sample | Large-hierarchy benchmarks; world-origin strategy |
| Frame driver and fixed-step contract | 1 | Integrated | Existing tests plus the proposed randomized frame-sequence test; accumulator is example-owned | Engine-owned fixed-step driver and interpolation |
| World composition (authored docs, one-level prefabs) | 1 | Exists | `canary-state` authored documents; sample loads a room | Scenes, nested prefabs, spawn API, additive load |
| Save/load and deterministic snapshots | 1 | Integrated | Snapshot round trip and determinism proofs | Load budgets, depth bound, migration tests |
| Vulkan RHI and window presentation | 2 | Integrated | Real-window and llvmpipe proofs pass; lifecycle gates; one OS family only | Multi-OS evidence, validation-layer CI, device-loss tests |
| Scene rendering | 2 | Exists | CPU-baked vertices; fixed virtual camera on the perspective path; `OrthoView` for 2D | GPU instancing, camera component, materials, lighting |
| Shaders and materials | 2 | Not present | Sample owns `ScenePipeline` and WGSL compile via `naga` | Engine-owned pipelines and a shader asset path |
| 2D rigid-body physics | 3 | Integrated | `canary-physics` over Rapier2D; structural reconciliation tests | Collision events, queries, joints, character controller |
| 3D physics | 3 | Not present | Deferred by ADR 0019 scope | Backend trait extension and a 3D backend |
| Animation (sprite or skeletal) | 3 | Not present | No engine-owned implementation | Data model ADR, playback, state machines |
| Asset store and loaders (PNG, GLB, WAV) | 4 | Integrated | Budgeted, typed errors, root confinement | Import pipeline, hot reload, dependency tracking, async loading |
| Runtime diagnostics | 5 | Exists | `tracing` logs only | Inspector, profiler, replay |
| Live collaboration operations | 5 | Exists | Server-authoritative first slice with permissions and conflict rules | An editor or tool consuming it |
| Audio | 6 | Integrated | Rodio bootstrap backend, triggers, silent fallback | Buses, spatialization, streaming |
| UI (egui adapter, capture routing) | 6 | Integrated | HUD in the sample; capture routing tests | Menu kit, focus navigation, accessibility |
| Localization | 6 | Integrated | Fluent bundles with fallback; CI gate is vacuous for the real files (finding CANARY-REL-010) | A gate that checks the shipped locale files |
| Input (actions, bindings, keyboard and mouse) | 6 | Integrated | Mapper, capture routing, edge retention tests | Gamepad, rebinding persistence, text input |
| Networking | 6 | Integrated | QUIC server-authoritative replication, loopback only, no client authentication | Authenticated non-loopback sessions, prediction |
| Plugins, native (Tier B) | 7 | Exists | Trusted loader with path confinement and ABI check | Capability surface, compatibility policy |
| Plugins, WASM (Tier A) | 7 | Exists | Fuel and per-memory limits; aggregate and table limits missing (finding CANARY-REL-008) | Complete resource budget, host API growth |
| CI and release engineering | 7 | Integrated | 12-job CI plus audit, Trivy and CodSpeed; publish path never exercised | Release dry run, approval gate, evidence check |
| Packaging and consumer API | 7 | Exists | Git or path dependency; no facade crate | Facade crate, template, crates.io readiness |
| Performance budgets | 7 | Not present | Benchmarks exist and are informational only | Declared budgets with blocking thresholds |

## How to use this document

- When writing a milestone plan, state for each capability the current level, the
  target level, and the evidence that will prove it (see the
  [`v0.2.0` roadmap](v0.2.0-roadmap.md) for the expected shape).
- When closing a milestone, update the snapshot above with evidence, and do not
  raise a level without the evidence listed for it.
- When a boundary for a future capability is needed early (render graph, asset
  identity and cooking, component reflection, parallel scheduling, world
  streaming, animation data model, telemetry), write the ADR as a **Proposed**
  decision first; do not build the full system in advance.
