# External architecture review triage — Review #3 (replaceable subsystems)

A third external AI architecture review ("Canary Architecture Review #1,"
by ChatGPT) was commissioned by Cloudy, prompted by his own proposal to
push replaceable-backend abstraction ("bindings, not hardcoding") much
further across the project — UI, physics, rendering, audio, networking,
asset importers, input, and platform services. This is the project-lead
triage pass over that review, done the same way as
[the September review triage](2026-09-review-triage.md) for the prior
two: every recommendation checked against the actual repository (docs and,
where code exists, code) rather than taken on faith, then marked
**Accept**, **Accept (deferred)**, **Partially accept**, **Already true**,
or **Reject**.

## The headline finding, stated up front

Most of this review's central thesis — "Canary owns contracts and
orchestration; implementations are replaceable unless a subsystem is
explicitly classified as internal" — is **already Canary's stated, named
architecture**, not a new proposal.
Specifically:

- [`docs/vision/design-philosophy.md`](../vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly)
  already states, as a general rule (not a per-subsystem observation): "no
  Canary crate calls a third-party library, or another Canary subsystem's
  internals, directly," including the exact "a trait that leaks a
  concrete third-party type has not actually achieved swappability" test
  the review independently arrives at for `PhysicsBackend`/`rapier3d`.
