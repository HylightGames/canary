# Canary Engine `v0.0.2` — Release Notes

**Released:** 2026-08-18
**Tag:** `v0.0.2`
**Nature of this release:** one subsystem, done properly. `v0.0.2` has a
single focus — the archetype ECS migration `v0.0.1` deliberately deferred
— rather than spreading effort across several half-finished pieces.

## What `v0.0.2` is

`v0.0.2` is where Canary's ECS stops being a documented-but-unbuilt target
design and becomes real, working, tested code:

- **Archetype-based component storage.** Entities that share a
  component-type signature are stored contiguously — one packed column
  per component type — replacing the `v0.0.1` placeholder's
  `HashMap<TypeId, HashMap<u32, Box<dyn Any + Send + Sync>>>`. This is the
  same family of design used by Bevy ECS, EnTT, and Unity DOTS, for the
  same reason: the cache-locality argument isn't engine-specific, it's a
  property of modern CPUs.
- **Cached queries.** `World::query` resolves which archetypes to scan
  through an incrementally maintained `TypeId -> Archetype` index, not a
  fresh linear scan of every archetype's signature on every call.
- **Change detection as a first-class query filter.** `World::query_changed_since`
  answers "has this component been written since tick X?" — the exact
  question networking replication (send only what changed) and editor
  tooling (only re-cook what changed) both need. Ticks correctly survive
  an entity moving to a different archetype because some *unrelated*
  component was added or removed, which was the subtlest part of this to
  get right and is covered by a dedicated test.
- **A first prototyped cut of stable component identity**
  ([ADR 0010](docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md),
  now `Accepted`). `TypeId` remains a host-internal fast-path lookup, as
  planned; a `CanaryComponent` trait and a `World::register_component`
  registry give components a stable, namespaced string+version identity
  for whenever a Tier A plugin, a replication message, or a marketplace
  tool needs to name one without ever observing `TypeId` directly.
- **13 new tests** (19 total in `canary-ecs`, up from 6 — all 6 original
  ones pass completely unmodified), including a `proptest` that runs
  arbitrary insert/remove/despawn sequences across multiple entities and
  component types, specifically to catch archetype-bookkeeping bugs a
  handful of hand-picked examples could miss.

## What this release deliberately is not

- **Not the parallel job-stealing scheduler.** The ECS now has the
  data-access shape a scheduler would need (archetype tables, change
  ticks), but no scheduler exists yet — see
  [`docs/architecture/core-runtime.md#threading--the-job-system`](docs/architecture/core-runtime.md#threading--the-job-system).
- **Not a Tier A (WASM) consumer of the identity registry.**
  `World::register_component`/`type_id_for_schema` is the seam a Tier A
  loader will eventually use to resolve a plugin-declared schema id — but
  no such loader exists yet in this release.
- **Not rendering, physics, networking, real windowing, or an editor.**
  Exactly as not-in-scope as they were for `v0.0.1`.

## Try it

```sh
git clone <this repository>
cd canary
git checkout dev
cargo build --workspace
cargo test --workspace
cargo run -p canary-runtime
```

See [`README.md`](README.md) for the full picture and
[`CONTRIBUTING.md`](CONTRIBUTING.md) to get involved.

## What's next: goals for `v0.0.3`

Per the sequencing already recorded in
[`docs/roadmap/status.md`](docs/roadmap/status.md#v003-sequenced-not-deeply-scoped-yet),
Tier A (sandboxed WASM/Wasmtime) plugin loading is next in line — it
depends on Tier B (done since `v0.0.1`) and benefits from `v0.0.2`'s
component-identity groundwork landing first, which it now has. That
sequencing is being revisited alongside a broader architecture discussion
before `v0.0.3` is actually scoped in detail; if it changes, this section
is exactly what gets updated to say so, on the record, the same way
`v0.0.1`'s own scope revision was.

## Thanks

Built and reviewed as a single continuous effort, picking up cleanly from
`v0.0.1`'s foundation via the documented architecture and ADRs rather
than from memory of how it was built.
