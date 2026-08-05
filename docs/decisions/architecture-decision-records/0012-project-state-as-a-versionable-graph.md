# 0012. Project state as an explicit, identifiable, versionable graph

**Status:** Proposed for identity, package format, and migration rules;
the real-time collaboration mechanism specifically is now **Accepted** —
see [ADR 0013](0013-live-collaboration-server-authoritative-topology.md),
which resolves the topology/authority question this ADR originally left
open (server-authoritative, not peer-to-peer or CRDT-based as the
top-level architecture), without changing anything else recorded here.
See [`docs/architecture/state-and-versioning.md`](../../architecture/state-and-versioning.md)
for the full design and the reasoning behind the remaining `Proposed`
scope.

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
- **Long-term**: undo/redo and time-travel debugging as consequences of
  the above; real-time collaborative editing, now resolved at the
  topology/authority level as server-authoritative
  ([ADR 0013](0013-live-collaboration-server-authoritative-topology.md)) —
  a self-hostable session server is authoritative over which operations
  are accepted, their ordering, conflict resolution, and permissions;
  clients submit requests, never force state. CRDT-style merge algorithms
  (Automerge, Loro — see
  [`technology-evaluations.md`](../../research/technology-evaluations.md#local-first-collaborative-state-crdts))
  remain a candidate technique the server could use *internally* to
  reconcile near-simultaneous operations, not the top-level architecture.

## Alternatives considered

**Do nothing; treat this as an implementation detail to solve whenever a
merge-conflict problem is actually painful.** Rejected: this is precisely
the retrofit-cost argument in "Context" above — the principle (explicit,
identifiable, versionable state) is nearly free to hold as a constraint
now on formats and identity schemes that don't exist yet, and expensive
to impose later on scenes, assets, and save data that already exist
without it.

**Decide the collaboration mechanism (server-authoritative vs. CRDT) from
a desk review, without implementation experience to ground it in.**
Originally rejected on exactly that basis: this is a genuinely hard
distributed-systems design question, and picking a direction without
either implementation experience or clear product direction would have
been speculation dressed up as a decision — the same discipline
[ADR 0010](0010-component-identity-across-language-boundary.md) already
established for component identity. **This was resolved not by
implementation experience but by explicit direction from the project
owner**, which is a legitimate way for an architectural question to move
from open to decided that this project's process should recognize as
readily as it recognizes prototyping — see
[ADR 0013](0013-live-collaboration-server-authoritative-topology.md).
The underlying caution above still applies to whatever this ADR and
ADR 0013 *haven't* yet resolved (wire protocol, permission-model
specifics): those remain open questions this project has no standing to
decide from a desk review either.

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
