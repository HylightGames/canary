# 0017. A single, always-3D `Transform` representation for both 2D and 3D

**Status:** Accepted

## Context

`v0.0.9` introduces `Transform`/`GlobalTransform` — per
[`docs/roadmap/v0.1.0-plan.md`](../../roadmap/v0.1.0-plan.md), "the single
most load-bearing new primitive," since rendering, physics, audio, and UI
anchoring all depend on it. Canary treats 2D and 3D as both first-class
(see [`docs/vision/project-goals.md`](../../vision/project-goals.md#2d-and-3d-games-and-beyond)),
and [`physics.md`](../../architecture/physics.md) already ships 2D and 3D
physics as genuinely separate backend crates (`rapier2d`, `rapier3d`) for
that reason. This forced an explicit choice, before any code depends on
the answer: does the ECS `Transform` component mirror that 2D/3D split,
or serve both with one representation? A choice here is expensive to
reverse once rendering, physics sync, hierarchy propagation, and UI
anchoring all exist against it.

A second, coupled question: which math crate provides the underlying
vector/quaternion/matrix types — `glam` or `nalgebra` — given neither
currently appears anywhere in the workspace.

## Decision

`Transform` is a single, always-3D representation:
`translation: glam::Vec3`, `rotation: glam::Quat`, `scale: glam::Vec3`.
`GlobalTransform` caches the composed world-space matrix
(`glam::Mat4`), recomputed by a hierarchy-propagation system. 2D games
use the same type as 3D games, conventionally living in one plane
(typically `translation.z` fixed, rotation constrained to the `z` axis)
rather than through a distinct `Transform2D` type. `glam` is the
project's math crate for this and future graphics-facing types.

See [`docs/architecture/transform.md`](../../architecture/transform.md)
for the fuller design, including hierarchy and propagation-ordering
details not repeated here.

## Alternatives considered

- **Parallel `Transform2D`/`Transform3D` component types, mirroring the
  physics backend split.** Rejected: this looked superficially consistent
  with `physics.md`'s 2D/3D backend split, but the reason physics splits
  (two genuinely different Rapier crates with different internal math)
  doesn't apply to the ECS-facing transform — rendering, physics *sync*,
  hierarchy propagation, and UI anchoring would each need two code paths
  (or a trait unifying both) for a distinction none of them actually need
  to make. Bevy — the closest Rust-ecosystem comparison, also targeting
  2D and 3D as both first-class — made the same call for the same reason;
  Godot's `Transform2D`/`Transform3D` split is not a counterexample here,
  because Godot's split exists because its *scene tree* is 2D/3D-typed,
  a structural difference Canary's plain ECS parent/child hierarchy
  doesn't share.
- **A type-level 2D/3D guard** (e.g. a phantom-typed `Transform<Dim>`, or
  a `Transform2D` newtype restricting rotation to one axis). Rejected for
  now: this is exactly the kind of complexity the `v0.1.0` quality bar
  (["ugly APIs are fine; don't add complexity mature engines have if a
  simple solution is genuinely sufficient"](../../roadmap/v0.1.0-plan.md))
  argues against paying for before it's needed. A 2D game can mechanically
  set a stray `z` value with the plain representation; that's an accepted,
  cheap-to-improve-later ergonomic gap, not a `v0.1.0` correctness one.
- **`nalgebra` instead of `glam`.** Rejected as the public/ECS-facing
  math type: `nalgebra` is a general-purpose, generic-dimension numerical
  library aimed more at scientific/robotics computing; `glam` is
  SIMD-friendly and purpose-built for exactly this game/graphics shape of
  problem, and is the de facto standard in the Rust game ecosystem
  (`bevy_math`, `macroquad`, `kira`, `egui-gizmo`, and others all depend
  on it). This does not conflict with `physics.md`'s Rapier commitment:
  Rapier is internally `nalgebra`-based, but its `parry` geometry crate
  already ships `glam` interop, so the backend-internal conversion
  physics needs at its sync boundary (see
  [`physics.md`](../../architecture/physics.md#trait-based-abstraction-default-backend))
  is a solved problem regardless of which crate Canary's own public
  `Transform` uses. Per
  [`design-philosophy.md`](../../vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly),
  neither `nalgebra` nor `glam` types are meant to leak across a
  `PhysicsBackend` trait boundary either way — this decision is about
  Canary's own public type, not about which crate a backend uses
  internally.
- **A generic, dimension-parameterized `Transform<const N: usize>` (or
  similar), unifying 2D and 3D through generality rather than choosing
  3D-always.** Rejected: Canary's actual goal is "2D and 3D both
  first-class," not "arbitrary-dimension" — no game needs more than 3
  spatial dimensions, so generalizing beyond that trades real ergonomics
  for a capability nothing will use, the same "don't add complexity
  mature engines have if a simple solution is genuinely sufficient"
  reasoning as above.

## Consequences

- Every subsystem that reads or writes spatial state (rendering, physics
  sync, audio spatialization, UI world-space anchoring) uses one
  `Transform`/`GlobalTransform` pair — no 2D/3D transform split to keep
  behaviorally consistent across systems.
- `glam` becomes a new public dependency of `canary-transform`, and
  transitively of every subsystem that consumes `Transform` directly
  (rendering, and later physics/UI). It is a small, widely-audited,
  `no_std`-capable, pure-math crate — consistent with this project's
  "minimal unnecessary dependencies" bar, and does not itself pull in
  any windowing, GPU, or OS-specific dependency tree.
- 2D games get no type-level protection against setting a stray `z`
  translation or an off-axis rotation; this is an accepted, revisitable
  ergonomic gap (see [`transform.md`](../../architecture/transform.md)),
  not a foreclosed one — a `Transform2D` convenience wrapper over the
  same underlying data remains a fully compatible future addition.
- Every future `PhysicsBackend`, `AudioBackend`, or rendering-backend
  implementation performs its own internal math-crate conversion at the
  point it reads or writes `Transform`/`GlobalTransform`; this is not new
  architectural surface, just the existing "backend owns its own math,
  synchronizes into Canary's own types" pattern (`physics.md`) with a
  concrete crate now named on the Canary side of that seam.
