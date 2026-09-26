# Future Roadmap (Beyond v0.0.1)

This tracks items that are real, intended parts of Canary's future but are
**not** assigned to a specific version yet — see
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md) for the
narrative "era" framing these fall into. Assigning fake specificity (a
version number, a date) to work this far out would cost more in false
confidence than it would deliver in planning value; this document is
deliberately organized by *dependency* rather than by date.

The one exception:
[`v0.0.1-roadmap.md`](v0.0.1-roadmap.md#definition-of-done-for-the-unqualified-v001--revised)
names the archetype ECS migration, the Wasmtime-backed Tier A plugin
loader, and a real windowing backend as the concrete near-term
candidates that were scoped *out* of `v0.0.1`. All three have since
shipped (`v0.0.2`, `v0.0.3`, `v0.0.4` respectively — see
[`milestones.md`](milestones.md#beyond-v002) for the full sequence
through `v0.0.12`); this paragraph is kept as history of how they were
originally sequenced, not as a claim that they're still upcoming.

## Blocked on the ECS reaching its target (archetype) design

This blocking condition is now satisfied — archetype storage
(`v0.0.2`), multi-component queries and typed resources (`v0.0.7`), and
the scheduler itself (`v0.0.8`, `canary-scheduler`) all exist. What's
left in this dependency group:

- Networking replication (`docs/architecture/networking.md`) — change
  detection exists, and the runtime harness now advances the ECS tick,
  but replication still needs durable removal/destruction records,
  canonical snapshots, deterministic simulation input, and authority
  semantics before it can consume those primitives safely. ADRs 0020–0022
  lock these foundations; implementation is planned for `v0.0.15`, after
  the project-state milestone.
- Rollback-netcode support — depends on networking above.
- Concurrent *disjoint* writes in `canary-scheduler` itself and a
  reusable runtime composition API remain open. The private
  `canary-runtime` harness owns an `EcsSubsystem` and a `Schedule`, but
  that is not yet the supported game-consumer surface.

## Blocked on the Tier A (WASM) plugin loader existing

This blocking condition is also now satisfied (`v0.0.3`). What's left in
this dependency group — each still genuinely unbuilt, just no longer
waiting on Tier A itself:

- Gameplay scripting hot reload (`docs/architecture/scripting-system.md`)
- The community marketplace (Era 6,
  `docs/vision/long-term-roadmap.md`) and its capability-review tooling
- Visual scripting graph (compiling to the same WASM component interface as
  textual languages — see
  `docs/architecture/scripting-system.md#designer-facing-ergonomics-vs-systems-programmer-ergonomics`)

## Blocked on a real windowing/GPU environment to build against

Real windowing (`v0.0.4`, `canary-platform`'s `winit-backend` feature)
and a first RHI backend (`v0.0.6`, `canary-render` + `canary-render-vulkan`)
both now exist, satisfying this blocking condition. Two corrections
while updating this section: the RHI backend that shipped is **not**
the `wgpu`-backed one originally planned in
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md) —
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md)
superseded that with native, per-graphics-API backend crates instead
(`canary-render-vulkan` via `ash`, the same "no privileged built-ins"
pattern `physics.md` uses for physics backends), and "`winit`-backed
`canary-platform` implementation" below is `v0.0.4`'s completed work,
not an open item. What's left in this dependency group:

- The render graph and materials system
- The `egui`-backed `CanaryUI` implementation (`docs/architecture/ui-toolkit.md`,
  [ADR 0011](../decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md)) —
  the `canary-ui-core` trait layer itself is not blocked on this and could
  start earlier
- The editor (`docs/ui/editor-design.md`) — additionally blocked on the
  plugin system (satisfied, `v0.0.3`) and on `CanaryUI` having a real
  backend (not yet)

## Blocked on authored asset identity and the mature asset pipeline

The minimal asset crate and synchronous GLB/PNG/WAV/Vorbis loaders exist.
The remaining state/editor work depends on stable `LogicalAssetId` values,
authored formats, and the mature pipeline contracts:

- Hot-reloadable content in editor/dev builds
- Cooking, cache keys, dependency tracking, and importer extensions
- Authored project state and scene/prefab references in `canary-state`
- Asset browser and editor workflows

## Not blocked on anything specific — genuinely open questions

- **`canary-ai` subsystem** (native AI/ML inference hooks — NPC behavior,
  procedural content, editor copilot tooling). Likely shape: a WASI
  `wasi-nn`-style interface or ONNX Runtime binding, exposed as a Tier B (or
  possibly Tier A, if a sandboxed inference story matures) plugin. Explicitly
  not scoped in detail yet — see
  [`docs/vision/design-philosophy.md`](../vision/design-philosophy.md#ai-ready-architecture)
  for the two-part interpretation of "AI-ready" this project is committing
  to, of which this subsystem is only the second part.
- **Console platform support** (PlayStation, Xbox, Switch). Gated on a
  motivated backer or community with access to the relevant NDA'd SDKs — see
  [`docs/vision/project-goals.md`](../vision/project-goals.md#non-goals-for-now).
  The platform-abstraction trait design
  (`docs/architecture/platform-abstraction.md`) is intended to make this
  "implement the trait for a new platform" rather than an engine-wide audit,
  but this is untested until it's actually attempted.
- **GPU-accelerated physics.** Dimforge (Rapier's maintainers) have publicly
  stated 2026 goals toward `rust-gpu`-based physics; if that matures, it's a
  natural fit for Canary's `PhysicsBackend` trait as an additional backend
  option — tracked here rather than promised, since it depends on upstream
  work outside this project's control.
- **Editor UI toolkit choice** (immediate-mode vs. retained-mode; `egui` as
  a bootstrap vs. a custom toolkit long-term) — see
  [`docs/ui/editor-design.md`](../ui/editor-design.md#ui-toolkit-an-open-question-not-a-decision)
  for the explicit deferral and the criteria that will eventually resolve
  it.
- **Mobile platform support.** Not explicitly requested in the founding
  brief and not ruled out; revisit once desktop is solid.
- **Real-time collaborative editing** ("live share"). Named and
  architecturally scoped in
  [`docs/architecture/state-and-versioning.md`](../architecture/state-and-versioning.md)
  and [ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md).
  The topology/authority question [ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md)
  originally left open is resolved: [ADR 0013](../decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md)
  commits to server-authoritative, client–server–client, the same
  authority model as gameplay networking. What's still genuinely open is
  the wire protocol, the operation schema, and permission-model
  specifics — deliberately left unresolved until there's a `canary-state`
  implementation to ground the choice in. Blocked on the medium-term
  scope of `canary-state` itself. The archetype ECS and minimal typed asset
  loading now exist; stable logical asset IDs, authored formats, versioned
  migrations, and the separate simulation-state contract remain
  prerequisites before collaboration protocol work starts.
- **First-party MCP (Model Context Protocol) access to a running engine
  instance**, distinct from the existing `canary-ai` in-game/in-editor
  inference bullet above: exposing ECS state, the scene graph, and
  (once it exists) asset-pipeline status as MCP tools, so an AI coding
  agent can inspect and drive a live Canary session the same way a human
  using the editor would, rather than through ad hoc reflection over an
  API that wasn't designed for it (which is how at least one existing
  third-party integration works for another engine today). This is the
  natural extension of design-philosophy.md's first "AI-ready" pillar
  ([legible to AI-assisted development](../vision/design-philosophy.md#ai-ready-architecture))
  rather than its second (in-game/in-editor inference, i.e.
  `canary-ai`) — and depends on [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)'s
  stable component identity (so an agent can name a component without
  ever needing to know a `TypeId`) and, for anything editor-side, on the
  editor existing at all (Era 5). Genuinely open on shape and timing;
  tracked here rather than assigned an era.

## Explicitly not on this list

Anything not written down here or in
[`docs/roadmap/v0.0.1-roadmap.md`](v0.0.1-roadmap.md) should be treated as
undecided, not as "implicitly planned." If you're a future contributor (or
a future development session) considering work not covered by this
document, the right first step is a new ADR or an issue, not code — see
[`CONTRIBUTING.md`](../../CONTRIBUTING.md).
