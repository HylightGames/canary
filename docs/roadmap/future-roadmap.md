# Future Roadmap (Beyond v0.0.1)

This tracks items that are real, intended parts of Canary's future but are
**not** assigned to a specific version yet — see
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md) for the
narrative "era" framing these fall into. Assigning fake specificity (a
version number, a date) to work this far out would cost more in false
confidence than it would deliver in planning value; this document is
deliberately organized by *dependency* rather than by date.

The one exception: [`v0.0.1-roadmap.md`](v0.0.1-roadmap.md#definition-of-done-for-the-unqualified-v001--revised)
names the archetype ECS migration, the Wasmtime-backed Tier A plugin
loader, and a real windowing backend as the concrete near-term
candidates that were scoped *out* of `v0.0.1`. Per the release cadence in
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md#release-cadence-one-focused-subsystem-per-00x-target-v010-as-substantially-feature-complete),
these now have an explicit order rather than being one undifferentiated
group: the archetype ECS migration is `v0.0.2` (see
[`v0.0.2-roadmap.md`](v0.0.2-roadmap.md) for its scope), Tier A WASM
plugin loading is next (`v0.0.3`), and real windowing follows after that
— each its own release, not bundled. Everything below remains genuinely
unassigned.

## Blocked on the ECS reaching its target (archetype) design

- Parallel job-stealing scheduler (`docs/architecture/core-runtime.md`)
- Change-detection query filters
- Networking replication (`docs/architecture/networking.md`) — depends on
  change detection above
- Rollback-netcode support — depends on networking above

## Blocked on the Tier A (WASM) plugin loader existing

- Gameplay scripting hot reload (`docs/architecture/scripting-system.md`)
- The community marketplace (Era 6,
  `docs/vision/long-term-roadmap.md`) and its capability-review tooling
- Visual scripting graph (compiling to the same WASM component interface as
  textual languages — see
  `docs/architecture/scripting-system.md#designer-facing-ergonomics-vs-systems-programmer-ergonomics`)

## Blocked on a real windowing/GPU environment to build against

- `winit`-backed `canary-platform` implementation
- The `wgpu`-backed RHI (`docs/architecture/rendering.md`)
- The render graph and materials system
- The `egui`-backed `CanaryUI` implementation (`docs/architecture/ui-toolkit.md`,
  [ADR 0011](../decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md)) —
  the `canary-ui-core` trait layer itself is not blocked on this and could
  start earlier
- The editor (`docs/ui/editor-design.md`) — additionally blocked on the
  plugin system, since the editor is meant to be built as a plugin host,
  and on `CanaryUI` having a real backend

## Blocked on the asset pipeline existing

- Hot-reloadable content in editor/dev builds
- Any real example game beyond programmer-authored test scenes
- The medium-term scope of `canary-state`
  (`docs/architecture/state-and-versioning.md`) — persistent identity and
  authored change tracking need real asset formats to attach to

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
  and [ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md),
  but the actual mechanism (server-authoritative operation broadcast vs.
  CRDT-based leaderless merge) is a genuinely open question, deliberately
  left unresolved until there's a `canary-state` implementation to ground
  the choice in. Blocked on the medium-term scope of `canary-state`
  itself, which is in turn blocked on the archetype ECS migration and the
  asset pipeline (see the dependency sections above).

## Explicitly not on this list

Anything not written down here or in
[`docs/roadmap/v0.0.1-roadmap.md`](v0.0.1-roadmap.md) should be treated as
undecided, not as "implicitly planned." If you're a future contributor (or
a future development session) considering work not covered by this
document, the right first step is a new ADR or an issue, not code — see
[`CONTRIBUTING.md`](../../CONTRIBUTING.md).
