# Changelog

All notable changes to Canary Engine are documented here, following the principles of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Canary's version numbers follow the project's versioning scheme defined in [ADR 0006](docs/decisions/architecture-decision-records/0006-versioning-scheme.md), rather than standard Semantic Versioning.

This file records **meaningful changes between released versions**. It intentionally does not reproduce the full development history of each release. For dated milestones, pre-releases, architecture reviews, and the work that led to each release, see [`docs/roadmap/milestones.md`](docs/roadmap/milestones.md).

## [Unreleased]

`v0.0.3` through `v0.0.13` are all implemented (see
[`docs/roadmap/status.md`](docs/roadmap/status.md) for current,
authoritative status) and tagged — `v0.0.1`/`v0.0.2` by release cuts,
`v0.0.12`/`v0.0.13` by release cuts, `v0.0.3`–`v0.0.11` by backfill tags
on their implementation records (2026-09-28; see `milestones.md`). Each
has its own
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
* **Minimal asset loading** — a new crate, `canary-assets`: `AssetId` (opaque content hash of file bytes plus loader version), `AssetHandle<T>` (generational handle mirroring `Entity`), `AssetStore<T>` (generational slots kept as an ECS resource, where stale handles resolve to `None`), and `AssetError` (typed failures with path context). (`v0.0.10`)
* **GLB mesh and PNG texture loaders** — synchronous, path-based, with checked-in fixtures and known-value assertions; corrupt, truncated, over-budget, and missing inputs fail with typed errors, never panics. (`v0.0.10`)
* **File-loaded meshes through the unchanged RHI** — `MeshRenderable` plus index-to-soup expansion at the bridge; `spinning-cube` loads its faces from `box.glb`, hardcoded arrays deleted. (`v0.0.10`)
* **Minimal texture creation and sampling** — purely additive RHI methods (`create_texture`, `create_textured_pipeline`, `set_texture`), UVs carried on the existing `Float32x2` attribute, single-texture fragment sampling with a Vulkan backend implementation; quadrant-correct pixel proof including a negative control that fails on the untextured pipeline. (`v0.0.10`)
* **2D physics** — a new crate, `canary-physics`: an object-safe, leak-free `PhysicsBackend` trait (no third-party types in public signatures), minimal components (`RigidBody`, `Collider`, `Velocity`, `GravityScale`, `LockedAxes`, `ColliderMaterial`, `PhysicsConfig`), and a private rapier2d 0.35.3 backend. (`v0.0.11`)
* **Fixed-timestep stepping with scheduler ordering** — a `PhysicsClock` accumulator plus `SimulationTime` (at most four `1/60` s steps per tick, leftover dropped by the spiral guard), frame time arriving as a `FrameDelta` resource through the existing `tick(dt)` seam with no `App` redesign, and first-position registration ahead of transform propagation and every render bake (proven fresh both directions, stale when reversed). (`v0.0.11`)
* **Physics game proof** — a ground, falling-box, and scripted-kinematic-platform scene drawn as z-pinned quads through the unchanged soup bake (zero RHI churn): headless tests in the normal suite plus `#[ignore]`-gated pixel tests with an unstepped-pipeline negative control. Box2D 3.2.0 measured against in a throwaway harness (rapier faster 1.13–1.19x on the N-box pile) and not shipped; determinism scoped to single-machine repeatability, not cross-platform. (`v0.0.11`)

* **Continuous performance benchmarking** — `divan` suites (through `codspeed-divan-compat`) in `canary-ecs`, `canary-scheduler`, `canary-transform`, `canary-assets`, `canary-loc`, `canary-physics`, and `canary-render-ecs`, covering entity/query/archetype work, stage scheduling, transform propagation, asset identity and PNG decode, localization resolution, fixed-step physics, and the full propagate → extract → bake frame. Measured on every pull request by CodSpeed's CPU simulation instrument (`.github/workflows/codspeed.yml`); benchmarks report, they do not gate — see [`docs/development/benchmarking.md`](docs/development/benchmarking.md).

