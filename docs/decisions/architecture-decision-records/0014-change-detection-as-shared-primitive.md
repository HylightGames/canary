# 0014. Change detection as a shared primitive for replication, live collaboration, hot-reload, and subsystem decoupling

**Status:** Proposed (a principle to build future subsystems against, not
yet cross-validated by a second real consumer that actually stresses its
semantics — see "Consequences" for the two-part bar for `Accepted`)

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
primitive for detecting *ECS-resident component mutation* — not, more
broadly, "changed" for the whole engine.** That distinction is worth
stating precisely rather than leaving it for a future reader to infer: a
source asset changing on disk, a collaborator's edit being accepted or
rejected, and a component's in-memory value being overwritten are three
different kinds of "change," and only the last one is what `Tick`
observes directly. What this ADR actually commits to is narrower and
more durable than "everything": every future subsystem whose own
mutation-tracking would otherwise duplicate "did this component's value
change" consumes `query_changed_since` for exactly that question,
rather than inventing a parallel mechanism for the same fact.

- **Networking replication** (Era 4,
  [`docs/architecture/networking.md`](../../architecture/networking.md)):
  a replicated component's dirty set for a given client is computed by
  `query_changed_since(client's last-acknowledged tick)`, not a
  separately designed dirty-flag or shadow-state-diff system. See
  "Known gaps" below for what this does *not* yet give replication.
- **Live collaboration** (medium-term `canary-state`, per
  [ADR 0012](0012-project-state-as-a-versionable-graph.md) and
  [ADR 0013](0013-live-collaboration-server-authoritative-topology.md)):
  for the ECS-resident *result* of an accepted operation,
  `query_changed_since` is what tells the session server which
  components actually changed value and therefore need broadcasting to
  other collaborators, or serializing to the diffable persisted format.
  This is deliberately narrower than ADR 0013's "operation log" itself —
  see "Known gaps."
- **Asset hot-reload** (Era 3,
  [`docs/architecture/asset-system.md`](../../architecture/asset-system.md)):
  for ECS-resident data *downstream* of an asset — a mesh handle, a
  material reference, anything the asset pipeline eventually writes into
  a component — `query_changed_since` is how a consumer of that data
  notices it changed. The upstream question, "this source file changed,
  therefore which derived assets need rebuilding," is a
  dependency-graph problem the asset system owns; this ADR doesn't reach
  that far up the pipeline, only the ECS-resident tail end of it.
- **Inter-subsystem decoupling**: for *persistent* state one subsystem
  writes and a peer reads later — physics writing a `Transform`, audio
  reading it — `query_changed_since` is that read. This does not extend
  to genuinely *transient* signals (a collision starting and ending
  within one tick window, an impulse applied and gone) that a polling
  read could miss or coalesce incorrectly; those need an actual event
  mechanism, which this ADR does not attempt to replace. See "Known
  gaps."

**This is a shared *definition*, not a shared *queue*.** Each consumer
keeps its own cadence, transport, and failure semantics — a network
client's last-acked tick, a collaborator's last-synced tick, and a
hot-reload watcher's last-cooked tick are independent bookkeeping,
each simply a `Tick` value a caller supplies to the same query. Nothing
about this decision implies one shared buffer or one consumer draining
before another can.

## Known gaps this ADR does not resolve

Named directly, because leaving them implicit is exactly how an ADR gets
misused later as license for more than it actually decided:

- **Component removal and entity destruction are not covered.** When a
  component is removed (`World::remove`) or an entity is despawned, the
  row it occupied is gone — there is nothing left for
  `query_changed_since` to report, because the archetype it would have
  been read from no longer has that row. A client that misses the tick
  a `Transform` was removed at has no way to learn "entity 42 no longer
  has a `Transform`" from this primitive alone, and replication
  genuinely needs that (a despawned entity has to disappear client-side,
  not linger). This ADR does not provide it. The likely shape — a
  durable removal/destruction log or tombstone mechanism, consulted
  *alongside* `query_changed_since` rather than folded into it, in the
  spirit of how other ECS designs expose removed-component notification
  as its own stream rather than trying to answer it via value polling —
  is future `canary-ecs` work, not resolved here. Tracked as risk R-33
  in [`docs/reviews/risk-register.md`](../../reviews/risk-register.md).
