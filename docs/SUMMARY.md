# Summary

[Introduction](index.md)

# Vision

- [Design Philosophy](vision/design-philosophy.md)
- [Long-Term Roadmap (Vision Level)](vision/long-term-roadmap.md)
- [Project Goals](vision/project-goals.md)

# Architecture

- [Asset System](architecture/asset-system.md)
- [Audio (`CanaryAudio`)](architecture/audio.md)
- [Core Runtime](architecture/core-runtime.md)
- [Engine Architecture Overview](architecture/engine-overview.md)
- [Execution Model](architecture/execution-model.md)
- [Input and Simulation Contract](architecture/input-and-simulation.md)
- [Live Collaboration: Authored Operations and Session History](architecture/live-collaboration.md)
- [Localization (`CanaryLoc`)](architecture/localization.md)
- [Networking & Multiplayer](architecture/networking.md)
- [Physics](architecture/physics.md)
- [Platform Abstraction](architecture/platform-abstraction.md)
- [Plugin System](architecture/plugin-system.md)
- [Rendering](architecture/rendering.md)
- [Runtime Composition and Consumer Lifecycle](architecture/runtime-composition.md)
- [Scripting System](architecture/scripting-system.md)
- [Project State, Versioning, and Collaboration](architecture/state-and-versioning.md)
- [Transform, `GlobalTransform`, and Hierarchy](architecture/transform.md)
- [UI Toolkit (`CanaryUI`)](architecture/ui-toolkit.md)

# Decisions (ADRs)

- [Architecture Decision Records](decisions/architecture-decision-records/README.md)
- [0001. Use Architecture Decision Records, in this format](decisions/architecture-decision-records/0001-record-format.md)
- [0002. Rust as the primary implementation language for the engine core](decisions/architecture-decision-records/0002-primary-language-selection.md)
- [0003. Two-tier plugin architecture: sandboxed WASM components + trusted native C ABI](decisions/architecture-decision-records/0003-plugin-and-modding-architecture.md)
- [0004. Rendering Hardware Interface: custom trait, bootstrapped on wgpu](decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md)
- [0005. Cargo workspace + `xtask` for build orchestration, no external build system](decisions/architecture-decision-records/0005-build-system-and-tooling.md)
- [0006. Versioning scheme: `v0.0.1-preN` → `v0.0.1` → ... → `v1.0.0`](decisions/architecture-decision-records/0006-versioning-scheme.md)
- [0007. Server-authoritative networking over QUIC, replication as an ECS concept](decisions/architecture-decision-records/0007-networking-and-multiplayer-model.md)
- [0008. Workspace crates version in lockstep, not independently](decisions/architecture-decision-records/0008-workspace-crate-versioning-lockstep.md)
- [0009. Plugin ABI versioning and forward-extensibility for the Tier B vtable](decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md)
- [0010. Component identity must be stable across the language boundary, not just within the Rust host](decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)
- [0011. `CanaryUI`: a UI abstraction from day one, bootstrapped on `egui`](decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md)
- [0012. Project state as an explicit, identifiable, versionable graph](decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md)
- [0013. Live collaboration is server-authoritative (client–server–client), not peer-to-peer or CRDT-based](decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md)
- [0014. Change detection as a shared primitive for replication, live collaboration, hot-reload, and subsystem decoupling](decisions/architecture-decision-records/0014-change-detection-as-shared-primitive.md)
- [0015. Localization format and key mechanism: Fluent via `fluent-rs`, a `LocKey` newtype, not a raw string](decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md)
- [0016. Rendering backends: native per-graphics-API crates, not a `wgpu` bootstrap](decisions/architecture-decision-records/0016-native-rendering-backends.md)
- [0017. A single, always-3D `Transform` representation for both 2D and 3D](decisions/architecture-decision-records/0017-unified-transform-representation.md)
- [0018. Asset handles: content-hashed IDs, generational handles, synchronous loading](decisions/architecture-decision-records/0018-asset-handles-and-synchronous-loading.md)
- [0019. Physics backend lineup: Rapier2D canonical 2D, Jolt canonical 3D, Rapier3D alternative](decisions/architecture-decision-records/0019-physics-backend-lineup.md)
- [0020. Pre-v0.3 architectural locks: what must be irreversible before the editor era](decisions/architecture-decision-records/0020-pre-v0-3-architectural-locks.md)
- [0021. Amendments to the pre-v0.3 locks: owner review corrections to ADR 0020](decisions/architecture-decision-records/0021-amendments-to-pre-v0-3-locks.md)
- [0022. Constitution clarifications and red-team amendments (second pass over ADR 0020/0021)](decisions/architecture-decision-records/0022-constitution-clarifications-and-red-team.md)
- [0023. v0.0.12 audio bootstrap: rodio behind a custom-first trait](decisions/architecture-decision-records/0023-audio-bootstrap-rodio-behind-custom-trait.md)
- [0024. Reusable game-runtime composition and scoped plugin access](decisions/architecture-decision-records/0024-reusable-runtime-composition.md)
- [0025. Route deterministic gameplay actions through one UI-aware input path](decisions/architecture-decision-records/0025-deterministic-input-actions-and-ui-capture.md)
- [0026. Keep authored project state and simulation snapshots separate](decisions/architecture-decision-records/0026-authored-state-and-simulation-snapshot-contract.md)
- [0027. Start networking with bounded reliable replication and server input authority](decisions/architecture-decision-records/0027-minimal-server-authoritative-replication.md)
- [0028. Authoritative live-collaboration operations](decisions/architecture-decision-records/0028-authoritative-live-collaboration-operations.md)