### Changed

* Transform propagation now skips quiet ticks (no `Transform`/`Parent`/`Children` writes since the last run): ~113–200x cheaper steady-state ticks with byte-identical globals, guarded by same-tick and follow-up-pass rails plus a downstream tick contract.
* Render baking reuses scratch buffers across frames (steady-state bake allocates nothing; ~31% faster at small scenes, tail latency collapsed), mirroring the existing extract-scratch discipline; pixel output unchanged.
* Two external architecture reviews (September 2026) were triaged against the actual codebase — see [`docs/reviews/triage/2026-09-review-triage.md`](docs/reviews/triage/2026-09-review-triage.md) — resolving `v0.0.7`'s and `v0.0.8`'s scope rather than picking from `future-roadmap.md`'s previously-open options.
* [ADR 0016](docs/decisions/architecture-decision-records/0016-native-rendering-backends.md) superseded [ADR 0004](docs/decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md)'s original `wgpu`-backed RHI plan with native, per-graphics-API backend crates instead.

### Fixed

* Fixed undefined behavior in the Vulkan backend: the validation-layer callback no longer panics across the FFI boundary (it logs and aborts instead), and `VulkanCommandEncoder`'s `Drop` no longer expects on the unwind-abandoned path.
* Non-finite (`NaN`/infinite) `Transform`s are now skipped in release builds too, not just under `debug_assertions` — previously they poisoned `GlobalTransform` and churned change detection every frame.
* The localization loader's decode budgets are now enforced during the read itself with a capped reader; the previous metadata-only check could be bypassed by a path whose metadata understated its contents. Oversized packs fail with typed `OverBudget` errors.
* Removing a hierarchy is now safe: the new `canary-transform::despawn_subtree` detaches and despawns a whole subtree where raw `World::despawn` orphaned `Parent` links and left stale handles in survivors' `Children` lists.
* `World::entity_count` is now O(1) instead of scanning every slot ever created; physics body/collider indices fail loudly on exhaustion instead of wrapping around to alias a live slot.
* Several real CI/release gaps fixed: the release workflow's notes file pointed at a nonexistent path (every release silently fell back to auto-generated notes), the release checkout lacked `lfs: true` while testing LFS-tracked fixtures, and cargo invocations now use `--locked` with per-job timeouts throughout.
* Asset loaders now enforce a 64 MiB per-file budget *before* reading (previously the budgets could only fire after the whole file was allocated), and ship confined `*_within_root` entry points that refuse `..`/absolute/symlink-out escapes; symlinked locale directories are refused rather than followed. Malformed asset IDs are carried as data, never installed as paths.
* Trapped WebAssembly guests are now visible: `on_load`/`on_unload` traps emit a `tracing::warn!` naming the plugin and entry point (exit semantics unchanged).
* Fixed an intermittent (~1/10) test failure in the locale tracing-capture helper, where dispatcher teardown timing could outlive the test's event-buffer unwrap; the helper now yields briefly before unwrapping.
* A real cargo-audit finding: `wasmtime`/`wasmtime-wasi` bumped to close 18 RustSec advisories (including two critical sandbox-escape bugs), and `wayland-scanner` bumped to close two more via a transitive `quick-xml` dependency.
* Several real CI gaps found and fixed during the same pass: a Vulkan-dependent test that ran unguarded on every CI platform, a Windows-only compile failure from unscoped Wayland dependencies, and `canary-plugin-api`'s host-only loader code being incorrectly checked against the `wasm32-wasip2` target.
* A macOS-only CI flake: replaced a wall-clock timing assertion in the scheduler's concurrency test with a deterministic overlap proof after loaded runners exceeded it twice through pure scheduling jitter. (`v0.0.9`)
* Removed the unused `wasmtime-wasi` dependency (zero references in source; the lockstep-pin lesson is preserved in comments for when a real call site lands). (`v0.0.9`)

