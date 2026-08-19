# 0014. Change detection is the shared primitive for replication, live collaboration, hot-reload, and subsystem decoupling

**Status:** Proposed (a principle to build future subsystems against, not
yet cross-validated by a second real consumer beyond `canary-ecs` itself
— see "Consequences" for what would move this to `Accepted`)

## Context

`World::query_changed_since` (`engine/canary-ecs/src/world.rs`, `v0.0.2`)
answers one question: has this component been written since tick X? That
question turns out to already be named, independently, in four different
places in this project's own documentation, without ever being tied
together as *one* mechanism:

- [`docs/roadmap/v0.0.1-roadmap.md`](../../roadmap/v0.0.1-roadmap.md)
  and [`v0.0.2-roadmap.md`](../../roadmap/v0.0.2-roadmap.md) motivate
  change detection by naming two consumers: networking replication
  ("send only what changed") and editor tooling ("only re-cook what
  changed").
- [`docs/vision/design-philosophy.md`](../../vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly)
  already commits peer subsystems to communicating through *observable
  state* rather than direct calls — physics writes, audio observes —
  which is, mechanically, a third consumer of "what changed" that was
  never named as one.
- [ADR 0013](0013-live-collaboration-server-authoritative-topology.md)
  commits live collaboration and version-control-friendly diffing to
  being "two consequences of one architecture, not two separate
  systems" — a fourth consumer, and one that already gestures at the
  same unification this ADR makes explicit, one level up.

Each of Unity, Unreal, and Bevy solves some subset of these as genuinely
separate systems with no shared infrastructure: Unity's networking, its
version control (Unity Version Control, formerly Plastic SCM — an
async, check-in/check-out model, not live collaboration), and real-time
multi-user scene editing (only available third-party, e.g. Scene Fusion,
which describes itself as still in development) are three unrelated
products. Unreal's Multi-User Editing is closer in spirit to live
collaboration, but it is explicitly meant to *augment* a separate
Perforce/SVN/Git workflow rather than unify with it, remains a "Beta,
use caution when shipping" feature years after introduction, and — most
relevantly here — uses its own protocol, entirely unrelated to Unreal's
actual gameplay replication system. Building one didn't make the other
better. That's the retrofit-pain pattern
[`docs/research/engine-comparisons.md`](../../research/engine-comparisons.md)
and this project's own design philosophy already warn against in
general; this ADR is that warning applied specifically to "how many
times does this codebase reinvent 'what changed'."

## Decision

**`World::query_changed_since`'s `Tick`-based mechanism is the canonical
definition of "changed" for the whole engine.** Every future subsystem
that needs to answer some version of "what's different since X"
consumes this primitive rather than designing its own change-tracking
from scratch:

- **Networking replication** (Era 4,
  [`docs/architecture/networking.md`](../../architecture/networking.md)):
  a replicated component's dirty set for a given client is computed by
  `query_changed_since(client's last-acknowledged tick)`, not a
  separately designed dirty-flag or shadow-state-diff system.
- **Live collaboration** (medium-term `canary-state`, per
  [ADR 0012](0012-project-state-as-a-versionable-graph.md) and
  [ADR 0013](0013-live-collaboration-server-authoritative-topology.md)):
  for ECS-resident project data, the session server's "accepted
  operation log" is materialized as ordinary `World::insert`/`get_mut`/
  `remove` calls; the resulting `Tick`s are what drive both what gets
  broadcast to other collaborators *and* what gets serialized into the
  diffable, persisted format ADR 0013 already commits to treating as
  one artifact, not two.
- **Asset hot-reload** (Era 3,
  [`docs/architecture/asset-system.md`](../../architecture/asset-system.md)):
  for ECS-resident data affected by a reimport, "what needs re-cooking"
  is answered the same way, rather than a separate file-watcher-driven
  dirty system that happens to exist alongside the ECS's own.
- **Inter-subsystem decoupling**: the "physics writes, audio observes"
  pattern design-philosophy.md already commits to is, mechanically, a
  `query_changed_since` read on whatever component physics wrote —
  named here as what it already structurally is, not a new mechanism.

**This is a shared *definition*, not a shared *queue*.** Each consumer
keeps its own cadence, transport, and failure semantics — a network
client's last-acked tick, a collaborator's last-synced tick, and a
hot-reload watcher's last-cooked tick are independent bookkeeping,
each simply a `Tick` value a caller supplies to the same query. Nothing
about this decision implies one shared buffer or one consumer draining
before another can.

## Alternatives considered

**Let each subsystem invent its own change-tracking when it's actually
built, as the unstated default.** Rejected: this is precisely what
Unity and Unreal did, and precisely why their networking, their version
control, and their collaboration tooling don't share infrastructure or
improve each other when one is improved. It is also just quietly what
would happen without a decision recorded here — the "unowned decisions
get made implicitly, one PR at a time" failure mode
design-philosophy.md already names.

**Build a single, all-consumers event bus or message-passing system now,
speculatively, before any of the three downstream consumers
(networking, collaboration, hot-reload) exist to validate its shape
against.** Rejected as premature abstraction — exactly the pattern the
`v0.0.1` release checklist explicitly declined to do ("no speculative
API surface was added for capabilities that don't exist yet"). This ADR
commits to the *principle* now, cheaply, while it's still free to state;
it deliberately does not design speculative plumbing beyond what
`query_changed_since` already provides.

**Fold this into ADR 0007, 0010, or 0013 individually instead of a new
ADR.** Considered, but rejected: the commitment cuts across all three
(plus the ECS's own internal design) and belongs to none of them
specifically. A dedicated ADR makes the cross-cutting principle
discoverable on its own rather than buried as a paragraph inside an ADR
primarily about something else — the same reasoning that already
justified ADR 0013 as its own record instead of an amendment to ADR
0012.

## Consequences

- Future work on Era 3 (hot-reload), Era 4 (replication), and
  `canary-state`/live collaboration should each explicitly justify any
  change-tracking mechanism that *isn't* built on
  `World::query_changed_since`, rather than silently inventing one.
  `docs/architecture/networking.md` and `asset-system.md` should
  reference this ADR once each subsystem is actually designed against
  it — they don't yet, since neither has been built.
- **A real risk, not glossed over**: `Tick` is currently a simple,
  per-column-row monotonic counter — enough for "has this changed since
  tick X," which covers replication and hot-reload cleanly. It is *not*
  enough, as-is, for the kind of causality tracking a CRDT-style
  conflict-reconciliation technique might eventually want internally
  (ADR 0013 names this as "a candidate technique the server could use
  internally," not a commitment). If that need materializes during Era
  4 or the live-collaboration work, extending or partially replacing
  `Tick`'s internals (e.g., toward per-client vector clocks for the
  replication/collaboration path specifically) is the expected outcome,
  not a violation of this ADR — the commitment is to *not reinventing
  the concept of change tracking per subsystem*, not to freezing
  `Tick`'s current representation forever. Tracked as risk R-32 in
  [`docs/reviews/risk-register.md`](../../reviews/risk-register.md) so
  this doesn't get rediscovered the hard way mid-Era-4.
- This ADR moves from `Proposed` to `Accepted` once at least one real
  consumer beyond `canary-ecs` itself — most likely Era 3's hot-reload,
  being the nearest in the sequence — is actually built against
  `query_changed_since`, the same "prototype before finalizing status"
  discipline [ADR 0010](0010-component-identity-across-language-boundary.md)
  and [ADR 0012](0012-project-state-as-a-versionable-graph.md) already
  follow.
