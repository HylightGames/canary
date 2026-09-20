# Execution Model

What a system is allowed to read and write, when it's allowed to do so,
and what identity and time mean while it's running — written down as one
contract instead of left as implications scattered across
[`core-runtime.md`](core-runtime.md) and [`canary-ecs`](../../engine/canary-ecs)'s
own doc comments. This document exists because of a specific, repeated
finding: two independent external architecture reviews in September 2026
converged on the same gap `core-runtime.md`'s own "Threading & the job
system" section had already implied but never made load-bearing — the
ECS has archetype storage but not yet the data-access architecture a
scheduler needs, and building the next layer of systems around it before
fixing that risks a redesign later instead of a decision now. See the
full triage: [`docs/reviews/triage/2026-09-review-triage.md`](../reviews/triage/2026-09-review-triage.md).

This is v0.0.7 and v0.0.8's shared document — it grows with each release
that adds to this contract rather than starting a new one per release.
It covers what's real today, what each release added, and what's still
deliberately out of scope.

## The five invariants

Adopted close to verbatim from the second review's own closing argument,
because they're correct and because writing them down as rules — not
just implications — is exactly the "tooling-enforced architecture, not
documentation-only standards" value this project already holds itself
to elsewhere. Nothing currently *enforces* these mechanically; that's
future work (see "Known limitations" below), not a reason to leave them
unwritten until it exists.

1. **Ownership.** Every piece of mutable state — a component column, a
   resource, eventually a render-graph node's output — has exactly one
   system (or exactly one archetype transition path, for ECS storage
   itself) that's allowed to write it in a given tick. Shared read
   access from many places is fine; shared *write* access is the bug
   class this exists to prevent.
2. **Access.** A system declares what it reads and writes *before* it
   runs, not implicitly through what it happens to call. [`World::query`]
   and friends are the first-cut version of this — the type parameter
   itself is the declaration. The not-yet-built scheduler's entire job
   is reading these declarations to decide what can run concurrently;
   see "What v0.0.7 deliberately does not include" below.
3. **Time.** A tick is a discrete, ordered point in a `World`'s history
   ([`Tick`], [`World::advance_tick`]) — not wall-clock time, not frame
   time, not simulation time. Those are real, distinct concepts a future
   fixed/variable timestep loop will need (`WallClock`, `FrameTime`,
   `SimulationTime` — see review #1, item 4 in the triage), and conflating
   any of them with `Tick` or with each other has historically been a
   real source of engine bugs elsewhere. Not built yet — there's no frame
   loop sophisticated enough to need them — but named here so `Tick`
   isn't quietly overloaded to mean more than "ECS write ordering" before
   they exist.
4. **Identity.** A structural identity (an [`Entity`]'s index+generation,
   a component's `TypeId`) and a persistent/stable identity (a
   component's [`CanaryComponent::SCHEMA_ID`], a future asset's content
   hash, a future project/scene/world identifier) are different things
   and must not collapse into one ID type. [ADR
   0010](architecture-decision-records/0010-component-identity-across-language-boundary.md)
   already does this correctly for components; it's a standing
   constraint for `canary-assets`/`canary-state` when they're scoped,
   not a gap in current code.
5. **Side-effects.** A system's writes should be attributable to that
   system, not smeared across "whatever happened to run during this
   tick." [`World::insert`]/[`World::get_mut`]/[`World::set_erased`]
   already tag every write with the [`Tick`] it happened at for exactly
   this reason — change detection is side-effect attribution by another
   name. Deferred, buffered mutation (a command buffer) is the next real
   piece of this once anything parallel exists to need it from.

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
a natural fit for a future system-parameter injection mechanism once the
scheduler exists to inject into, not something to guess at the shape of
now), same `T: Send + Sync + 'static` bound so `World` stays `Send +
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
- Integration with `canary-core`'s `Subsystem`/`App` — `Schedule` is
  usable standalone today (see its own tests); wiring one into the
  `App` tick loop, replacing `Subsystem::tick`'s current "everything
  sequential on the main thread" default, is real next work once a
  second real subsystem beyond the ECS itself has systems worth
  scheduling this way.
- A persistent work-stealing thread pool (above).

## Commands and events

Named here, not fully built here. `canary-scheduler`'s specific design
(above) still means there's no *live race* a command buffer would need
to prevent: concurrently-running systems are, by construction, always
all read-only (multiple reads never race each other), and anything that
writes always runs alone. A command buffer becomes necessary the moment
concurrent *writes* become possible (the deliberately-deferred
limitation above) — building one now would still mean guessing at an
API shape that milestone's real access patterns should determine.
Review #1's sequencing is otherwise adopted as-is. Same reasoning for
`EventWriter`/`EventReader` (review #2, item 1) — real, useful
vocabulary worth having, deferred to whenever the first real
cross-system signal (not a direct component read) actually needs it.

## Shutdown ordering

Not part of this document's `v0.0.7` scope — this is a
`canary-core`/`App`-`Engine` concern
([`core-runtime.md`](core-runtime.md#the-appengine-bootstrap)), not an
ECS one, and today's bootstrap is a straightforward start/stop, not a
state machine. Review #2's `Created`→...→`Stopped`/`Failed` lifecycle
(item 2 in the triage) is accepted and real, and matters more once
plugins, a job system, and a renderer all have resources to tear down in
a specific order — none of which exist together yet. Filed here so it
isn't lost, not designed here.

## Known limitations

- **Nothing mechanically enforces the five invariants above, except
  Ownership/Access within `canary-scheduler` itself.** `Schedule`
  proves two conflicting systems can't share a concurrent stage; Time,
  Identity, and Side-effects remain written rules, not compiler-checked
  ones, and Ownership/Access enforcement doesn't extend beyond one
  `Schedule`'s own systems (nothing stops two *separate* schedules, or a
  schedule and code outside it, from touching the same `World` data
  concurrently). Review #2's suggestion of `cargo-deny`-style
  dependency-direction checks (item 15/16 in the triage) is the right
  *kind* of answer for the rest — worth real scoping as its own CI/xtask
  work, separate from this document.
- **The scheduler exists but doesn't parallelize concurrent writes, and
  isn't wired into `canary-core`'s `App` tick loop yet.** See "The
  scheduler" above for both.
- **`query2_mut` is the only mixed-mutability query shape.** No
  `query3_mut`, no two-mutable-one-shared, no arbitrary permutation —
  see "What's still deliberately not here" above.