- **This is not an operation log.** A `Tick` answers "does the current
  value differ from the value as of tick X" — it does not preserve
  intermediate values, the order distinct writers produced them in, or
  authority/conflict metadata. A value going `10 -> 20 -> 10` between
  tick 99 and tick 103 reports as "unchanged since 99," even though two
  writes happened in between. That is exactly the right, cheap answer
  for state replication, where only the current value matters to a
  client. It is not sufficient, on its own, for collaboration history,
  undo provenance, or conflict reconciliation — those need the actual
  sequence of operations, which
  [ADR 0013](0013-live-collaboration-server-authoritative-topology.md)'s
  operation log is responsible for, as a genuinely separate artifact
  from whatever `query_changed_since` reports about the resulting
  state, not a byproduct derivable from `Tick`s after the fact.

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
  *value-mutation* tracking that isn't built on `query_changed_since`.
  The "Known gaps" above are exactly the parts of "what changed" this
  ADR does *not* yet claim to cover — building a removal log or an
  event system for those is not a violation of this ADR, it's the
  complementary work this ADR deliberately declines to design
  speculatively, per the same reasoning in "Alternatives considered."
  `docs/architecture/networking.md` and `asset-system.md` should
  reference this ADR once each subsystem is actually designed against
  it — they don't yet, since neither has been built.
- **Precisely, to close an ambiguity an earlier draft of this ADR left
  open**: `Tick` is one world-wide, monotonically increasing counter
  (`World::advance_tick`); each component's storage records, per row,
  the `Tick` value in effect the last time that row was written. It is
  *not* an independent per-column or per-row counter — every `Tick`
  value is comparable against every other, engine-wide, which is what
  makes `query_changed_since(since)` meaningful for a caller that never
  observed the intervening writes to any *other* component. This
  representation is a **local, single-timeline ordering primitive**. It
  is a different thing from a **distributed causality primitive** (e.g.
  per-client vector clocks), which is what a CRDT-adjacent
  conflict-reconciliation technique would actually need if
  `canary-state` goes that direction — see the next point.
- **A real risk, not glossed over**: `Tick`'s current representation is
  enough for "has this changed since tick X" — replication and
  hot-reload, once "Known gaps" above is also addressed. It is *not*
  enough, as-is, for the causality tracking a CRDT-style
  conflict-reconciliation technique might eventually want internally
  (ADR 0013 names this as "a candidate technique the server could use
  internally," not a commitment). If that need materializes during Era
  4 or the live-collaboration work, the expected outcome is adding a
  distributed-causality primitive *alongside* `Tick` — not replacing
  it, since replication's needs for a simple local ordering don't go
  away — which is a different temporal dimension underneath the same
  higher-level "consume the canonical primitive, don't reinvent one"
  commitment, not a violation of it. Tracked as risk R-32 in
  [`docs/reviews/risk-register.md`](../../reviews/risk-register.md).
- This ADR moves from `Proposed` to `Accepted` once **two** things are
  true, not one: (1) at least one real consumer beyond `canary-ecs`
  itself — most likely Era 3's hot-reload, being nearest in the
  sequence — is actually built against `query_changed_since`, *and* (2)
  that consumer's experience demonstrates the primitive's semantics are
  actually sufficient for its needs, not merely that it compiles against
  them. Hot-reload is a real but comparatively easy validation — it
  could plausibly succeed while never exercising either "Known gap"
  above. Networking replication (Era 4) is the harder, more honest test
  of sufficiency, precisely because it needs removal/destruction
  detection this ADR admits it doesn't yet provide. `Accepted` status
  should wait for whichever consumer actually stresses the primitive,
  not just whichever lands first — the same "prototype before
  finalizing status" discipline
  [ADR 0010](0010-component-identity-across-language-boundary.md) and
  [ADR 0012](0012-project-state-as-a-versionable-graph.md) already
  follow.
