# Design Philosophy

This document explains *how* Canary makes decisions, which matters more than
any individual decision, because the individual decisions will keep coming
long after this foundational session ends.

## First principles over precedent

The instruction that shaped this entire foundation was: don't clone Unreal,
Unity, or Godot — analyze what they do well and badly, and reason from first
principles. Concretely, that means every architecture document in
[`docs/architecture/`](../architecture/) is expected to state:

1. The problem being solved.
2. How the three incumbent engines (plus relevant Rust-ecosystem projects
   like Bevy) approach it, and the tradeoff each made.
3. Why Canary makes a different (or the same) tradeoff, explicitly.

"First principles" does not mean "reinvent everything." Where an existing
tool already solves a problem well and its tradeoffs match Canary's goals
(wgpu for cross-API graphics, Rapier/Jolt for physics, Wasmtime for the WASM
component runtime), Canary depends on it rather than rewriting it, and says so
in an ADR. Reasoning from first principles about *architecture* is orthogonal
to a NIH ("not invented here") reflex about *implementation* — the latter is
exactly the kind of premature effort this project explicitly avoids in its
early milestones.

## Modularity is the mechanism, not just the marketing

"Extremely modular" and "AAA-capable while remaining accessible" are only
compatible if modularity is real: a subsystem that isn't enabled shouldn't
appear in compile times, binary size, or API surface. Concretely:

- Subsystems are separate crates behind trait interfaces
  (`docs/architecture/core-runtime.md`), not `#[cfg(feature = ...)]` soup
  inside one crate.
- The plugin system (`docs/architecture/plugin-system.md`) is not a
  bolt-on scripting layer; the *editor itself* is expected to be built as a
  plugin host, so "is the plugin API good enough for real work" gets
  continuously tested by the project's own tooling (see
  `docs/ui/editor-design.md`).
- Replacing a subsystem should not require forking the engine. If a
  contributor wants Jolt instead of Rapier, or a custom RHI instead of the
  wgpu-backed one, that should be a new crate implementing an existing trait,
  not a patch to core.

## Decisions are owned, not deferred

