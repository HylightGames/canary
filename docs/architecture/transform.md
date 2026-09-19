# Transform, `GlobalTransform`, and Hierarchy

`Transform` is the single most load-bearing new primitive in the `v0.1.0`
plan (see [`docs/roadmap/v0.1.0-plan.md`](../roadmap/v0.1.0-plan.md)):
rendering, physics, audio, and UI anchoring all read or write it. Getting
its shape wrong is expensive precisely because so much depends on it early
— the same reason this document exists ahead of `canary-transform`'s code,
consistent with every other subsystem doc in this project. See
[ADR 0017](../decisions/architecture-decision-records/0017-unified-transform-representation.md)
for the decision record; this document is the fuller design.

## The problem: one representation, or two?

Canary treats 2D and 3D as both first-class (see
[`docs/vision/project-goals.md`](../vision/project-goals.md#2d-and-3d-games-and-beyond)),
and [`physics.md`](physics.md) ships them as genuinely separate backend
crates (`rapier2d`, `rapier3d`) for exactly that reason. It would be a
reasonable-sounding mistake to carry that same 2D/3D split up into the ECS
component itself — a `Transform2D` and a `Transform3D`, mirroring physics.
Canary does not do this: **`Transform` is a single, always-3D
representation, used by both 2D and 3D games.**

## How other engines approach this

- **Unity** has separate 2D and 3D physics/collider types, but its core
  `Transform` component has always been a single 3D
  position/rotation/scale — a 2D game simply doesn't touch the `z` axis
  (or `Sprite Renderer`'s `Order in Layer` substitutes for it) or
  constrains rotation to the `z` axis. This is the precedent Canary's
  choice most directly follows.
- **Bevy** (the closest Rust-ecosystem comparison — see
  [`docs/research/engine-comparisons.md`](../research/engine-comparisons.md))
  made the same choice explicitly: one `Transform` (`glam::Vec3` /
  `glam::Quat` / `glam::Vec3`) for both `bevy_sprite` (2D) and 3D
  rendering, rather than parallel component types.
- **Godot** is the counterexample: `Node2D` and `Node3D` are genuinely
  separate scene-tree branches with separate `Transform2D`/`Transform3D`
  types, because Godot's scene tree itself is the thing being specialized
  for 2D vs. 3D, not just the transform data. Canary doesn't have this
  same structural reason — Canary's hierarchy (below) is a plain ECS
  parent/child relationship, not a 2D- or 3D-flavored tree — so Godot's
  reason for splitting doesn't transfer.

## Why one representation, for Canary specifically

- **Every downstream consumer is simpler with one type.** Rendering,
  physics synchronization, UI world-space anchoring, and hierarchy
  propagation would each need two code paths (or a trait over both) with
  a split representation, for a 2D/3D distinction none of them actually
  need to make — a renderer drawing a 2D sprite and a renderer drawing a
  3D mesh both just need "where is this, in world space," which a 3D
  transform answers for both.
- **2D is already "a specialization, not a separate architecture"
  elsewhere in Canary.** [`rendering.md`](rendering.md) makes exactly this
  choice for the render graph (an orthographic camera and flat quads
  through the same RHI, not a parallel 2D renderer); a single `Transform`
  is the same design instinct applied one layer down.
- **Physics splitting into `rapier2d`/`rapier3d` doesn't force `Transform`
  to split too.** The physics backend owns its own internal simulation
  state and math types (`nalgebra`, for both Rapier crates) and
  synchronizes into Canary's `Transform`/`GlobalTransform` once per step —
  already the documented pattern in
  [`physics.md`](physics.md#trait-based-abstraction-default-backend). A
  2D physics step writes a `Transform` with `translation.z` and
  `rotation`'s non-`z` axes left at whatever the game already had there
  (typically zero) — it doesn't need `Transform` itself to be
  2D-shaped to do this.

## The representation

```rust
// Illustrative — see engine/canary-transform/src/lib.rs for the real
// implementation.
pub struct Transform {
    pub translation: glam::Vec3,
    pub rotation: glam::Quat,
    pub scale: glam::Vec3,
}

pub struct GlobalTransform(glam::Mat4);
```

- **`glam`, not `nalgebra`, for this type.** `glam` is the Rust
  game/graphics ecosystem's de facto standard math crate (`bevy_math`,
  `macroquad`, `kira`, `egui-gizmo`, and others all depend on it) —
  SIMD-friendly and purpose-built for exactly this shape of problem,
  versus `nalgebra`'s general-purpose, generic-dimension design aimed
  more at scientific/robotics computing. This is not a barrier to the
  Rapier integration `physics.md` already commits to: Rapier's own
  `parry` geometry crate already ships `glam` interop, so the
  backend-internal conversion physics already needs to do (see above) is
  a solved problem either way. See
  [ADR 0017](../decisions/architecture-decision-records/0017-unified-transform-representation.md)
  for the full alternatives analysis.
- **`GlobalTransform` caches the composed world-space matrix** (not
  position/rotation/scale again), because its job is to answer "where is
  this in world space, ready to feed a render/physics query" without every
  reader re-walking the parent chain — the same reason Bevy's equivalent
  type is a cached matrix rather than a second position/rotation/scale
  triple.
- **No `Transform2D` newtype, no phantom-typed dimensionality marker.**
  A 2D game can, mechanically, set a non-zero `translation.z` or a
  non-`z`-axis rotation by mistake; Canary accepts this rather than
  adding a type-level guard against it. This is a deliberate instance of
  the `v0.1.0` mandate's "ugly APIs are fine, don't add complexity mature
  engines have if a simple solution is genuinely sufficient" quality bar
  (see [`docs/roadmap/v0.1.0-plan.md`](../roadmap/v0.1.0-plan.md)) — a
  2D-specific ergonomic wrapper is a `v0.2.0`+ "make it pleasant"
  concern, not a `v0.1.0` "does it work" one.

## Hierarchy and propagation

- **Parent/child is a plain ECS relationship** — a `Parent(Entity)`
  component on the child, a `Children(Vec<Entity>)` component on the
  parent (kept in sync with each other), not a separate tree structure
  living outside the ECS. This keeps hierarchy queryable and networkable
  through the same mechanisms as everything else, consistent with
  [`design-philosophy.md`](../vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly)'s
  "communicate through shared, observable state" discipline.
- **`GlobalTransform` propagation is a system registered through
  `canary-scheduler`**, not a special-cased engine-internal step — it
  declares its data access (reads `Transform` and `Parent` across the
  whole hierarchy, writes `GlobalTransform`) the same way any gameplay
  system would, which is precisely why this is also the scheduler's
  first proof against a real gameplay-shaped system rather than only its
  own test doubles (per
  [`docs/roadmap/v0.1.0-plan.md`](../roadmap/v0.1.0-plan.md)).
- **Propagation order**: roots (entities with no `Parent`) compute their
  `GlobalTransform` directly from their local `Transform`; children
  compose their local `Transform` onto their parent's already-computed
  `GlobalTransform`, requiring a parent-before-child visitation order.
  The first implementation does this by depth ordering the hierarchy walk
  each run; a future optimization (change-detection-gated, only
  re-propagating subtrees whose `Transform` or ancestry actually changed
  since the last tick) is a `v0.2.0`+ performance concern, not a
  correctness requirement now — matches the same "correct now, fast
  later, don't block on a perf question nothing has measured yet" posture
  [`core-runtime.md`](core-runtime.md#threading--the-job-system) already
  takes with the job system.

## Status in this foundation

Implemented on `dev` (not yet tagged): `engine/canary-transform` holds
`Transform`/`GlobalTransform`, the `Parent`/`Children` hierarchy
components with sync-keeping helpers, and a hierarchy-propagation system
registered through `canary-scheduler` — the scheduler's first proof
against a real gameplay-shaped system rather than only its own test
doubles. ECS-driven rendering off `GlobalTransform` has since landed
too, via the `canary-render-ecs` bridge (see `rendering.md`); still
open, per [`docs/roadmap/v0.1.0-plan.md`](../roadmap/v0.1.0-plan.md):
the RHI upgrades the bridge still defers (push constants/uniforms,
depth/culling, buffer and texture updates, materials past `v0.0.10`'s
single-texture slice, swapchain/presentation) and the camera
component.
