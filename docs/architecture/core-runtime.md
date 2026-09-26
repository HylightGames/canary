# Core Runtime

Covers `canary-core` and `canary-ecs`: the application lifecycle, ECS data
model, and boundaries between the engine core and the subsystems composed
above it. Windowing and rendering exist in other crates; reusable consumer
runtime composition and networking remain separate work.

## The `App`/`Engine` bootstrap

`canary-core` defines the entry point every Canary program (game, headless
server, editor, or test harness) shares:

```rust
// Illustrative composition. `canary-runtime` currently demonstrates
// this with a private headless EcsSubsystem; it is not yet a reusable
// game-runtime library. A game subsystem owns and runs its Schedule.
let mut app = canary_core::App::new();
app.add_subsystem(some_crate::SomeSubsystem::default());
app.run(|| {
    // Poll the platform here and return false when the app should stop.
    false
})?;
```

`App` owns the top-level lifecycle (init → main loop → shutdown) and a
registry of **subsystems** — the Layer 3 pieces from
[engine-overview.md](engine-overview.md). A subsystem is deliberately a small
trait with `init`, `tick(dt)`, and `shutdown` hooks. Schedule construction
and system ordering belong to the subsystem or a higher-level runtime
composition crate; `canary-core` does not own a scheduler. Adding a subsystem
is opt-in and explicit, never implied by which crates happen to be linked.
`App::add_plugin_dir` currently records a path only; it does not load plugins.

Shutdown is panic-safe: a panicking `shutdown` does not skip the
remaining subsystems (each is shut down under `catch_unwind` in reverse
registration order, the first panic payload re-raised after every
subsystem has shut down), and a panicking `tick` shuts everything down
the same way before the panic resumes to the caller — one subsystem's
broken teardown can neither leak the rest nor silently swallow the
failure into an imagined healthy state.

## Logging & diagnostics

Structured logging (via the `tracing` ecosystem's conventions — spans and
fields, not `println!`-style strings) is a Layer 2 concern precisely because
every other subsystem needs it before it needs anything else. Design
commitments:

