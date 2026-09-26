# Future Roadmap (Beyond `v0.1.0`)

This document tracks work that matters to Canary's long-term direction but is
not a dated or versioned commitment. The current implementation inventory is
[`status.md`](status.md); the next dependency-ordered delivery sequence through
`v0.1.0` is [`v0.1.0-plan.md`](v0.1.0-plan.md). The narrative eras are in
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md).

Do not treat an architecture document, risk entry, or technology evaluation
as an implicit implementation promise. When a proposed item reaches its
trigger below, review current code and evidence, write or amend the relevant
ADR, update the architecture design, then add a milestone with measurable
acceptance. No calendar dates or speculative version numbers are assigned
here.

## The committed path comes first

The repository is at `v0.0.12` on `dev`; `v0.0.13` is next. Finish the live
handoff and acceptance proof in [`docs/roadmap/README.md`](README.md), then
continue in this order:

1. `v0.0.13`: supported game-runtime composition, window presentation,
   shared input actions, and `CanaryUI`.
2. `v0.0.14`: authored project state plus separate deterministic simulation
   snapshots.
3. `v0.0.15`: minimal server-authoritative networking with removal history,
   canonical state, and frame-tagged input.
4. `v0.0.16`: a first live-collaboration slice with explicit operation,
   history, authorization, conflict, and version-lineage rules.
5. `v0.1.0`: prove the systems together in a real, small game using the
   supported consumer API.

The milestones are intentionally integration-focused. The editor,
beginner-friendly workflows, full asset cooking, visual scripting, marketplace,
3D physics, and prediction/rollback are not prerequisites for that bar.

## Post-`v0.1.0` sequence: editor to ecosystem

The dependency order below is a direction, not a fixed implementation plan.
At each stage, keep the engine headless-usable and keep editor code as a
consumer of the same public runtime and subsystem APIs games use.

### 1. Make authored projects practical to work with

Build on `canary-state`'s stable logical identity, schema codecs, migrations,
and explicit project/simulation separation. Add the asset pipeline capabilities
the first game did not need: deterministic cook keys, dependency tracking,
cache invalidation, importer extensions, residency/lifetime policy, and
development-time reload. Keep source identity separate from derived cache
identity and paths as hints rather than permanent asset identity.

**Exit evidence:** a project can be moved or renamed without breaking authored
references; changing an input asset or importer version rebuilds exactly the
affected derived artifacts; reload reports failures without corrupting the
running project; builds work without opening the editor.

### 2. Build an editor vertical slice on the game-facing APIs

The editor is a first-party client/plugin host, not a second runtime. Start
after a reusable consumer runtime, project save/reload, windowed UI, and live
plugin `World` access have been demonstrated. The first useful slice should
open a project, show a scene hierarchy and inspector, select and transform an
entity in the viewport, save/reload the change, and build/run the game from a
headless command-line path.

Keep built-in panels on the same extension model intended for user panels, but
do not stabilize a public editor-panel SDK until plugin lifecycle, failure
isolation, reload/replacement, and workspace persistence are designed and
dogfooded (risk R-36). The GUI is not a prerequisite for building, testing, or
running a project.

### 3. Add high-feedback authoring and debugging tools

Once the editor shell and authored state are usable, add asset browsing,
validation and import diagnostics, hot reload, log/trace inspection, profiling,
and script debugging. Preserve structured diagnostics and stable object IDs
so tools can refer to entities, assets, and schemas without relying on memory
addresses or Rust `TypeId`s.

Textual gameplay scripting and visual scripting both target the existing
language-neutral WebAssembly Component interface. Visual scripting is an
authoring front end for that runtime, not a second execution model. Resolve
the fast feedback path before promising editor iteration: compile-per-edit may
not meet the immediate response expected from a graph editor (risk R-17).
Measure an interpreted or incremental path against a real graph before
choosing it.

### 4. Mature collaboration beyond the first shared-edit proof

Extend the `v0.0.16` slice only after project-state operations and the
authoritative session have proven their identity, ordering, and recovery
contracts. Add resilient history, reconnect/replay, permission administration,
conflict presentation, and optional local merge techniques as supported
workflows require. The session remains self-hostable. The server-authoritative
topology in ADR 0013 is the accepted direction; its operation format and
product-level permission rules remain design work until the first slice.

Undo/redo and time-travel debugging should reuse a deliberate operation or
snapshot history rather than grow separate mutation logs in each tool.
Cross-platform deterministic lockstep and rollback are separate, evidence-led
projects, not synonyms for basic replication.

### 5. Open the plugin ecosystem deliberately

Public distribution comes after Canary has exercised its plugin API in its
own editor and game workflows. Before strangers can publish packages, define
the manifest and package format, engine/API compatibility ranges, dependency
resolution, capability review and display, provenance/signing, safe unload and
resource reclamation, and what happens when plugins claim the same replacement
role (R-08, R-09, R-19, R-26, R-35). These requirements span code and authored
content, so package migrations and project-state schema rules must align.

**Exit evidence:** a package can be inspected without executing it; the host
can reject incompatible or over-capable packages before load; the user can
identify provenance and permissions; removing or replacing a plugin reclaims
its resources safely.

### 6. Harden and widen platform/backend support when consumers justify it

