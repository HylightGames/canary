# Execution Model

What a system is allowed to read and write, when it's allowed to do so,
and what identity and time mean while it's running — written down as one
contract instead of left as implications scattered across
[`core-runtime.md`](core-runtime.md) and [`canary-ecs`](../../engine/canary-ecs)'s
own doc comments. The September 2026 architecture reviews identified
that archetype storage alone was not enough: access declarations,
scheduling, time ownership, and identity boundaries needed explicit
contracts before more subsystems relied on them. `canary-scheduler` and
the tick/identity decisions now exist; this document records their current
scope and the remaining runtime and command/message gaps. See the full
triage: [`docs/reviews/triage/2026-09-review-triage.md`](../reviews/triage/2026-09-review-triage.md).

Introduced alongside v0.0.7 and v0.0.8, this document grows with each
release that adds to the execution contract rather than starting a new
document per release. It covers what's real today and what remains
deliberately out of scope.

## The five invariants

Adopted close to verbatim from the second review's own closing argument,
because they're correct and because writing them down as rules — not
just implications — is exactly the "tooling-enforced architecture, not
documentation-only standards" value this project already holds itself
to elsewhere. The ECS and scheduler enforce some of these locally;
cross-schedule ownership and the runner's broader context remain future
work (see "Known limitations" below).

1. **Ownership.** Conflicting mutable access must never happen
   concurrently or without explicit ordering. Multiple systems may write
   the same location during a tick when the schedule orders those writes
   (for example physics → gameplay correction → animation). Shared read
   access from many places is fine. This is the semantic rule from ADR
   0021 Amendment 2; today's scheduler implements the safe solo-writer
   subset.
2. **Access.** A system declares what it reads and writes *before* it
   runs, not implicitly through what it happens to call. `SystemAccess`
   records component and resource reads/writes; the current `Schedule`
   uses the read/write class to form stages and runs every writer alone.
   It does not yet schedule disjoint writes based on the declared sets.
   Automatic access inference from query or system parameters remains
   future work. Each body can still access the whole `World` allowed by
   its closure (`&World` or `&mut World`), so the scheduler cannot verify
   that the declaration matches the body's actual component/resource
   access.
3. **Time.** A tick is a discrete, ordered point in a `World`'s history
   ([`Tick`], [`World::advance_tick`]) — not wall-clock time, frame time,
   or physics simulation time. The simulation runner owns advancement and
   advances once before the systems for a simulation run; the scheduler
   executes work without advancing time. The current `canary-runtime`
   harness follows this rule in its `EcsSubsystem`. A structured
   `RunContext` and a general fixed-step/presentation loop remain future
   runtime work; see ADR 0021 Amendment 7.