- Log lines are structured (key-value fields), not just formatted strings,
  so tooling (including future editor log panels and AI-assisted debugging,
  per [design-philosophy.md](../vision/design-philosophy.md#ai-ready-architecture))
  can filter and query them.
- Subsystems log through spans scoped to their name (`ecs`, `render`,
  `net::replication`, ...), so a contributor can turn one subsystem's
  verbosity up without drowning in everyone else's.
- No subsystem panics on recoverable errors. `canary-core` defines the
  project's error-handling convention (typed errors via `thiserror` for
  library code, `anyhow`-style context at application boundaries) — see
  [`docs/development/coding-standards.md`](../development/coding-standards.md#error-handling).

## Memory management philosophy

Canary does not adopt a single global allocation strategy; it adopts a
*policy for choosing one per subsystem*, because "one size fits all" memory
management is precisely the kind of decision that looks fine at prototype
scale and becomes a rewrite at production scale:

- **Steady-state, high-frequency allocations** (per-frame temporaries,
  ECS component storage) are expected to use arena/bump allocators or
  pre-sized pools, not the global allocator, once we're past the v0.0.1
  placeholder.
- **Long-lived, infrequent allocations** (assets, plugin state) use the
  global allocator normally — optimizing this would be solving a problem
  that doesn't exist yet.
- **The global allocator itself** is left as Rust's default
  (`System`/platform default) for now; swapping in `mimalloc` or
  `jemalloc` is a one-line, measurable change we defer until there's a
  profiling reason to make it (see
  [`docs/roadmap/future-roadmap.md`](../roadmap/future-roadmap.md)).
- Rust's ownership model handles the memory-*safety* half of this problem
  for free in the host language; the memory-*performance* half (cache
  locality, allocation patterns) is what the ECS's data layout, below, is
  actually about.

## ECS architecture

### ECS design and target contracts

Canary's ECS is **archetype-based**: entities with the same set of component
types are stored contiguously (an "archetype table"), so iterating over
"every entity with `Position` and `Velocity`" is a linear scan over tightly
packed memory rather than a chase through scattered allocations. This is the
same family of design used by Bevy ECS, EnTT, and Unity DOTS (see
[`docs/research/engine-comparisons.md`](../research/engine-comparisons.md)
for the comparison), because the cache-locality argument for it is not
engine-specific — it's a property of modern CPUs.

Target-design commitments:

- **Entities** are generational indices (`index`, `generation`), so a stale
  handle to a despawned entity is detectable rather than silently aliasing a
  new one.
- **Components** are plain Rust structs with no inheritance/vtable
  requirement — data, not behavior.
- **Systems** make data access explicit so the scheduler can identify
  conflicts. Today, `SystemAccess` is manual metadata beside a closure that
  can access the full `World`; it is not checked against the body (R-24).
  Keep writers solo until typed system parameters or an equivalent mechanism
  enforces the declaration. Concurrent disjoint writes and automatic
  signature inference are not implemented.
- **Queries** are cached where possible so that iterating "all entities with
  X" doesn't re-derive the matching archetype set every call.
- **Change detection** (has this component been written since system Y last
  ran?) is a first-class query filter. Mutation ticks do not report removals
  or destruction; durable history for those is still required for networking
  (R-33).

### What's implemented as of `v0.0.7`

At `v0.0.7`, `canary-ecs`'s `World` implemented archetype storage, cached
queries, multi-component reads, a limited mutable/shared query, typed
resources, change detection, and stable component schema registration. The
scheduler was added separately in `v0.0.8`; current execution limitations are
described in
[`execution-model.md`](execution-model.md#known-limitations):

- **Archetype storage**: entities sharing a component-type signature live
  together in one archetype table; each component type is its own
  contiguous column, row-parallel to the entities it belongs to.
- **Cached queries**: `World::query`/`World::query_changed_since` resolve
  which archetypes to scan via a `TypeId -> Archetype` index maintained
  incrementally as archetypes are created, rather than checking every
  archetype's signature on every call. Since `v0.0.7`,
  `World::query2`/`World::query3`/`World::query2_mut` extend this to real
  multi-component joins (archetype-set intersection, not union) — see
  [`docs/architecture/execution-model.md`](execution-model.md) for the
  design these are a deliberately narrow first cut of.
- **Change detection** is a first-class query filter —
  `World::query_changed_since` — backed by a per-column, per-row tick
  that survives archetype moves caused by unrelated components (moving
  an entity because a *different* component was added or removed doesn't
  make its untouched components look freshly written). Since `v0.0.7`,
  `Tick` is a `u64` (was `u32`; see `execution-model.md` for the
  wraparound reasoning), and the same change-detection story extends to
  typed resource storage (`World::insert_resource`/`resource`/
  `resource_mut`/`resource_changed_since`) alongside components.
- **Component identity**: a first cut of
  [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)'s
  proposed direction — `CanaryComponent::SCHEMA_ID` plus
  `World::register_component`/`World::type_id_for_schema` — validating the
  string+version identity, `TypeId`-stays-host-internal approach. As of
  `v0.0.3`, this has a real consumer exercising it: `canary-plugin-api`'s
  Tier A loader resolves a WASM guest's `schema-id` through exactly this
  registry — see
  [`docs/architecture/plugin-system.md`](plugin-system.md) for that side
  of it, which lives in a different crate than this one.

This section covers `canary-ecs` specifically, so the implementation details
remain scoped to `v0.0.7`, while the engine as a whole is now at `v0.0.12`.
The scheduler added in `v0.0.8`,
[Threading & the job system](#threading--the-job-system) below, lives
in a new crate (`canary-scheduler`), not here, and doesn't change
anything about `canary-ecs`'s own API. Tier A's own loader, capability
enforcement, resource budget, and data ABI are real as of `v0.0.3`, but
live in `canary-plugin-api`, not here — see
[`docs/roadmap/v0.0.3-roadmap.md`](../roadmap/v0.0.3-roadmap.md) for
exact scope and what's explicitly excluded there.

The public API surface present since `v0.0.1-pre1` (`spawn`, `insert`,
single-component `query`, ...) is unchanged in behavior — the `v0.0.2`
archetype migration changed the *implementation* behind these calls, not
the call sites that use them. `v0.0.7`'s additions
(`query2`/`query3`/`query2_mut`, resource storage) are genuinely new
surface, not a reimplementation of existing calls — see
`execution-model.md` for why each is scoped the way it is. Since then,
two hardening refinements: `World::entity_count` is O(1) (a cached
alive count maintained by `spawn`/`despawn`, not a scan over every slot
ever created), and `World::remove` is documented idempotent — a
stale/unknown entity and an alive entity lacking `T` both yield `None`,
identically, since `remove` answers "give me the component if there is
one to take," not "prove this handle is live" (liveness checks belong
to `is_alive`, the way `insert`/`despawn` enforce them with
`EcsError::StaleOrUnknownEntity`).

## Threading & the job system

Target design: a work-stealing thread pool (one worker per physical core,
roughly), fed by the ECS scheduler's dependency graph. Longer-running,
coarse-grained work (asset cooking, physics broad-phase) submits jobs to the
same pool rather than spawning ad hoc OS threads, so the engine has one
place to reason about CPU utilization instead of N subsystems each guessing
how many threads they're "allowed."

`App` calls each subsystem's tick sequentially on the main thread. The
headless `canary-runtime` harness wraps its `World` and `Schedule` in a
private `EcsSubsystem`; the scheduler runs compatible read-only stages
concurrently inside that subsystem. See
[`docs/architecture/execution-model.md#the-scheduler`](execution-model.md#the-scheduler)
for its current limits (write systems run solo, threads are scoped per
stage). A reusable game composition API and a shared "one pool for
everything" design remain future work —
longer-running, coarse-grained work (asset cooking, physics
broad-phase) submitting to the *same* pool `Schedule` uses, once either
of those has real jobs worth submitting.

## Error handling conventions

- Library crates (`canary-ecs`, `canary-platform`, etc.) return typed errors
  (`Result<T, XError>` with `thiserror`-derived enums) — callers should be
  able to `match` on failure modes, not parse an error string.
- Application/binary code (`canary-runtime`, future editor, future game
  templates) may use a boxed/dynamic error type with added context at the
  boundary, since a top-level `main` typically wants to report and exit, not
  branch on error variants.
- Panics are reserved for genuine programmer errors (violated invariants,
  "this should be unreachable"), never for expected failure conditions like
  "file not found" or "plugin failed to load" — those are `Result`s.

See [`docs/development/coding-standards.md`](../development/coding-standards.md)
for the enforced version of these conventions.

## Known limitations

The ECS storage and identity work landed in `v0.0.2` and `v0.0.7`; that does
not close the execution and replication limitations around it. In particular,
manual/unverified scheduler access declarations (R-24) and missing durable
removal/destruction history (R-33) remain open. See
[`execution-model.md`](execution-model.md#known-limitations) for current
scheduler and runtime-composition gaps, and the resolution notes below
for what the August 2026 review and the `v0.0.2` archetype migration
closed out.


Resolved by the `v0.0.2` archetype migration: change detection is now
implemented (`World::query_changed_since`, ticked per column-row — see
above) rather than merely named as a requirement, and component identity
has a first prototyped cut (`World::register_component`) rather than
relying on `TypeId` alone — see
[ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md),
now `Accepted`.

Resolved since the August 2026 review, for `v0.0.1`: component storage
now requires `T: Send + Sync` (was previously unbounded, contradicting
the threading design above — see [`World::insert`](../../engine/canary-ecs/src/world.rs)
and its `world_is_send_and_sync` compile-time guard test), and
`Entity::generation` was widened from `u32` to `u64`, moving the
generation-wraparound risk on a long-lived, hot-recycled slot from
"plausible over years of real uptime" to "not reachable by any realistic
runtime."
