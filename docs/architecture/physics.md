# Physics

Like rendering, no physics code exists in v0.0.1 — this document is
architecture for a later era, reasoned about now.

## Trait-based abstraction, default backend

Physics follows the same "replaceable subsystem" pattern as rendering: a
`PhysicsBackend` trait covering rigid bodies, colliders, constraints/joints,
and scene queries (raycasts, sweeps, overlap tests). Engine and gameplay
code call the trait, never the concrete backend crate — this is the same
"bind through an interface, never call a third party directly" discipline
recorded as a project-wide principle in
[`docs/vision/design-philosophy.md`](../vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other-or-a-third-party-directly),
applied here concretely:

```
Your Engine Physics API
          |
   Physics Abstraction Layer (PhysicsBackend trait)
          |
  -----------------------
  |                     |
Rapier Backend       Jolt Backend
(2D + 3D, default)   (3D only, built in, opt-in)
```

Concretely:

- **Default backend: [Rapier](https://rapier.rs/)** — a pure-Rust physics
  engine, actively maintained, with an explicit 2026 roadmap toward
  GPU-accelerated rigid-body simulation. Choosing a pure-Rust default avoids
  an FFI boundary for the common case and keeps the "batteries included"
  path dependency-simple. Rapier ships **2D and 3D as genuinely separate
  crates** (`rapier2d`, `rapier3d`, plus `f64`-precision variants of each),
  sharing a similar API rather than one 3D system 2D games have to route
  around — a real, concrete instance of
  [`docs/vision/project-goals.md`](../vision/project-goals.md#2d-and-3d-games-and-beyond)'s
  "2D is first-class" commitment, not just an assertion. Worth noting for
  that same document's broader point: Rapier's own official description
  positions it for "games, **animation, and robotics**" — independent,
  external confirmation that a physics engine Canary already depends on
  is itself built for exactly the kind of non-game-exclusive use this
  project's architecture aims not to foreclose.
- **Built in, opt-in: [Jolt Physics](https://github.com/jrouwe/JoltPhysics)** —
  a C++ engine built specifically for multithreaded, production game use
  (it ships in Horizon Forbidden West and Death Stranding 2, and Godot added
  it as a selectable backend in 4.4). Unlike a purely "documented, someone
  could write this" alternative, Jolt is intended to ship as a first-party-
  maintained backend alongside Rapier — available without a user having to
  go integrate it themselves — while Rapier stays the default. It's also
  the reference example for "what a trusted, native (Tier B) subsystem
  replacement looks like in practice" — see
  [plugin-system.md](plugin-system.md#tier-b--trusted-native-c-abi). Jolt
  is 3D-only; a project needing 2D physics stays on Rapier.
- **Documented alternative: [Avian](https://github.com/Jondolf/avian)** — a
  younger, ECS-native Rust physics engine built specifically to avoid
  maintaining a separate physics "world" outside the host ECS, also
  shipped as separate `avian2d`/`avian3d` crates. Worth
  revisiting as it matures; not the default today because Rapier is more
  battle-tested (see
  [`docs/research/technology-evaluations.md`](../research/technology-evaluations.md)
  for the sourcing behind this comparison).

The trait boundary is what makes "swap the physics engine" a crate-level
decision rather than an engine fork — the same principle applied in
[rendering.md](rendering.md) and recorded generally in
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md).

## Fixed timestep, deterministic where it matters

Physics steps on a fixed timestep, decoupled from (and typically at a
different rate than) the variable render framerate — standard practice, but
worth stating because it's foundational for two other Canary goals:

- **Multiplayer** ([networking.md](networking.md)) generally requires
  simulation to be reproducible enough for client prediction/reconciliation
  to converge; a fixed timestep is a precondition for that, not a guarantee
  of it (true cross-platform floating-point determinism is a harder,
  separate problem, and Rapier explicitly supports an optional deterministic
  mode for this reason).
- **Replayability/debugging**: a fixed timestep makes "record inputs, replay
  simulation" tooling tractable for bug reports and automated testing.

## Integration with the ECS

Rigid bodies and colliders are represented as ECS components
(`RigidBody`, `Collider`, ...); the physics backend owns its internal
simulation state but synchronizes transforms into ECS component storage
once per physics step, following the same "backend owns its world, ECS gets
a synchronized view" pattern used by existing Rust integrations like
`bevy_rapier` — a pattern Canary's own ECS-native alternative path (mirroring
Avian's approach) may improve on later, but not before the archetype ECS
described in [core-runtime.md](core-runtime.md) exists to build it against.

## Status in this foundation

Entirely architectural. No `canary-physics` crate exists yet; the
`PhysicsBackend` trait itself isn't implemented in v0.0.1's code. This
is intentional scope discipline — see
[`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md).