4. **Identity.** Structural identity (`Entity` index+generation and a
   component's `TypeId`), schema identity (`CanaryComponent::SCHEMA_ID`),
   authored identity, and content identity are distinct. Assets follow
   ADRs 0021 and 0022: `LogicalAssetId` names the authored asset,
   `ContentHash` names exact bytes, `CookKey` names a derived artifact,
   and an artifact storage key names where that artifact is stored.
   Current `AssetId` is provisional content identity, not a persistent
   authored reference. These types must not collapse into one ID.
5. **Side-effects.** A system's writes should be attributable to that
   system, not smeared across "whatever happened to run during this
   tick." [`World::insert`]/[`World::get_mut`]/[`World::set_erased`]
   already tag every write with the [`Tick`] it happened at for exactly
   this reason — change detection is side-effect attribution by another
   name. Commands, simulation messages, and observation events have
   distinct semantics under ADRs 0021 and 0022. Their current contract is
   summarized in [input-and-simulation.md](input-and-simulation.md);
   buffered mutation and message APIs remain future work.

## Queries

### What's implemented as of `v0.0.7`

[`World::query<T>`] (single component, since `v0.0.2`) is joined by three
new, deliberately narrow methods rather than a fully generic
`Query<D>` trait over arbitrary tuples:

- [`World::query2<A, B>`] — entities with *both* `A` and `B`, yielding
  `(Entity, &A, &B)`. Correct archetype-set intersection (not union): an
  entity missing either component is never yielded.
- [`World::query3<A, B, C>`] — the same, for three components.
- [`World::query2_mut<A, B>`] — one mutable, one shared:
  `(Entity, &mut A, &B)`, the "update `A` based on `B`" shape that's the
  canonical reason multi-component queries exist at all (`Position`
  updated from `Velocity`, the worked example in review #1 itself).

This mirrors a pattern this crate already established for a different
API: [`CanaryComponent`]'s own doc comment explains implementing "ADR
0010's manual trait impl alternative rather than a derive macro... a
manual impl was enough to validate the direction without committing to
macro infrastructure before there was a second real consumer to design
against." The same reasoning applies here: a fully general `Query<D>`
trait implemented via a tuple-arity macro (what
[`World::query2_mut`]/[`World::query3`] would generalize into) is real,
useful future work, but designing its exact shape — arbitrary read/write
combinations, arbitrary arity, filters like `Without<T>`/`Added<T>`
composed in — against zero real call sites beyond this document's own
examples would mean guessing. `query2`/`query3`/`query2_mut` are that
second real consumer in miniature: enough to unblock real systems now,
narrow enough that generalizing later is additive, not a breaking
redesign.

**On `unsafe`**: [`World::query2_mut`] is the first `unsafe` code in
`canary-ecs`. [`docs/development/coding-standards.md`](../development/coding-standards.md#unsafe-code)
names two expected boundaries for `unsafe` (platform abstraction, Tier B
FFI) and asks that it be rare enough elsewhere that its presence is
itself a signal to look closer — this is exactly that signal, flagged
here deliberately rather than left for a reviewer to notice on their
own. The specific problem: getting a `&mut TypedColumn<A>` and a
`&TypedColumn<B>` simultaneously from the same archetype's
`HashMap<TypeId, Box<dyn ColumnOps>>`, for `A != B`, is provably safe
(two different `TypeId` keys own two disjoint heap allocations — writing
through one cannot alias a read through the other) but isn't expressible
through two ordinary `HashMap` accesses, since the borrow checker
reasons about the map as a whole, not about the disjointness of two
specific keys. The `unsafe` block in `Archetype::column_pair_mut`
resolves this the standard way (a raw pointer to the map, one `get_mut`
and one `get` through it, gated by an explicit `type_id_a != type_id_b`
check that would otherwise be a real aliasing bug) — see its `// SAFETY:`
comment for the full argument. Restructuring `Archetype`'s column
storage from a `HashMap` to a `Vec` + index (enabling this via safe
slice-splitting instead) was considered and deferred: real, but a larger
and riskier change to `canary-ecs`'s well-tested internals than this
document's scope justifies for `v0.0.7` alone.

### What's still deliberately not here

- Filters composed into a query (`Without<T>`, `Added<T>`, `Changed<T>`
  as a query-shape parameter rather than [`World::query_changed_since`]
  as its own separate method) — real, deferred until the generic
  `Query<D>` trait is designed.
- Arbitrary arity and arbitrary read/write combinations beyond the three
  methods above.
- Query result caching/pre-computed join tables beyond the existing
  per-type archetype cache ([`World`]'s own docs) — not a measured
  bottleneck, so not built ahead of evidence it's needed, per this
  crate's existing "don't over-optimize archetype storage yet" precedent
  (review #1, item 6).

## Resources

Real gap, confirmed by inspection rather than assumed: no `Resource`-like
concept existed anywhere in `canary-ecs` or `canary-core` before
`v0.0.7`. A `Resource` is globally-unique, engine-owned state addressed
by type rather than by entity — time, a future asset server handle, a
future seeded RNG (see the triage's accepted item on engine-owned RNG) —
the same "at most one per `World`" shape a component has "at most one
per entity."

[`World::insert_resource<T>`]/[`World::resource<T>`]/
[`World::resource_mut<T>`]/[`World::remove_resource<T>`]/
[`World::contains_resource<T>`] mirror the existing
`insert`/`get`/`get_mut`/`remove` component API deliberately — same
`Option`-returning convention (no panicking accessor exists yet; that's
a natural fit for a future system-parameter injection mechanism, not
something to guess at before a reusable runtime API has a real consumer),
same `T: Send + Sync + 'static` bound so `World` stays `Send +
Sync`, and the same [`Tick`]-based change detection every component
column already has
([`World::resource_changed_since`] mirrors [`World::query_changed_since`]
exactly). A resource is implemented as a single-slot analogue of a
component column rather than a new storage concept — consistent with
"primitives that compound rather than accumulating as separate
features."

## The scheduler

### What's implemented as of `v0.0.8`

`canary-scheduler` (a new crate, depending only on `canary-ecs`'s public
API — no privileged access to `World`'s internals, per "no privileged
built-ins") implements the Access invariant concretely:

- `SystemAccess` (`engine/canary-scheduler/src/access.rs`) — a system's
  declared reads/writes, component and resource types tracked
  separately. Built as an explicit, chainable declaration
  (`.reads::<T>()`/`.writes::<T>()`/`.reads_resource::<T>()`/
  `.writes_resource::<T>()`) rather than inferred automatically from a
  system function's parameter types — inferring access the way a fuller
  `SystemParam`-style framework would is real future work, but doing it
  correctly for arbitrary query shapes is a substantially bigger
  undertaking than this first release attempts.
- `Schedule` (`engine/canary-scheduler/src/schedule.rs`) — greedily
  batches systems, in registration order, into *stages*: a stage is
  either one or more read-only systems (always safe to run concurrently
  with each other, regardless of what they read — multiple shared
  borrows never conflict) or exactly one system that writes anything.
  Multi-system stages run each system on its own thread via
  `std::thread::scope`, joined before the next stage starts.

This is a genuine, tested case of the Ownership and Access invariants
being mechanically enforced rather than just documented: two systems
that both write the same component type cannot end up in the same
stage — `Schedule::compute_stages` is built (and tested) so that's not
representable, not just discouraged by convention.

**The one deliberately narrow limitation**: two *write* systems never
run concurrently, even when their `SystemAccess` can prove they're
disjoint (one writes only `Position`, the other only `Velocity`, say).
Doing that safely means handing each system its own provably-disjoint
view of `World` rather than an exclusive `&mut World` — a real,
substantially larger `unsafe` undertaking than `query2_mut`'s
already-narrow column-pair split (see "Queries" above), closer in scope
to Bevy's `UnsafeWorldCell`/`SystemParam` machinery than to anything
this project has built so far. Every write system runs alone,
sequentially relative to everything else, which is always *correct*,
just not maximally parallel. Revisit once profiling actually shows this
mattering, not before — consistent with this crate's own "don't
over-optimize archetype storage yet" precedent (review #1, item 6).

Threads are spawned fresh per multi-system stage rather than pulled from
a persistent work-stealing pool — real spawn overhead on every parallel
stage of every tick, correct but not what
`docs/architecture/core-runtime.md#threading--the-job-system`'s "one
worker per physical core" target design ultimately wants. Swapping in a
persistent pool (hand-rolled or an external work-stealing crate) later
is an internal change to `Schedule::run`, not a change to `SystemAccess`
or how systems are registered.

### What's still deliberately not here

- Concurrent disjoint writes (above).
- Automatic access inference from a system function's signature.
- A reusable runtime composition API. `canary-core::App` calls
  subsystems sequentially; the private `canary-runtime` harness owns an
  `EcsSubsystem` whose tick runs a `Schedule`. This proves composition
  internally but is not yet the supported game-consumer entry point.
- A persistent work-stealing thread pool (above).

## Commands and events

The semantic contracts are locked by ADR 0021 Amendment 8 and ADR 0022
Clarification 2. A Command requests mutation at a defined point; a
Simulation message is ordered, deterministic communication that may
affect simulation; an Observation event only notifies UI, audio, or
tooling and is not recorded as authoritative state. These distinctions
serve structural mutation and subsystem communication even while writes
are serialized, so they are broader than a prerequisite for concurrent
writers. Queueing, ordering, retention, and delivery APIs remain deferred
until real consumers establish their needs; see
[input-and-simulation.md](input-and-simulation.md).

## Shutdown ordering

Not part of the ECS execution contract — this is a
`canary-core`/`App`-`Engine` concern
([`core-runtime.md`](core-runtime.md#the-appengine-bootstrap)), not an
ECS one. Review #2's `Created`→...→`Stopped`/`Failed` lifecycle (item 2
in the triage) is accepted and will matter as plugin, job-system, and
renderer lifetimes compose. The current `Subsystem` API still has only
`init`, `tick`, and `shutdown`; richer lifecycle semantics remain open
(risk R-36). Filed here so it isn't lost, not designed here.

## Known limitations

- **Access declarations are not tied to the system body.** All writer
  bodies currently run alone, so a metadata mismatch does not enable
  concurrent writes today. It would become a correctness risk if
  disjoint-write scheduling or mixed read/write stages used those sets
  without first verifying actual access. Typed system parameters or
  another verifiable access mechanism should be designed before expanding
  concurrency or making the scheduler a long-term plugin/game API.
- **Enforcement stops at schedule boundaries.** Time, Identity, and
  Side-effects remain written rules, not compiler-checked ones, and
  Ownership/Access enforcement doesn't extend beyond one `Schedule`'s
  own systems (nothing stops two *separate* schedules, or a schedule and
  code outside it, from touching the same `World` data concurrently).
  Review #2's suggestion of `cargo-deny`-style
  dependency-direction checks (item 15/16 in the triage) is the right
  *kind* of answer for the rest — worth real scoping as its own CI/xtask
  work, separate from this document.
- **The scheduler runs inside the private headless harness but is not a
  reusable consumer API.** `App` still ticks subsystem instances
  sequentially; concurrent disjoint writes and a shared worker pool also
  remain open. See "The scheduler" above.
- **`query2_mut` is the only mixed-mutability query shape.** No
  `query3_mut`, no two-mutable-one-shared, no arbitrary permutation —
  see "What's still deliberately not here" above.
