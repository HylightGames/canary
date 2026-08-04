# 0012. Project state as an explicit, identifiable, versionable graph

**Status:** Proposed (records a founding principle and a layered scope
this project commits to now; the hardest mechanism — real-time
collaborative merge — is explicitly left an open question, not decided
here. See [`docs/architecture/state-and-versioning.md`](../../architecture/state-and-versioning.md)
for the full design and the reasoning behind `Proposed` rather than
`Accepted`.)

## Context

Engines that represent a project as a folder of opaque, engine-specific
files pay for it later in exactly the way this project's own
[research](../../research/engine-comparisons.md) already documents for
multiplayer bolted on late: Git can show that a binary scene file
changed, but not what changed inside it, which makes team collaboration
on shared scenes and assets a well-known, persistent source of friction
across the industry. Retrofitting a better data model onto an engine
that didn't design for one from the start is expensive in direct
proportion to how much content already exists in the old model.

## Decision

Adopt, as a founding principle recorded in
[`docs/vision/design-philosophy.md`](../../vision/design-philosophy.md#state-is-explicit-identifiable-and-versionable):

> All important engine state must be explicitly represented, identifiable,
> serializable, and versionable.

Concretely, and layered by how far out each part is (full detail in
[`state-and-versioning.md`](../../architecture/state-and-versioning.md)):

- **Now**: authored data formats should prefer diffable, mergeable
  structured text over opaque binary where practical, and every authored
  object that might be referenced from elsewhere gets a **persistent
  identity**, explicitly distinct from `canary-ecs`'s runtime `Entity`
  identity (see the linked document for why conflating the two would be
  a mistake).
- **Medium-term** (a future `canary-state` crate, depending on the
  archetype ECS migration and asset pipeline): a persistent-identity
  registry, authored-level change tracking (distinct from the ECS's own
  runtime change detection), unknown-schema preservation building on
  [ADR 0010](0010-component-identity-across-language-boundary.md), and a
  real marketplace package format with migration rules.
- **Long-term, genuinely open**: undo/redo and time-travel debugging as
  consequences of the above; real-time collaborative editing, for which
  two credible directions exist (server-authoritative operation
  broadcast, consistent with [ADR 0007](0007-networking-and-multiplayer-model.md)'s
  authority model; or CRDT-based leaderless merge, bootstrapped on an
  existing pure-Rust library — Automerge or Loro were identified as
  credible candidates, see
  [`technology-evaluations.md`](../../research/technology-evaluations.md#local-first-collaborative-state-crdts) —
  rather than a bespoke implementation). **Which of these two Canary
  eventually wants is not decided by this ADR.**

## Alternatives considered

**Do nothing; treat this as an implementation detail to solve whenever a
merge-conflict problem is actually painful.** Rejected: this is precisely
the retrofit-cost argument in "Context" above — the principle (explicit,
identifiable, versionable state) is nearly free to hold as a constraint
now on formats and identity schemes that don't exist yet, and expensive
to impose later on scenes, assets, and save data that already exist
without it.

**Decide the collaboration mechanism (server-authoritative vs. CRDT) now,
rather than leaving it open.** Rejected: this is a genuinely hard
distributed-systems design question with real tradeoffs (coordination
requirements, offline support, implementation complexity) that this
project has no implementation experience to ground a confident choice in
yet. Marking this `Proposed` and naming the two real options, rather than
picking one from a desk review, is the same discipline
[ADR 0010](0010-component-identity-across-language-boundary.md) already
established for component identity — a decision this consequential
deserves prototyping, not speculation.

**Build a bespoke CRDT implementation if that direction is eventually
chosen.** Rejected as a default: mature, pure-Rust, permissively-licensed
prior art exists (Automerge, Loro) — reinventing this would be the same
unjustified-NIH mistake this project already declined to make for
graphics (`wgpu`) and physics (Rapier).

## Consequences

- No new crate exists yet; this ADR is a scope and principle commitment,
  not an implementation task for `v0.0.1` or even necessarily `v0.0.2`.
- Future asset-pipeline and archetype-ECS design work
  ([`asset-system.md`](../../architecture/asset-system.md),
  [`core-runtime.md`](../../architecture/core-runtime.md)) should account
  for persistent identity and diffable formats from the start, rather
  than needing a later migration.
- This ADR should be revisited — updated to `Accepted` for whichever
  parts have been prototyped, or superseded — once real `canary-state`
  design work begins, rather than left as a permanently-open proposal
  once there's implementation experience to ground it in.