## [v0.0.15] — 2026-09-30

### Minimal server-authoritative networking

`v0.0.15` adds the smallest real server-authoritative replication path
that builds on project state — reusing the versioned codecs and snapshot
contracts `v0.0.14` establishes, plus the authority, removal history, and
ordering networking genuinely needs (ADR 0027 Accepted): a new
`canary-net` crate with a Canary-owned transport trait and a QUIC default,
canonical snapshot/delta replication with entity- and type-level opt-in,
a typed session path with validated client input, a durable tombstone log
with reconnect resync, and deterministic loopback proofs plus a
separate-process QUIC round trip. No prediction, no rollback, no
production identity — those stay explicitly out of scope.

### Added

* **Replication transport crate** — new crate `canary-net`: an object-safe, leak-free `NetTransport` trait (no third-party types in public signatures) with a private `QuinnTransport` QUIC default (ALPN `canary-1`, DER-bytes constructors, certificate pinning as a proof-only peer policy), length-prefix framing with the limit gate before any allocation, and a `postcard` envelope codec (no default features) that rejects trailing bytes and verifies the SHA-256 checksum before the payload is trusted.
* **Replication vocabulary** — canonical snapshots (sort-then-checksum bytes) and sequenced deltas with base-sequence basing and validate-all-before-apply semantics; stable server-scoped network identity (`NetEntityMap`, tuple-keyed so slot recycling never aliases); entity-level opt-in via the `Replicated` marker in `canary-ecs` composed with type-level opt-in via `ReplicationRegistry`; per-schema payload codecs where unknown schemas fail as resync-required, never panic; dirty-set computation stays in `canary-ecs` via `World::query_changed_since` (no parallel dirty-flag system). `Tick` gains only `get()`/`from_raw()` boundary accessors — wire sequence, simulation step (`SimTick`), and scheduler tick are never compared across domains.
* **Session path** — typed handshake vocabulary (`Hello`/`Welcome`/`Reject` with independent protocol, schema-manifest, game, and plugin/API checks; version mismatch is a typed reject followed by close), per-client input ingress validation (ownership, sequence, bounds, action-schema plus exact action-version compatibility, input window; malformed input is a typed error with no state mutation and the connection stays alive), bounded per-client queues with explicit backpressure (ingress overflow disconnects the offender, egress overflow drops the oldest stale state), per-client session records with whole-record disconnect, connection admission and liveness policy on a caller-supplied `u64` clock (no wall-clock reads), and capped per-client metrics rows so churn cannot grow memory.
* **Removal history and resync** — `TombstoneLog` (bounded retention, per-client cursors, ack-gated reclamation, drop→resync) closes the durable removal/destruction gap replication needs (R-33); `BaselineRetention` (bounded ring of recent authoritative snapshots consulted with the session-surviving tombstone log) lets a reconnect — always a new session — replay its shared baseline plus retained tombstones when covered, else take a live full snapshot. Per-delta journal replay is deliberately not built: retained snapshots plus tombstones supersede it (rationale documented in `resync.rs`).
* **Proofs** — `QuinnTransport` pinning proven over a real QUIC connection (right pin round-trips an envelope, wrong pin refuses the handshake); a separate-process QUIC session round trip (`tests/session_roundtrip.rs`: server and client as distinct processes over real TLS/ALPN exchanging snapshot, deltas, and frame-tagged input); an in-process loopback transport with deterministic counter-based fault injection carrying the resync end-to-end proof (drop mid-session, reconnect, incremental replay, live deltas after) and the 5-cell network-conditions matrix (clean, loss, duplication, reorder, latency — convergence plus no double-apply per cell, each fault cell proving its fault fired).

### Explicitly not in `v0.0.15`

Per-delta journal replay, an unreliable datagram lane, client prediction/reconciliation/rollback, production identity/matchmaking (certificate pinning is proof-only), and cross-build wire compatibility — carried to `.16` and beyond per ADR 0027.