API stability, reproducible performance work, backend-removal checks,
cross-platform window/render validation, 3D physics, additional graphics
backends, mobile, and console support are real future work, but each needs a
motivating consumer and a measurable acceptance bar. The RHI and subsystem
traits are intended to make replacement possible; they do not prove a second
backend until one is actually built. Console work additionally requires an
authorized SDK path and a motivated contributor/backer.

## Architecture gaps that remain open

These are risks and design questions, not a new release queue. See the
[`risk register`](../reviews/risk-register.md) for current IDs and status.

- **Scheduler safety and scaling (R-24):** access sets are manual metadata
  beside system bodies that can access the full `World`. Keep writers solo
  until typed parameters or another enforceable access mechanism exists.
  Measure real game-shaped workloads before adopting a persistent work-stealing
  pool or disjoint-write execution.
- **Plugin/runtime lifecycle (R-34, R-36):** a Tier A host currently owns its
  `World`; safe access to a live runtime world, reload, pause/resume, and
  replacement need one shared lifecycle/access design before editor dogfooding.
- **Removal history and causality (R-32, R-33):** mutation ticks do not report
  despawns, and local ticks are not a distributed causal clock. Implement and
  test durable removal plus canonical snapshots before replication.
- **Stable state and input:** project formats must preserve unknown schema
  data, and networking must receive deterministic, frame-tagged input rather
  than platform events. ADRs 0020–0022 lock the direction; the `.14`/`.15`
  milestones must prove the mechanisms.
- **Asset identity:** current `AssetId` is content-derived and provisional.
  Authored `LogicalAssetId` and source-hash/importer-version/dependency keyed
  derived artifacts are separate identities and have separate lifecycle.

## Documents and ADRs to create when the work reaches them

These records are deliberately not written as speculative decisions. Start
them at the relevant milestone, then append the next ADR number in the index.

| Trigger | Document or ADR | It needs to settle or explain |
|---|---|---|
| Before `v0.0.13` runtime/UI implementation | `docs/architecture/runtime-composition.md` and an ADR | Public game entry point; platform/window ownership; event, simulation, UI, render and audio phase order; `RunContext`/tick advancement; shutdown and errors; safe scoped plugin access to the active `World`; UI focus/input capture; lifecycle consequences for pause, reload, and replacement. |
| Before `v0.0.14` formats are implemented | Expand `state-and-versioning.md` or add `state-format-and-migrations.md`, plus an ADR if the encoding/compatibility choice is cross-cutting | Stable IDs, codec and encoding version, migration graph/failure, unknown-schema round trip, missing vs. unknown fields, atomic persistence, canonical order, and the distinct authored-state/simulation-snapshot products. Resolve the open status of ADR 0012 deliberately. |
| Before `v0.0.15` wire behavior is implemented | Networking architecture update and an ADR | Replication unit, state/removal ordering, full snapshot vs. delta, `Tick` vs. causality, frame-tagged input, authority and trust boundaries, transport failure/reconnect behavior, and compatibility. Build on ADR 0007 rather than re-deciding QUIC without evidence. |
| Before collaborative edits are accepted | An ADR amending/extending ADR 0013 and a collaboration protocol section in `state-and-versioning.md` | Operation identity and history, server ordering, authorization, conflict/rejection semantics, stale-client recovery, version ancestry, and what can be undone. |
| Before the public editor panel API | Expand `docs/ui/editor-design.md` and add a lifecycle/extension ADR | Panel ownership and isolation, workspace persistence, plugin failure/reload/replacement, game-vs-editor process boundaries, and headless CLI behavior. Address R-36. |
| Before visual scripting implementation | `docs/architecture/visual-scripting.md` and an ADR | Graph file/version format, mapping from nodes to the shared WASM component ABI, incremental/interpreted feedback path, debug/source mapping, hot-reload state, and deterministic simulation constraints. Address R-17. |
| Before external plugin distribution | Plugin package/security architecture doc and an ADR | Manifest, compatibility ranges, dependency resolution, capability declarations/review, signing/provenance, safe unload, authored-content migrations, and replacement conflicts. Address R-08/R-09/R-19/R-35. |

Avoid duplicating settled rules: ADR 0020–0022 already constrain render graph
direction, capability/surface seams, identity, serialization, determinism,
input, and scheduler semantics. Amend those records explicitly if new evidence
requires changing a lock.

## Open research with no assigned milestone

- **AI/ML inference (`canary-ai`):** possible native inference hooks for
  gameplay, procedural content, or editor assistance. Decide the runtime and
  trust boundary only when a concrete workload and supported model format
  exist. See [design philosophy](../vision/design-philosophy.md#ai-ready-architecture).
- **First-party MCP for a live engine:** a possible inspection/control
  interface for agents, distinct from in-game inference. It depends on stable
  component identity and a supported runtime/debugging surface; define safe
  operations and user consent from a real editor/game workflow first.
- **GPU-accelerated physics:** consider as another `PhysicsBackend` only when
  upstream technology and a measured workload justify it.
- **Mobile and console:** revisit with motivated consumers; console access
  needs the relevant licensed SDKs.
- **Other graphics APIs:** Metal, Direct3D 12, and OpenGL/WebGL can be separate
  optional RHI backend crates. Choose one based on platform demand and prove
  the trait against it rather than building a backend as an architecture demo.

Anything absent from this document and the
[`v0.1.0` plan](v0.1.0-plan.md) remains undecided. Propose it through an issue
or a new ADR before treating it as committed work; see
[`CONTRIBUTING.md`](../../CONTRIBUTING.md).