# Roadmap

- [Roadmap and contributor handoff](roadmap/README.md)
- [v0.0.1 Roadmap](roadmap/v0.0.1-roadmap.md)
- [v0.0.2 Roadmap](roadmap/v0.0.2-roadmap.md)
- [`v0.0.3` Roadmap: Tier A (Sandboxed WASM) Plugin Loading](roadmap/v0.0.3-roadmap.md)
- [`v0.0.4` Roadmap: Real Windowing](roadmap/v0.0.4-roadmap.md)
- [`v0.0.5` Roadmap: Localization (`canary-loc`)](roadmap/v0.0.5-roadmap.md)
- [`v0.0.6` Roadmap: Rendering Bootstrap (RHI + `canary-render-vulkan`)](roadmap/v0.0.6-roadmap.md)
- [`v0.0.7` Roadmap: ECS Execution Model (Queries, Resources, `Tick` width)](roadmap/v0.0.7-roadmap.md)
- [`v0.0.8` Roadmap: ECS Scheduler (`canary-scheduler`)](roadmap/v0.0.8-roadmap.md)
- [`v0.0.9` Roadmap: Real-Time Loop + `Transform` + ECS-Driven Rendering](roadmap/v0.0.9-roadmap.md)
- [`v0.0.10` Roadmap: Minimal Asset Loading (`canary-assets`)](roadmap/v0.0.10-roadmap.md)
- [`v0.0.11` Roadmap: 2D Physics (`canary-physics`)](roadmap/v0.0.11-roadmap.md)
- [`v0.0.12` Roadmap: Audio Playback (`canary-audio`)](roadmap/v0.0.12-roadmap.md)
- [`v0.0.13` Roadmap: Windowed Game UI and Shared Input](roadmap/v0.0.13-roadmap.md)
- [`v0.1.0` Plan: A Real, Small Game Against Canary's Runtime](roadmap/v0.1.0-plan.md)
- [Future Roadmap (Beyond `v0.1.0`)](roadmap/future-roadmap.md)
- [Milestones](roadmap/milestones.md)
- [Project Status](roadmap/status.md)

# Development

- [Benchmarking](development/benchmarking.md)
- [Build System](development/build-system.md)
- [Coding Standards](development/coding-standards.md)
- [Git Workflow](development/git-workflow.md)
- [Localization pipeline (phase 1)](development/localization-pipeline.md)
- [Repository Structure](development/repository-structure.md)

# UI design

- [Editor Design (Target — Not Yet Built)](ui/editor-design.md)
- [UX Principles](ui/ux-principles.md)

# Research

- [Distribution-ecosystem audit (2026-10-03)](research/distribution-audit.md)
- [egui → Canary RHI integration](research/egui-rhi-integration.md)
- [Engine Comparisons](research/engine-comparisons.md)
- [Technology Evaluations](research/technology-evaluations.md)

# Distribution

- [Channels and identifiers](distribution.md)

# Reviews

- [Architecture Reviews](reviews/README.md)
- [v0.0.2 Release Checklist](reviews/v0.0.2-RELEASE_CHECKLIST.md)
- [`v0.0.12` Release Checklist](reviews/v0.0.12-RELEASE_CHECKLIST.md)
- [`v0.0.13` Release Checklist](reviews/v0.0.13-RELEASE_CHECKLIST.md)
- [`v0.0.14` Release Checklist](reviews/v0.0.14-RELEASE_CHECKLIST.md)
- [Senior Architecture Review — August 2026](reviews/2026-08-senior-architecture-review.md)
- [API Review: R-34 Scoped Tier A World Access (ADR 0024 Acceptance Gate)](reviews/2026-09-r34-api-review.md)
- [External architecture review triage — Review #3 (replaceable subsystems)](reviews/triage/2026-09-review-3-triage.md)
- [External architecture review triage — Review #4 ("pre-v0.2.0")](reviews/triage/2026-09-review-4-triage.md)
- [External architecture review triage (September 2026)](reviews/triage/2026-09-review-triage.md)
- [2026-09 Ultrareview Hardening Pass](reviews/2026-09-ultrareview-hardening.md)
- [Canary Engine release-readiness review](reviews/2026-10-11-release-readiness-review.md)
- [Pre-v0.1.0 readiness audit](reviews/2026-10-v0.1.0-readiness-audit.md)
- [v0.0.1 Release Checklist](reviews/RELEASE_CHECKLIST.md)
- [Risk Register](reviews/risk-register.md)

# Release notes

- [Canary Engine `v0.0.1` — Release Notes](release-notes/v0.0.1.md)
- [Canary Engine `v0.0.2` — Release Notes](release-notes/v0.0.2.md)
- [Canary Engine `v0.0.3` — Release Notes](release-notes/v0.0.3.md)
- [Canary Engine `v0.0.4` — Release Notes](release-notes/v0.0.4.md)
- [Canary Engine `v0.0.5` — Release Notes](release-notes/v0.0.5.md)
- [Canary Engine `v0.0.6` — Release Notes](release-notes/v0.0.6.md)
- [Canary Engine `v0.0.7` — Release Notes](release-notes/v0.0.7.md)
- [Canary Engine `v0.0.8` — Release Notes](release-notes/v0.0.8.md)
- [Canary Engine `v0.0.9` — Release Notes](release-notes/v0.0.9.md)
- [Canary Engine `v0.0.10` — Release Notes](release-notes/v0.0.10.md)
- [Canary Engine `v0.0.11` — Release Notes](release-notes/v0.0.11.md)
- [Canary Engine `v0.0.12` — Release Notes](release-notes/v0.0.12.md)
- [Canary Engine `v0.0.13` — Release Notes](release-notes/v0.0.13.md)
- [Canary Engine `v0.0.14` — Release Notes](release-notes/v0.0.14.md)
- [Canary Engine `v0.0.15` — Release Notes](release-notes/v0.0.15.md)
- [Canary Engine `v0.0.16` — Release Notes](release-notes/v0.0.16.md)
- [Canary Engine `v0.1.0` — Release Notes](release-notes/v0.1.0.md)

