# 0019. Physics backend lineup: Rapier2D canonical 2D, Jolt canonical 3D, Rapier3D alternative

**Status:** Accepted

## Context

`docs/architecture/physics.md` already decides the trait-shaped part:
a `PhysicsBackend` trait (rigid bodies, colliders, joints, scene
queries) that engine and gameplay code program against, with concrete
backends behind it and no third-party types leaking into game code.
What it left as emphasis, not decision, is the *lineup*: it names
Rapier the default (2D and 3D), Jolt a built-in opt-in (3D only), and
Avian a documented alternative. Meanwhile `v0.0.11` was scoped as
"2D first (`rapier2d`)" with 3D physics explicitly deferred past
`v0.1.0`.

An external 2026 review of the physics ecosystem (prompted
September 2026, evaluated against Canary specifically rather than
adopted verbatim) forced the emphasis question before `v0.0.11`
locks anything in: with 3D physics deferred, not yet built, the
choice of *canonical* 3D backend is still cheap to make and will be
expensive to revisit once a second backend exists. The evidence it
brought: Rapier at 0.35.x (mature, SIMD, parallelism, deterministic
mode); Jolt's multithreading/vehicles/ragdolls/character systems and
large-world support as production-proven 3D strengths; Box2D 3.1's
genuine multithreaded/deterministic improvements; Avian's
ECS-native design but shorter track record; the main PhysX Rust
binding archived (May 2026); and the Jolt caveat that matters most —
`jolt-rust` is early-stage against a fast-moving upstream, so Jolt's
*Rust integration maturity* lags Rapier's substantially.

## Decision

Two specialized backends behind the unchanged trait, selected by
configuration rather than hardcoded at the architecture level:

```text
PhysicsBackend
├── Rapier2D      (canonical 2D)
├── Jolt3D        (canonical 3D)
└── Rapier3D      (swappable alternative 3D)
```

```toml
# Illustrative — exact config shape when backends land.
[physics]
dimension = "3d"
backend = "jolt"  # or "rapier"
```

- **2D → Rapier2D, canonical.** Pure Rust, mature, SIMD, parallelism,
  deterministic mode, ECS-friendly. Box2D 3.1 is respectably
  improved but C-FFI against a Rust-first project; Avian is the
  more interesting long-term alternative but less battle-tested.
  Neither displaces Rapier2D today.
- **3D → Jolt, canonical — with the caveat priced in.** Jolt is the
  stronger technical 3D choice for an engine with serious 3D
  ambitions (multithreaded island solving, vehicles, ragdolls,
  character controllers, large-world support), and Rapier3D —
  genuinely capable — stays a fully supported swappable alternative
  for portability, determinism work, and testing. "Canonical" means
  *selected by default in configuration*, never a permanent
  dependency of the physics model: the trait boundary is what makes
  replacing Jolt later an implementation swap, not an engine
  rewrite.
- **Game code sees Canary concepts only** (`RigidBody`, `Collider`,
  `Transform`, `Velocity`, `Joint`, `CharacterController`,
  `Raycast`, …). Backends translate into whatever the solver
  requires. This restates the existing trait discipline with a
  concrete concept list; it changes no boundary.
- **v0.0.11 scope is unchanged: rapier2d-only 2D.** This ADR locks
  *direction*, not schedule. Jolt and Rapier3D backends land when 3D
  physics lands (post-`v0.1.0`, per the `v0.1.0` plan) — which
  doubles as schedule-based risk management for the Jolt caveat:
  the bindings get time to mature, and if they don't, Rapier3D is
  already accepted as the fallback, not a redesign.
- **Box2D gets a benchmark, not a decision.** Before performance
  targets lock, run Box2D 3.1 against Rapier2D on a representative
  2D workload. If Box2D wins decisively *and* the FFI cost stays
  contained, revisit; otherwise the record shows the comparison
  happened.

See [`docs/architecture/physics.md`](../../architecture/physics.md)
for the design (backend section revised to match this decision) and
[`docs/roadmap/v0.1.0-plan.md`](../../roadmap/v0.1.0-plan.md) for
sequencing.

## Alternatives considered

- **Rapier everywhere (2D + 3D).** Rejected as the canonical lineup
  — not on capability (Rapier3D is a proper 3D engine) but on
  ambition fit: selecting the 3D backend for convenience now risks
  bending the 3D architecture around it later. Rapier3D remains a
  first-class alternative, which keeps this decision reversible at
  low cost — the strongest reason to make it now rather than later.
- **Jolt hardcoded as *the* 3D backend at the architecture level.**
  Rejected: "canonical default in configuration" captures the
  intent without making Jolt irreplaceable. If the bindings stall
  or a better solver appears, the swap stays crate-level.
- **Box2D as canonical 2D.** Rejected for now: a very good C engine
  wrapped via FFI versus a very good Rust engine used directly, on
  a Rust-first project. The benchmark task above is the honest
  version of this alternative — evidence before reversal.
- **Avian as canonical anything.** Rejected for now: the ECS-native
  design is attractive, but track record decides defaults and
  Rapier's is longer. Stays a documented alternative to revisit as
  it matures (unchanged from `physics.md`).
- **PhysX / Bullet.** Rejected: archived Rust bindings (PhysX) and
  a less attractive fit than Jolt for this architecture (Bullet).
  Noted here so nobody re-researches them from scratch.

## Consequences

- `physics.md`'s backend section is revised to this lineup (canonical
  2D / canonical 3D / alternative, config-selected); the trait,
  fixed-timestep, and ECS-sync sections are untouched.
- `v0.0.11` builds exactly one backend (Rapier2D) against the trait;
  the trait must already accommodate the *idea* of configured
  backends (dimension + backend selection) even though only one
  exists — a second hardcoding would recreate the problem this ADR
  solves.
- Jolt integration is post-`v0.1.0` work with a real readiness gate:
  bindings mature enough to bind, or Rapier3D goes first by default.
  Either way the game-facing API does not move.
- A Box2D-vs-Rapier2D benchmark is owed before performance targets
  lock; its result is recorded even if it changes nothing.