- [`docs/architecture/engine-overview.md`](../architecture/engine-overview.md#the-two-structural-bets-this-engine-makes)
  names "everything replaceable is a trait, not a `#[cfg]` flag" as one of
  exactly two foundational structural bets the whole engine makes.
- This is mechanically verified, not just asserted, for the one
  backend-swap point already built: `canary-platform`'s `winit-backend`
  feature is off by default, and `cargo tree` without it shows no
  `winit` in the dependency graph
  ([`platform-abstraction.md`](../architecture/platform-abstraction.md#status-in-this-foundation)).
- Networking transport
  ([`networking.md`](../architecture/networking.md#transport-quic-as-the-default)),
  asset importers
  ([`asset-system.md`](../architecture/asset-system.md#importers-as-plugins)),
  and input
  ([`platform-abstraction.md`](../architecture/platform-abstraction.md#what-it-abstracts))
  — the three areas the review specifically calls out as needing this
  treatment beyond UI/physics/rendering/audio — are each already
  documented as trait boundaries for exactly this reason.

This doesn't make the review low-value — an independent review converging
on a project's own already-stated first principles, from reading the
subsystem docs rather than the philosophy doc that generalizes them, is
itself a useful signal (the same "independently convergent" validation
[the prior triage](2026-09-review-triage.md) noted for ECS-before-scheduler).
It does mean most items below are **Already true**, and the real value in
this review is the handful of genuinely new, concrete additions — a
compile-time-checkable invariant, a data-ownership table, named gaps that
weren't previously named — called out explicitly where they occur.

## Numbered points

| # | Point | Verdict |
|---|---|---|
| 1 | Elevate replaceability to a project-wide architectural law | **Already true** — see above |
| 2 | Revisit `egui` coupling; test "remove the backend, does it still build" as an invariant | **Partially accept** |
| 3 | Audit dependency boundaries for dead-code elimination | **Partially accept** |
| 4 | Don't let rendering couple to "basically Vulkan" | **Already true** |
| 5 | API stability vs. implementation stability, phased by version | **Partially accept** |
| 6 | ECS as shared data model, not a god object | **Already true** |
| 7 | Explicit per-subsystem data-ownership table | **Accept** |
| 8 | Deep-review `World` serialization before Project State lands | **Already true**, with one genuine addition |
| 9 | Don't let early `AssetId`/`AssetHandle<T>` be secretly path-based | **Already true** |
| 10 | Measurable "unused subsystem elimination" requirement | **Partially accept** |
| 11 | Extension / Replacement / Override as distinct plugin concepts | **Partially accept** |
| 12 | Design subsystem lifecycle (pause/resume/reload/replace) before subsystems multiply | **Accept** |
| 13 | Separate "engine API" from "game-facing API" more deliberately | **Already true** |
| 14 | Explicit escape-hatch levels (high-level API → advanced → backend-specific) | **Already true** |
| 15 | Capability detection instead of platform stereotypes | **Accept (deferred)** |
| 16 | Keep the `v0.1.0` dependency order; stop and fix a primitive if it proves insufficient | **Already true** |

### 1 — Elevate replaceability to a project-wide law

**Already true.** See "The headline finding," above. Nothing to change;
the review's own proposed wording ("Canary owns contracts and
orchestration...") is a close paraphrase of
[`design-philosophy.md`](../vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly)'s
existing text, not a new rule.

### 2 — Revisit `egui`; test backend removal as an invariant

**Partially accept.** The abstraction itself is already correctly
designed — [`ui-toolkit.md`](../architecture/ui-toolkit.md#two-different-things-deliberately-kept-separate)
already separates `canary-ui-core` (the trait/API layer, starts now) from
`canary-ui-egui` (a replaceable backend, `v0.0.13`), explicitly generalized
as "exactly like an alternative `PhysicsBackend` or RHI implementation."
Nothing here is built yet (`CanaryUI` is `v0.0.13` per
[the plan](../roadmap/v0.1.0-plan.md)), so there's no premature `egui` coupling to
fix today. The genuinely new, concrete piece worth adopting: **turn "does
it still compile with the backend feature off" into a standing,
per-backend CI check**, not a one-time manual verification. This already
happened once, informally, for `winit` (`cargo tree` checked by hand when
`v0.0.4` shipped); formalizing it as a repeatable CI matrix entry — run
once for each optional backend feature as it lands (`winit-backend` today;
`egui`, a Rapier/Jolt physics backend, and an FMOD/Wwise audio binding
later) — is real, low-cost, high-value process the review adds that
wasn't explicit before. Filed as a `docs/development/coding-standards.md`
/ CI-checklist addition to make when `v0.0.13` (or the next backend
feature) actually lands, not retrofitted onto `winit` alone right now.

### 3 — Audit dependency boundaries for dead-code elimination

**Partially accept.** Same underlying point as #2, generalized past UI.
Already done, mechanically, for the one backend that exists
(`winit-backend`, verified via `cargo tree`,
[`platform-abstraction.md`](../architecture/platform-abstraction.md#status-in-this-foundation)).
Nothing to audit yet for physics/audio/UI backends because none exist in
code. Accept the review's implicit process point — feature-gate every
optional backend, verify via `cargo tree` (or the CI-matrix version of the
same check from #2) that the default build excludes it — as a standing
practice for each backend crate as it's built, not a new one-time audit
task now.

### 4 — Don't let rendering couple to "basically Vulkan"

**Already true.** [ADR 0004](architecture-decision-records/0004-rendering-abstraction-strategy.md)
and [ADR 0016](architecture-decision-records/0016-native-rendering-backends.md)
already establish exactly the layering the review asks for (render graph
→ RHI trait → `canary-render-vulkan`), and it's real, not aspirational —
`canary-render`/`canary-render-vulkan` already exist as separate crates on
`dev`, with the RHI trait boundary between them. The review's specific
worry (future GI/GPU-driven-rendering research inheriting today's Vulkan
specifics) is a reasonable thing to stay alert to as that research
actually starts, but there's no current violation to fix — the boundary
the review wants already exists at the crate level, not just on paper.

### 5 — API stability vs. implementation stability, phased by version

**Partially accept.** The spirit already exists as a stated north star
(the `v0.1.0` "It works" / `v0.2.0` "It's good" / `v0.3.0` "It's pleasant"
/ `v1.0.0` "It's dependable" framing, and
[`design-philosophy.md`](../vision/design-philosophy.md#what-professional-grade-means-for-a-pre-10-project")'s
"professional-grade... achievable on day one" framing), and the
`Subsystem::tick` signature break the review specifically praises
(`v0.0.9`'s `Duration` parameter, made now rather than retrofitted) is
already exactly the "break it now while it's cheap" behavior it's asking
for. What's genuinely missing: an explicit, written **breaking-change
policy statement** (something closer to semver's own "what changes at
each stage" table) rather than an implicit norm everyone currently just
follows. Worth a short addition to
[ADR 0006](architecture-decision-records/0006-versioning-scheme.md) once
`v0.1.0` is close enough to make "what does `v0.2.0`+ stability actually
mean" a real near-term question — not urgent while every release before
`v0.1.0` is still expected to break things by design.

### 6 — ECS as shared data model, not a god object

**Already true.** Checked directly against `canary-ecs`'s actual `World`
API (`spawn`/`insert`/`query`/`query2`/`query3`/typed resources/change
detection) — `World` is a storage-and-query facility that subsystems read
and write through, not a coordinator that owns a renderer, physics
world, or network socket. [`engine-overview.md`](../architecture/engine-overview.md#how-a-frame-is-expected-to-flow-target-design-post-era-2)'s
frame-flow diagram already shows this precisely: physics/networking/
rendering each interact with the ECS scheduler, not with each other or
with a `World`-as-god-object. Worth staying alert to as physics/audio/
rendering are actually built (a bindable subsystem quietly reaching into
`World` beyond its declared system access would be the real violation),
but there's no current design that risks it.

### 7 — Explicit per-subsystem data-ownership table

**Accept.** This is genuinely useful and doesn't exist as a single
artifact today — ownership is currently stated piecemeal, correctly, but
scattered (e.g. "the physics backend owns its internal simulation state
but synchronizes transforms into ECS component storage,"
[`physics.md`](../architecture/physics.md)). A consolidated table (data →
owner: `Transform` → ECS, physics simulation internals → the physics
backend, render GPU resources → the renderer, asset metadata → the asset
system, network connection state → networking, ...) is cheap to write and
genuinely prevents the "three systems think they own the same object"
failure mode the review names. Filed as a near-term addition to
[`engine-overview.md`](../architecture/engine-overview.md) — the natural
home, since it's the cross-cutting map document already — to be written
alongside (not blocking) the `canary-transform` work, since `Transform`'s
own ownership line is one of the first real entries.

### 8 — Deep-review `World` serialization before Project State lands

**Already true**, with one genuine addition. Checked against
[`state-and-versioning.md`](../architecture/state-and-versioning.md) and
[ADR 0012](architecture-decision-records/0012-project-state-as-a-versionable-graph.md)/[ADR 0013](architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md):
this document already covers, explicitly, nearly every item the review
lists — versioning, entity identity (the runtime-vs-persistent-identity
distinction is exactly what the review asks for), component identity and
unknown-schema preservation (via ADR 0010), a real evaluated-and-rejected
CRDT alternative with stated reasoning, and migration rules for package
schema evolution. Genuinely not yet explicit: the review's specific
nuance that "Project State and network replication should share
*primitives*, but not necessarily the exact same wire *format*." The
current docs describe one shared model feeding both without drawing this
line as sharply. Filed as **accepted-but-deferred** — worth stating
explicitly when `canary-state` design work actually starts (`v0.0.14` per
[the plan](../roadmap/v0.1.0-plan.md)), not a gap in anything built or even fully
specified yet.

### 9 — Don't let `AssetId`/`AssetHandle<T>` be secretly path-based

**Already true.** [`asset-system.md`](../architecture/asset-system.md#content-addressing-and-caching)
already commits to content-addressed identity (a hash of source bytes +
importer version + import settings), explicitly not file paths, and
explicitly separates source assets from cooked runtime assets. Nothing
here is built yet (`v0.0.10`), so this is a docs-level check, not a
code audit — worth re-verifying against the real `AssetHandle<T>` API
once it exists, but the documented design already avoids the trap the
review names.

### 10 — Measurable "unused subsystem elimination" requirement

**Partially accept.** Same substance as #2/#3, restated as a requirement
rather than a practice. Already demonstrated once (`winit-backend`); the
genuinely new value is the review's framing of this as something to
*require*, not just *do when convenient* — worth stating explicitly in
[`docs/development/coding-standards.md`](../development/coding-standards.md)
as a checklist item for any PR introducing an optional backend, alongside
the CI-matrix addition from #2.

### 11 — Extension / Replacement / Override as distinct plugin concepts

**Partially accept.** [`plugin-system.md`](../architecture/plugin-system.md#why-two-tiers-instead-of-one)'s
Tier A/Tier B split already captures much of this distinction, but along
a *trust* axis (sandboxed vs. native), not the review's *kind-of-change*
axis (adds functionality / replaces an implementation / alters existing
behavior) — the two axes are genuinely orthogonal and the second one
isn't currently named anywhere. The review's specific concern —
"five plugins all trying to become *the* physics backend, and the
architecture never defined who wins" — is a real, currently open
question, not yet addressed by the Tier A/B split (which addresses *how
much a plugin can touch*, not *what happens when two plugins claim the
same subsystem-replacement role*). Filed as a new risk-register entry
(R-35) rather than designed now, since it isn't urgent before Tier B
subsystem-replacement plugins are a real multi-plugin scenario (Era 6),
but it's cheap to name today per this project's own standing practice of
naming gaps before they're discovered mid-implementation.

### 12 — Subsystem lifecycle before subsystems multiply

**Accept.** Checked directly against
[`engine/canary-core/src/subsystem.rs`](../../engine/canary-core/src/subsystem.rs):
`Subsystem` currently has exactly three lifecycle hooks — `init`, `tick`,
`shutdown` — with no `pause`/`resume`/`reload`/`replace` concept at all.
This is a real, accurately-observed gap, not yet a problem (nothing at
`v0.1.0` needs pause/resume/hot-reload/replace semantics — no editor, no
live collaboration, no scripting hot-reload in scope), but exactly the
kind of thing this project's own practice says to name while it's cheap
rather than let get discovered under scripting hot-reload
([`scripting-system.md`](../architecture/scripting-system.md#hot-reload)),
asset hot-reload, or editor play-mode later. Filed as a new risk-register
entry (R-36): design lifecycle semantics (create/init/start/tick/pause/
resume/stop/shutdown/reload/replace) before the subsystem count and
hot-reload/live-collaboration/editor-play-mode requirements make it
expensive to retrofit — timed for whenever scripting hot-reload (Era 3)
or live collaboration (Era 4) actually starts, not before.

### 13 — Separate "engine API" from "game-facing API"

**Already true, by construction.** Every Layer 3 subsystem's public
surface *is* its trait interface
([`engine-overview.md`](../architecture/engine-overview.md#layering)) —
a game programs against `PhysicsBackend`, `canary_ui::Window`, and so on,
never against a concrete backend's internals. There's no separate
"internal engine implementation" surface currently leaking into game
code that would need a deliberate second facade layer on top of what
already exists.

### 14 — Explicit escape-hatch levels

**Already true.** [`rendering.md`](../architecture/rendering.md) already
describes exactly this layering (render graph → RHI trait → a specific
backend's extensions), and [`plugin-system.md`](../architecture/plugin-system.md#tier-b--trusted-native-c-abi)'s
Tier B exists specifically so an advanced integrator can go past the
sandboxed API without forcing that cost onto everyone else. Nothing new
to add architecturally; the review's contribution here is a useful
naming/diagramming exercise more than a design gap.

### 15 — Capability detection instead of platform stereotypes

**Accept (deferred).** Genuinely not addressed anywhere yet, and
genuinely not needed yet — this matters once mobile/web targets and a
real range of GPU capability tiers are in scope (Era 3 rendering research
and beyond per
[`long-term-roadmap.md`](../vision/long-term-roadmap.md)), not at
`v0.1.0`, where the only rendering target is desktop Vulkan. Worth a
one-line forward-pointer in [`rendering.md`](../architecture/rendering.md)'s
future-work framing so it isn't rediscovered from scratch, consistent
with how this project already handles work "this far out" (see
[`docs/roadmap/future-roadmap.md`](../roadmap/future-roadmap.md)'s stated
avoidance of assigning fake specificity to distant work).

### 16 — Keep the `v0.1.0` dependency order

**Already true.** This is agreement, not a change — and the review's one
addition ("if implementation reveals an earlier primitive is
insufficient, stop and fix it before building more on top") is, close to
verbatim, [Cloudy's own standing mandate](../roadmap/v0.1.0-plan.md#why-this-order-dependency-reasoning-not-preference):
"If Physics needs pieces of the asset system, runtime, transforms, or
scheduling first, build those first... If networking exposes flaws in
ECS ownership or serialization, fix the underlying architecture." Nothing
to adopt that isn't already the stated policy.

## What this triage actually changes right now

Per the same "docs for now" instruction that scoped this pass, nothing
here is implemented as part of this triage. Concretely:

1. **Already done as part of this pass** (small, docs-only, and directly
   unblocks the next `v0.0.9` work rather than competing with it): the
   `Transform` representation question this review's point #1 and the
   `v0.1.0` plan's own dangling "2D vs. 3D" reference both pointed at —
   resolved in [`transform.md`](../architecture/transform.md) and
   [ADR 0017](architecture-decision-records/0017-unified-transform-representation.md).
2. **New risk-register entries** (R-35, plugin subsystem-replacement
   conflict resolution; R-36, subsystem lifecycle beyond init/tick/
   shutdown) — named now, resolved later, per this project's standing
   practice.
3. **Filed as accepted-but-deferred, timed to the milestone they actually
   belong to**: the CI backend-removal-matrix check (#2/#3/#10, next
   backend feature that lands), the data-ownership table (#7, alongside
   `canary-transform`), the serialization-primitives-vs-format nuance
   (#8, `v0.0.14`), the breaking-change policy addition to ADR 0006 (#5,
   as `v0.1.0` approaches), and the capability-detection forward-pointer
   (#15, Era 3 rendering research).
4. **Everything else** is **Already true** — no action, because the
   architecture the review asks for is the architecture already
   documented; noted here so it isn't re-litigated from scratch by a
   future review that hasn't read `design-philosophy.md` and
   `engine-overview.md` first.
