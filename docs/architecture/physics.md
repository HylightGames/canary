# Physics

Like rendering, no physics code exists in v0.0.1 — this document is
architecture for a later era, reasoned about now.

## Trait-based abstraction, default backend

Physics follows the same "replaceable subsystem" pattern as rendering: a
`PhysicsBackend` trait covering rigid bodies, colliders, constraints/joints,
and scene queries (raycasts, sweeps, overlap tests), with a default
implementation and room for alternatives. Concretely:

- **Default backend: [Rapier](https://rapier.rs/)** — a pure-Rust 2D/3D
  physics engine, actively maintained, with an explicit 2026 roadmap toward
  GPU-accelerated rigid-body simulation. Choosing a pure-Rust default avoids
  an FFI boundary for the common case and keeps the "batteries included"
  path dependency-simple.
- **Documented alternative: [Jolt Physics](https://github.com/jrouwe/JoltPhysics)** —
  a C++ engine built specifically for multithreaded, production game use
  (it ships in Horizon Forbidden West and Death Stranding 2, and Godot added
  it as a selectable backend in 4.4). Jolt is the reference example for
  "what a trusted, native (Tier B) subsystem replacement looks like in
  practice" — see [plugin-system.md](plugin-system.md#tier-b--trusted-native-c-abi).
- **Documented alternative: [Avian](https://github.com/Jondolf/avian)** — a
  younger, ECS-native Rust physics engine built specifically to avoid
  maintaining a separate physics "world" outside the host ECS. Worth
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