Full scope: [`docs/release-notes/v0.0.15.md`](docs/release-notes/v0.0.15.md).

[v0.0.15]: https://github.com/HylightGames/canary/releases/tag/v0.0.15

## [v0.0.16] — 2026-10-01

### Server-authoritative live-collaboration operations

`v0.0.16` adds the first shared-authored-state slice built directly on
`.14`'s stable IDs/codecs and `.15`'s server-authoritative transport: a
new `canary-collab` crate owning a single authored-transform operation
with an 11-stage validation pipeline, server-assigned ordering,
`(actor, client-op-id)` idempotency, a server-side permission store with
restart fencing, bounded retained history with checkpoint envelopes, and
`postcard` wire codecs with 1 MiB ceilings and typed `TooLarge`
failures. The machine history is the ordering truth — no dual-write
into the human change log (ADR 0028 Accepted). Two clients can edit one
shared authored entity through an authoritative session with
permissions, conflict, persistence, and reconnect recovery all
observable; CRDTs, offline merge, and editor UI stay explicitly out of
scope.

### Added

* **Collaboration operation crate** — new crate `canary-collab`: the one operation (`op.rs`: complete local-transform replacement on an existing authored entity, `TransformPayload::validate` with finite floats and quaternion normalization within `QUAT_NORM_EPSILON`; zero/negative scale is content, not an error), server-minted actor identity (`actor.rs`: `ActorId`, owner/editor/reader `Role`, `ClientOpId` bounded to 128 bytes — no client-supplied identity is ever trusted), and the authoritative `Session` (`session.rs`: fixed validation stages 1–11, durable commit before ack, ordered FIFO broadcast outbox capped at 1024 messages).
* **Permission store** — server-side `PermissionStore` (`permissions.rs`: roles plus fencing epoch in its own atomic-JSON file, epoch 1 at provisioning, restart bumps the epoch, the file wins for known actors) provisioned out-of-protocol from pre-shared credentials; stage-4 role checks deny with the connection kept alive.
* **Ordering, idempotency, and history** — server-assigned sequences monotonic across restarts via the durable store (never reused after restart); stable `(actor, client-op-id)` idempotency where an identical replay returns the prior outcome and evicted IDs follow the resync-or-reject path, never silent re-execution; target-scoped compare-and-set conflicts (no global CAS, no LWW, no history rewrite); bounded retained history (`MAX_RETAINED_OPERATIONS` 128) where each trim records a checkpoint envelope (project revision, last retained op sequence, history marker) so a client behind the checkpoint takes the snapshot-plus-checkpoint path, never a partial tail.
* **Revisions and document gates** — `canary-state` gains `revisions.rs`: `LogicalEntityId` (the validated `entity.<local>` suffix; a rename is delete-plus-create, never a silent retarget), `ProjectRevision`/`ObjectRevision`/`OperationSequence`, the additive in-document `history` section (`DocumentHistory`: same canonical JSON and atomic save as the state it versions, `#[serde(default)]` and skipped while empty so files that never saw collaboration are byte-identical to pre-history files, genesis at project 0 / next-sequence 1 regardless of the human log), `trim_retained`, and `tail_since`/`TailGap`; plus document gates (`entity_section_exists`, `transform_override_allowed` prefab veto reusing the one-level prefab rules, `entity_transform`/`set_entity_transform`). `canary-runtime` composes the `CollabSessionHost` (`collab_session.rs`) over `canary-net` framing.
* **Wire codecs and ceilings** — bounded `postcard` request/response/sync codecs with an owned frame-tag registry (`wire.rs`: `TAG_EDIT`/`TAG_SYNC`, versioned with `COLLAB_PROTOCOL_VERSION`) and the limit gate before any allocation: 8 KiB requests, 512 B sync requests, 1 MiB responses and snapshots (`MAX_SNAPSHOT_BYTES`). Oversize fails typed (`CollabError::TooLarge` on the content gate, the encoded-reply gate, and every frame-body path), never truncated and never a generic `Malformed`. The 1 MiB snapshot ceiling is the `.16`-and-`.1.0` contract; the revisit trigger is the first real project whose canonical snapshot exceeds 512 KiB (chunked transfer wins over a silent bump), and the accept-path clone revisit trigger is p99 accept latency over 2 ms on a 1 MB project measured on real hardware.
* **Proofs** — stage-gated unit coverage (each rejection stage pins its stable code) plus break-it suites (`canary-collab/tests/break_it.rs`, `canary-runtime/tests/collab_break.rs`): two clients converge through the session, duplicates/conflicts/denials behave per the written rules, restart fencing bumps the epoch with no sequence reuse or double apply, and reconnect resync recovers through the retained tail or a checkpointed snapshot.

