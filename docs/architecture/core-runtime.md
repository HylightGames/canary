# Core Runtime

Covers `canary-core` and `canary-ecs`: the parts of the engine that exist
before there's a window, a renderer, or a network connection.

## The `App`/`Engine` bootstrap

`canary-core` defines the entry point every Canary program (game, headless
server, editor, or test harness) shares:

```rust
// Illustrative target shape — see engine/canary-core/src/lib.rs for the
// current, deliberately minimal v0.0.1-pre1 implementation.
let mut app = canary_core::App::new();
app.add_subsystem(canary_ecs::EcsSubsystem::default());
app.add_plugin_dir("plugins/");
app.run();
```

`App` owns the top-level lifecycle (init → main loop → shutdown) and a
registry of **subsystems** — the Layer 3 pieces from
[engine-overview.md](engine-overview.md). A subsystem is deliberately a small
trait (`Subsystem::init`, `Subsystem::shutdown`, plus scheduling hooks), so
adding "physics" or "networking" to a program is opt-in and explicit, never
implied by which crates happen to be linked.

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

### Target design (Era 2+)

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
- **Systems** declare their data access (which components/resources they
  read vs. write) in their function signature; the scheduler uses that
  declaration to run non-conflicting systems in parallel automatically. No
  system manually spawns a thread.
- **Queries** are cached where possible so that iterating "all entities with
  X" doesn't re-derive the matching archetype set every call.
- **Change detection** (has this component been written since system Y last
  ran?) is a first-class query filter, because it's foundational for
  networking replication (only send what changed) and for editor tooling
  (only re-cook what changed).

### What v0.0.1-pre1 actually ships

The `canary-ecs` crate in this foundation is a deliberately minimal
**placeholder**, not the archetype design above:

- A `World` holding entities as generational indices.
- Components stored per-type in a simple map (not archetype tables).
- Synchronous, single-threaded system execution — no scheduler, no
  automatic parallelism, no change detection yet.

This is documented as a placeholder rather than quietly presented as "the
ECS is done" — see [`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md)
for why this scope was chosen for the very first buildable milestone, and
what specifically needs to change to reach the target design above. The
public API surface (`spawn`, `insert`, `query`) is intentionally shaped so
that migrating to archetype storage later changes the *implementation*
behind these calls, not the call sites that use them — but this is an
intent, not a guarantee; some call-site churn during that migration should
be expected and is fine.

## Threading & the job system

Target design: a work-stealing thread pool (one worker per physical core,
roughly), fed by the ECS scheduler's dependency graph. Longer-running,
coarse-grained work (asset cooking, physics broad-phase) submits jobs to the
same pool rather than spawning ad hoc OS threads, so the engine has one
place to reason about CPU utilization instead of N subsystems each guessing
how many threads they're "allowed."

v0.0.1-pre1 has no job system at all — `canary-runtime` runs everything on
the main thread. This is intentionally deferred rather than half-built: a
job system designed before the ECS's real data-access declarations exist
would likely need redesigning anyway once those declarations land.

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