This foundation was explicitly built with a mandate to make real technical
decisions rather than presenting a menu of options and waiting. Every ADR in
this repository reflects a decision that was actually made, with reasoning,
not a "here are three options, TBD." Where a decision is genuinely deferred
(the editor's UI toolkit, for instance), that deferral is itself documented
as a decision, with the criteria that will resolve it later.

This matters for a multi-year, multi-contributor project specifically because
the alternative failure mode is worse: unowned decisions get made implicitly,
one PR at a time, by whoever touches the code next, and by the time anyone
notices there's a de facto architecture nobody chose on purpose.

## Honesty about scope beats impressive-sounding scope

A recurring temptation in a foundation-laying session like this one is to
describe more as "done" than is actually true — a fuller-sounding ECS
description, a plugin loader that "supports" WASM when only the interface is
defined. Canary's documentation is written to resist that temptation:
architecture docs describe the *target* design; roadmap docs are explicit
about what's implemented today versus planned; and code comments flag
placeholder implementations as placeholders (see, for example, the
`canary-ecs` crate's module docs). A false "done" is more expensive to a
multi-year project than an honest "not yet," because the former gets built on
top of before anyone notices it needs redoing.

## AI-ready architecture

"AI-ready" is one of the vaguer pillars in the founding brief, so this
foundation resolves it into two concrete, separable commitments rather than
one vague promise:

1. **Legible to AI-assisted development, today.** Consistent module
   structure, exhaustive doc comments on public APIs, ADRs that state
   reasoning (not just outcomes), and a coding-standards document that a
   coding assistant — or a new human contributor — can actually follow. This
   is not aspirational: it's why this foundation itself leans so heavily on
   ADRs and doc comments rather than tribal knowledge.
2. **Native hooks for in-game and in-editor AI/ML, later.** A future
   `canary-ai` subsystem (tracked in
   [`docs/roadmap/future-roadmap.md`](../roadmap/future-roadmap.md), not part
   of v0.0.1) is expected to expose model inference (e.g. via ONNX Runtime or
   a WASI `wasi-nn`-style interface — see
   [`docs/research/technology-evaluations.md`](../research/technology-evaluations.md))
   for NPC behavior, procedural content, and editor copilot-style tooling.
   This is deliberately scoped as a subsystem behind the same plugin
   boundaries as everything else, not a special-cased core dependency.

## Community and marketplace as a security model, not just a feature

A plugin marketplace is usually described as a feature. Canary treats it as a
security problem first: the sandboxed WASM tier
(`docs/architecture/plugin-system.md`) exists *so that* a marketplace can
exist without every installed mod being an unaudited native-code supply-chain
risk. The feature (a marketplace) falls out of the architectural decision
(sandboxing), not the other way around.

## Determinism and multiplayer as foundations, not add-ons

Retrofitting deterministic simulation or clean client/server separation onto
an engine not designed for it is one of the most consistently painful
experiences in game development (see
[`docs/research/engine-comparisons.md`](../research/engine-comparisons.md)).
Canary's ECS and networking architecture are designed together from the
start — component replication is a first-class ECS concept, not a
side-channel bolted onto an existing single-player simulation loop — even
though full rollback-netcode support is explicitly out of scope for v0.0.1.

## State is explicit, identifiable, and versionable

A later addition to this list, but the same kind of principle as the two
above: **all important engine state must be explicitly represented,
identifiable, serializable, and versionable.** Most engines treat a
project as a folder of opaque files, which is precisely why team
collaboration on shared scenes is such a well-known source of friction
industry-wide — a binary scene file's Git diff tells you it changed, not
what changed, and a merge conflict in it is unresolvable by reading it.
Canary holds this as a constraint from the start specifically because it's
nearly free to hold now and expensive to retrofit once scenes, assets, and
save data already exist without it — the same "design together from the
start" logic as the multiplayer/ECS pairing above, applied to the project
itself rather than to the running simulation. See
[`docs/architecture/state-and-versioning.md`](../architecture/state-and-versioning.md)
and [ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md)
for the full design — including the deliberate, explicit distinction
between the ECS's fast *runtime* entity identity and a project's stable
*persistent* identity, which this principle depends on not conflating.

## Subsystems bind through interfaces, never call each other — or a third party — directly

Every replaceable-backend pattern in this project (RHI/`wgpu`, physics/
Rapier, `CanaryUI`/`egui`) is a specific instance of one general rule,
worth stating on its own: **no Canary crate calls a third-party library,
or another Canary subsystem's internals, directly.** It goes through a
trait Canary itself defines. Concretely, two things this actually
requires, not just implies:

1. **No direct third-party calls.** Engine and gameplay code call
   `PhysicsBackend`, never `rapier3d` directly; the render graph calls
   the RHI trait, never `wgpu` directly. This is already how
   [`rendering.md`](../architecture/rendering.md) and
   [`physics.md`](../architecture/physics.md) are designed.
2. **No direct peer-subsystem calls, either — and this is the part
   that's easy to state and easy to accidentally violate.** Physics
   should not import `canary-audio` and call a function on it to play an
   impact sound; the ECS-native pattern is that physics writes state
   (an impact event/component), and audio observes it — the same
   "communicate through shared, observable state, not direct function
   calls" discipline that already makes the render graph work
   ([`engine-overview.md`](../architecture/engine-overview.md#how-a-frame-is-expected-to-flow-target-design-post-era-2)).
   Two peer subsystems that import each other's crates directly have
   quietly created a dependency the architecture was supposed to prevent.

**A trait that leaks a concrete third-party type in its public signature
has not actually achieved swappability, even if it compiles against
multiple backends.** If `PhysicsBackend`'s public API takes or returns a
`rapier3d::RigidBody` anywhere, replacing Rapier with Jolt requires every
caller of that API to change too — the abstraction didn't abstract
anything. Canary's own types cross every trait boundary; a third-party
type is allowed *inside* a backend's implementation of a trait, never in
the trait's own public signature. This is a concrete, checkable property
worth reviewing for on every PR that touches a backend-facing trait, not
just an aspiration — see
[`docs/development/coding-standards.md`](../development/coding-standards.md).

## What "professional-grade" means for a pre-1.0 project

It does not mean "feature complete." It means: a build system that works the
same way for every contributor, CI that catches regressions before review,
documentation that matches what the code actually does, a license and
governance model that won't change out from under adopters, and a versioning
scheme that tells the truth about stability. Those properties are achievable
on day one, and this foundation is built to have all of them before a single
gameplay feature exists.