### Explicitly not in `v0.0.16`

Peer-to-peer authority, CRDT merge, offline operation queues, generic component patches, prefab-graph rewrites, editor UI, presence/cursors, locks, hosted identity, unbounded history, history rewriting, and gameplay `World` replication. Carried forward per ADR 0028 and `live-collaboration.md`: in-protocol grant/revoke, a JSON depth/size budget on project and permission files (local operator-owned input for now), signed sync checkpoints (single-operator trust for now), and per-property LWW (deferred, not rejected — a possible future refinement inside the target revision check).

Full scope: [`docs/release-notes/v0.0.16.md`](docs/release-notes/v0.0.16.md).

[v0.0.16]: https://github.com/HylightGames/canary/releases/tag/v0.0.16

## [v0.0.12] — 2026-09-28

### Audio playback

`v0.0.12` gives Canary sound-asset loading plus real audio playback:
an `AudioSource` component triggered by real game state, playing real
sound assets through Canary's own trait with a private `rodio`
bootstrap backend.

### Added

* **WAV and Ogg Vorbis sound loading** — `Sound` (interleaved `f32` PCM, mono/stereo) decoded by pure-Rust `hound` + `lewton` (Symphonia declined on MPL-2.0 grounds), with bit-exact fixtures, file and decode budgets, and the same root confinement as every other loader.
* **Audio playback** — a new crate, `canary-audio`: an object-safe, leak-free `AudioBackend` trait (no third-party types in public signatures), minimal components (`AudioSource` trigger intent, `AudioListener`, system-owned voices, `AudioConfig`), and a private rodio 0.22.2 bootstrap backend (decode-only MIT/Apache features; no-device operation degrades typed, never panics). A scheduler-registered trigger system turns game-state transitions into backend calls (removal/despawn reaps voices), proven by a stub-backend game proof asserting exact call order.
* [ADR 0023](docs/decisions/architecture-decision-records/0023-audio-bootstrap-rodio-behind-custom-trait.md) (rodio bootstrap behind a Canary-owned trait) moved from **Proposed** to **Accepted**.

Full scope: [`docs/release-notes/v0.0.12.md`](docs/release-notes/v0.0.12.md).

[v0.0.12]: https://github.com/HylightGames/canary/releases/tag/v0.0.12

## [v0.0.14] — 2026-09-29

### Authored project state and deterministic simulation snapshots

`v0.0.14` adds authored project state and deterministic simulation
snapshots as two separate products sharing one codec vocabulary
(ADR 0026 Accepted): canonical-JSON project documents with one-level
prefab bake and staged spawn, plus checksummed postcard simulation
snapshots with an owned-RNG simulation step — presentation excluded,
atomic saves with interrupted-write recovery, migration and unknown-data
fixtures proving both products independently.

### Added

* **Authored project documents** — `canary-state` canonical pretty-JSON documents with deterministic ordering, unknown-field preservation (byte-identical load→save round trips), per-schema linear migration chains with a `migrate_fields` JSON bridge, an authored change log, atomic save (sibling temp file, flush, rename), and staged load.
* **One-level prefab bake** — `SpawnPlan::from_document` bakes `prefab` references one level (base fields first, instance fields win per field; chained bases rejected) without touching a live world and without rewriting the document; bake output equals the hand-written equivalent.
* **Staged spawn** — `canary-runtime`'s `AuthoredSpawner` validates the whole plan (prefab bake, asset resolution, world registry agreement, full decode into staged typed inserts) before the first `World::spawn`, so any failure aborts with zero mutations.
* **Simulation snapshot boundary** — `SnapshotRegistry` captures only bound component/resource schemas in canonical record order (byte-identical across binding order and allocator history), with SHA-256 `checksum` re-pinning the envelope digest and staged validate-before-mutate `restore` (undeclared schemas, decode failures, dangling references, duplicate resource records, and tampered payloads all abort with zero world mutations).
* **Deterministic simulation step** — `Simulation::step` advances the ECS tick, folds `dt` into the sim clock (published as a `SimClock` resource), and draws from a seed-owned `OwnedRng` stream with no OS randomness; tick, clock, and RNG position travel as a reserved `canary.sim-state` record at `u32::MAX` with no wire-format break (the golden-bytes test still pins the pre-sim-state encoding), and same-seed determinism holds across the save/restore boundary step for step.
* **Proof fixtures** — migration + unknown-data fixtures, same-seed determinism across save/restore, presentation-exclusion sentinels (including byte scans of the capture payload), interrupted-write recovery on both products (last good file loadable; truncated files fail typed at the parse/checksum gate, never partial state), capture→restore→recapture byte stability (scoped to the LIFO free-stack discipline), and the phase-two re-decode purity proof.

Full scope: [`docs/release-notes/v0.0.14.md`](docs/release-notes/v0.0.14.md).

[v0.0.14]: https://github.com/HylightGames/canary/releases/tag/v0.0.14

## [v0.0.13] — 2026-09-28

### Playable and visible: UI, presentation, and the frame driver

`v0.0.13` adds windowed presentation with live lifecycle gates,
deterministic gameplay input with UI capture, a backend-neutral UI
abstraction bootstrapped on `egui`, the public runtime frame driver,
and the first game sample — a HUD over a live scene in a real window.

### Added

* **Window presentation with live lifecycle gates** — the Vulkan presenter acquires, clears-or-blits, and presents through the real swapchain: steady frames, minimize/restore, resize→recreate, content-present, and safe destruction proven live; capability-rejection and fatal-error mappings unit-gated.
* **Pointer and focus platform events** — `canary-platform` normalizes pointer position (logical pixels), buttons, pointer-leave, and focus-loss alongside keyboard transitions (`winit` + headless).
* **Deterministic gameplay input with UI capture** — new crate `canary-input`: game-declared schemas, multi-binding mapping, per-binding UI pass-through judged inside the mapper, held-state release on focus-loss/leave/capture, and the immutable per-pass `SimulationInput` snapshot.
* **Backend-neutral UI on `egui`** — new crates `canary-ui-core` and `canary-ui-egui` (egui 0.36 adapter painting through the same RHI pass as the scene).
* **Runtime frame driver** — `Runtime::drive_frame` owns the phase order (event pump → UI routing → tick → one scheduled pass → extract → paint → present); tick and `sim_time` advance only on real simulation passes.
* **Windowed game proof** — new `ui-game` example: WASD/arrow movement, Space-or-HUD-button fire, readout HUD in the same RHI pass as the scene; 600/600 frames presented live.
* [ADR 0025](docs/decisions/architecture-decision-records/0025-deterministic-input-actions-and-ui-capture.md) moved from **Proposed** to **Accepted**; [ADR 0024](docs/decisions/architecture-decision-records/0024-reusable-runtime-composition.md) items 1–5 **Accepted** (item 6, typed lifecycle failure semantics, stays Proposed).

Full scope: [`docs/release-notes/v0.0.13.md`](docs/release-notes/v0.0.13.md).

[v0.0.13]: https://github.com/HylightGames/canary/releases/tag/v0.0.13

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
