// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Minimal 2D physics components and the [`PhysicsConfig`] resource.
//!
//! This is the Task 3 data half of `canary-physics`: plain data the game
//! writes and the physics system reads. Nothing here knows about any
//! backend crate (`rapier2d`, a future Jolt/Rapier3D, or a user-authored
//! backend): all fields are primitives, `glam` math, or types defined in
//! this module, so a second backend fits later with zero API movement.
//!
//! Joints, scene queries, and trimesh/heightfield/polyline colliders are
//! deliberately OUT of this cut (see the v0.0.11 work plan) — each is a
//! named deferred item with a future owner, not an oversight. What lands
//! here is the smallest set a falling-box-on-ground scene plus one
//! scripted kinematic platform needs: bodies, three solid collider
//! shapes, velocity, per-body gravity scaling, axis locks, one material,
//! and the config resource that names the dimension/backend selection
//! (ADR 0019) even though only one backend exists yet.

use crate::PhysicsError;

/// How a body participates in simulation.
///
/// Only one kinematic flavor exists, deliberately: [`RigidBodyKind::KinematicPosition`].
/// A position-kinematic body follows a scripted pose the game sets each
/// tick (moving platforms, doors) and pushes dynamic bodies out of the
/// way without itself responding to forces. The velocity-kinematic flavor
/// (`KinematicVelocityBased` in Rapier's terms) is OUT of this cut: it
/// answers a different authoring question ("drive this body by velocity
/// and let the solver integrate it") that no v0.0.11 scene asks, and
/// carrying both flavors would force every backend adapter and every
/// sync test to cover a mode with no consumer. When a real consumer
/// arrives it is one additive variant, not a redesign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RigidBodyKind {
    /// Simulated body: gravity, forces, impulses, and contacts move it.
    Dynamic,
    /// Never moves no matter what hits it (ground, walls). Infinite mass
    /// in solver terms; the cheapest body to simulate.
    Fixed,
    /// Scripted body: the game sets its pose, the solver moves others
    /// around it. See the enum-level docs for why this is the only
    /// kinematic flavor in this cut.
    KinematicPosition,
}

/// The physics body marker on an entity: which solver role the entity
/// plays. Pose itself lives on [`canary_transform::Transform`] (the sync
/// target the backend writes each fixed step in Task 4), never here, so
/// there is exactly one source of truth for where things are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RigidBody {
    /// The solver role of this body. See [`RigidBodyKind`].
    pub kind: RigidBodyKind,
}

impl RigidBody {
    /// A [`RigidBodyKind::Dynamic`] body: the common case for falling
    /// boxes, projectiles, and anything else forces should move.
    pub fn dynamic() -> Self {
        Self {
            kind: RigidBodyKind::Dynamic,
        }
    }

    /// A [`RigidBodyKind::Fixed`] body: ground, walls, anything that must
    /// never move regardless of impact.
    pub fn fixed() -> Self {
        Self {
            kind: RigidBodyKind::Fixed,
        }
    }

    /// A [`RigidBodyKind::KinematicPosition`] body: scripted platforms and
    /// doors whose pose the game drives directly.
    pub fn kinematic_position() -> Self {
        Self {
            kind: RigidBodyKind::KinematicPosition,
        }
    }
}

/// A solid 2D collider shape attached to a [`RigidBody`] entity.
///
/// Three shapes, deliberately: ball (rolls, cheapest contact), cuboid
/// (boxes, ground slabs, walls), capsule (characters — a capsule slides
/// over steps and ground seams that snag a cuboid's corners). Everything
/// else Rapier offers (trimesh, heightfield, polyline, cone, cylinder)
/// is OUT of this cut: each needs either asset-pipeline input (meshes)
/// or 3D semantics, and none has a v0.0.11 consumer. All three shapes
/// here are fully described by one or two positive scalars, which keeps
/// [`Collider::validate`] total and backend translation trivial.
///
/// All extents are half-measures in world units, matching Rapier's own
/// constructors (`ColliderDesc::cuboid(hx, hy)`,
/// `ColliderDesc::capsule(half_height, radius)`), so the Task 4 adapter
/// passes them through without conversion math that could drift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Collider {
    /// A disc: `radius` must be finite and positive.
    Ball {
        /// Disc radius in world units. Finite and positive; see
        /// [`Collider::validate`].
        radius: f32,
    },
    /// An axis-aligned box: both half-extents must be finite and
    /// positive. Full size is `2 * half_extents` per axis.
    Cuboid {
        /// Half-width (`[0]`) and half-height (`[1]`) in world units.
        half_extents: [f32; 2],
    },
    /// A capsule (a line segment swept by a disc): the cylindrical
    /// section's half-height plus cap radius must each be finite and
    /// positive. Total height is `2 * (half_height + radius)`.
    Capsule {
        /// Half of the inner segment length (the cylindrical part, not
        /// counting the caps), in world units.
        half_height: f32,
        /// Cap/disc radius in world units.
        radius: f32,
    },
}

impl Collider {
    /// A disc collider of the given radius. No validation here by
    /// design: construction is infallible and cheap, while fallibility
    /// lives in exactly one place — [`Collider::validate`] — so systems
    /// and backends share one check instead of each inventing one.
    pub fn ball(radius: f32) -> Self {
        Self::Ball { radius }
    }

    /// An axis-aligned box collider of the given half-extents. See
    /// [`Collider::ball`] for why this constructor does not validate.
    pub fn cuboid(half_extents: [f32; 2]) -> Self {
        Self::Cuboid { half_extents }
    }

    /// A capsule collider. See [`Collider::ball`] for why this
    /// constructor does not validate.
    pub fn capsule(half_height: f32, radius: f32) -> Self {
        Self::Capsule {
            half_height,
            radius,
        }
    }

    /// Checks this collider describes a shape a solver can actually
    /// simulate, returning a typed [`PhysicsError`] naming the offending
    /// dimension otherwise.
    ///
    /// Every scalar must be finite (`NaN`/`infinite` extents would poison
    /// solver state with no clear blame) and strictly positive (zero or
    /// negative extents describe a degenerate shape with zero or
    /// negative volume — a caller bug, since no real object has one).
    /// Backends call this on attach and systems can call it at spawn;
    /// either way the error names the bad value, never a bare bool.
    pub fn validate(&self) -> Result<(), PhysicsError> {
        match *self {
            Self::Ball { radius } => {
                if !radius.is_finite() || radius <= 0.0 {
                    return Err(PhysicsError::InvalidRadius { radius });
                }
                Ok(())
            }
            Self::Cuboid { half_extents } => {
                if half_extents.iter().any(|e| !e.is_finite() || *e <= 0.0) {
                    return Err(PhysicsError::InvalidHalfExtents { half_extents });
                }
                Ok(())
            }
            Self::Capsule {
                half_height,
                radius,
            } => {
                if !half_height.is_finite() || half_height <= 0.0 {
                    return Err(PhysicsError::InvalidHalfHeight { half_height });
                }
                if !radius.is_finite() || radius <= 0.0 {
                    return Err(PhysicsError::InvalidRadius { radius });
                }
                Ok(())
            }
        }
    }
}

/// Linear + angular velocity of a body, in world units per second.
///
/// The sync direction matters: the backend OWNS integration (it steps
/// velocities into poses internally) and writes poses back to
/// [`canary_transform::Transform`]; this component is how the game
/// *seeds or overrides* motion (set an initial velocity, script a
/// platform's speed) and how `set_velocity` on the trait takes its
/// input — never a per-frame integration variable the game advances
/// itself, which would fight the fixed-step solver.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Velocity {
    /// Linear velocity `[x, y]` in world units per second.
    pub linvel: [f32; 2],
    /// Angular velocity around the z-axis in radians per second
    /// (2D rotation has one degree of freedom).
    pub angvel: f32,
}

impl Velocity {
    /// Zero linear and angular velocity.
    pub fn zero() -> Self {
        Self {
            linvel: [0.0, 0.0],
            angvel: 0.0,
        }
    }
}

impl Default for Velocity {
    /// Same as [`Velocity::zero`].
    fn default() -> Self {
        Self::zero()
    }
}

/// Per-body gravity multiplier.
///
/// `1.0` (the default) takes the full [`PhysicsConfig`] gravity, `0.0`
/// opts out of gravity while still colliding (floating pickups,
/// top-down movement), and negative values reverse it. Any finite value
/// is meaningful, so unlike [`Collider`] there is no validation error
/// here — only `NaN`/infinity would be nonsense, and those arrive via
/// caller bugs the backend's float hygiene (Task 4 quarantine) catches
/// at the boundary that matters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GravityScale(pub f32);

impl GravityScale {
    /// Full gravity. See the type-level docs for the other regimes.
    pub fn full() -> Self {
        Self(1.0)
    }

    /// No gravity, still collides.
    pub fn none() -> Self {
        Self(0.0)
    }
}

impl Default for GravityScale {
    /// Same as [`GravityScale::full`].
    fn default() -> Self {
        Self::full()
    }
}

/// Which degrees of freedom the solver may not change.
///
/// Locks are solver-side constraints (a top-down player that must never
/// rotate, a lift constrained to the y-axis), not game-side clamps
/// applied after the fact: post-hoc clamping fights the solver and
/// injects energy, while a lock removes the degree of freedom from
/// integration entirely. Plain bools, not a bitflags dependency — three
/// independent axes need no bit algebra, and a struct keeps each axis
/// self-documenting at every use site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct LockedAxes {
    /// The solver may not move the body along world x.
    pub lock_translation_x: bool,
    /// The solver may not move the body along world y.
    pub lock_translation_y: bool,
    /// The solver may not rotate the body (the single z-axis rotation).
    pub lock_rotation: bool,
}

impl LockedAxes {
    /// No locks: the solver owns all three degrees of freedom.
    pub fn unlocked() -> Self {
        Self::default()
    }

    /// Rotation locked, translation free: the top-down-player shape that
    /// motivated this component (collisions must never spin the sprite).
    pub fn rotation_locked() -> Self {
        Self {
            lock_translation_x: false,
            lock_translation_y: false,
            lock_rotation: true,
        }
    }
}

/// Surface response of a collider: friction plus bounciness.
///
/// One material per collider (passed alongside it at attach time), not a
/// global setting, because ground slabs, ice patches, and rubber balls
/// coexist in one scene. Defaults mirror Rapier's own
/// (`friction = 0.5`, `restitution = 0.0`) so the adapter's "no material
/// specified" path and an explicit default agree exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColliderMaterial {
    /// Coulomb friction coefficient, `0.0` (ice) upward. Must be finite
    /// and non-negative; see [`ColliderMaterial::validate`].
    pub friction: f32,
    /// Bounciness on contact. Must be finite and within `0.0..=1.0`
    /// (`0.0` dead, `1.0` perfectly elastic); values above `1.0` would
    /// inject energy every bounce, which is never what a material
    /// means — use an impulse for trampolines.
    pub restitution: f32,
}

impl ColliderMaterial {
    /// Builds a material without validation; fallibility lives in
    /// [`ColliderMaterial::validate`], mirroring [`Collider`]'s
    /// construct-infallibly/check-explicitly split.
    pub fn new(friction: f32, restitution: f32) -> Self {
        Self {
            friction,
            restitution,
        }
    }

    /// Rejects negative or non-finite friction and out-of-range or
    /// non-finite restitution with a typed [`PhysicsError`] naming the
    /// offending value.
    pub fn validate(&self) -> Result<(), PhysicsError> {
        if !self.friction.is_finite() || self.friction < 0.0 {
            return Err(PhysicsError::InvalidFriction {
                friction: self.friction,
            });
        }
        if !self.restitution.is_finite() || self.restitution < 0.0 || self.restitution > 1.0 {
            return Err(PhysicsError::InvalidRestitution {
                restitution: self.restitution,
            });
        }
        Ok(())
    }
}

impl Default for ColliderMaterial {
    /// Rapier's own defaults (`friction = 0.5`, `restitution = 0.0`);
    /// see the type-level docs for why they match.
    fn default() -> Self {
        Self {
            friction: 0.5,
            restitution: 0.0,
        }
    }
}

/// Which simulation dimension the physics world runs in.
///
/// Only [`PhysicsDimension::TwoD`] exists today (v0.0.11 is rapier2d-only
/// 2D scope), but the selection already lives in [`PhysicsConfig`] as a
/// typed enum rather than a comment, per ADR 0019's consequence: "the
/// trait must already accommodate the *idea* of configured backends
/// (dimension + backend selection) even though only one exists". A 3D
/// variant later is additive — no game-facing signature moves.
/// [`non_exhaustive`](https://doc.rust-lang.org/reference/attributes/type_system.html#the-non_exhaustive-attribute)
/// forces downstream `match`es through a wildcard arm today, so adding
/// that variant cannot break exhaustive matches outside this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PhysicsDimension {
    /// 2D simulation (the only dimension in this cut).
    TwoD,
}

impl PhysicsDimension {
    /// The config-file spelling from ADR 0019 (`dimension = "2d"`):
    /// kept next to the type so serialization and docs cannot drift
    /// apart when the 3D variant arrives.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TwoD => "2d",
        }
    }
}

/// Which backend implementation steps the simulation.
///
/// Only [`PhysicsBackendName::Rapier`] exists today, for the same
/// ADR 0019 reason as [`PhysicsDimension`]: Jolt (canonical 3D) and
/// Rapier3D (alternative 3D) arrive post-v0.1.0 as new variants plus new
/// backend crates behind the unchanged [`crate::PhysicsBackend`] trait.
/// `#[non_exhaustive]` for the same future-proofing rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PhysicsBackendName {
    /// Rapier2D, the canonical 2D backend (ADR 0019).
    Rapier,
}

impl PhysicsBackendName {
    /// The config-file spelling from ADR 0019 (`backend = "rapier"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rapier => "rapier",
        }
    }
}

/// Global physics configuration, stored as a `World` resource.
///
/// A plain `Send + Sync + 'static` struct — the only three things
/// `canary-ecs` resources require (`World::insert_resource`) — holding
/// gravity plus the ADR 0019 dimension/backend selection. Systems read
/// it via `World::resource::<PhysicsConfig>()`; there is at most one
/// per `World`, matching the "one physics world per game" reality.
/// Gravity lives here (not per-scene, not per-backend-constructor) so
/// tuning it is data — set once at startup, hot-reloadable later —
/// never a code change threading through backend constructors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsConfig {
    /// Gravity acceleration applied to dynamic bodies (scaled per-body
    /// by [`GravityScale`]), in world units per second squared. Default
    /// points down in a y-up world.
    pub gravity: glam::Vec2,
    /// Which simulation dimension to run. Only 2D exists yet; see
    /// [`PhysicsDimension`].
    pub dimension: PhysicsDimension,
    /// Which backend implementation to use. Only Rapier exists yet; see
    /// [`PhysicsBackendName`].
    pub backend: PhysicsBackendName,
}

impl PhysicsConfig {
    /// Builds an explicit config. Prefer [`PhysicsConfig::default`]
    /// unless a field genuinely differs — the default is the documented
    /// v0.0.11 world (earth-like 2D gravity, rapier2d).
    pub fn new(
        gravity: glam::Vec2,
        dimension: PhysicsDimension,
        backend: PhysicsBackendName,
    ) -> Self {
        Self {
            gravity,
            dimension,
            backend,
        }
    }
}

impl Default for PhysicsConfig {
    /// Earth-like 2D gravity (`[0.0, -9.81]`, y-up), 2D dimension,
    /// Rapier backend.
    fn default() -> Self {
        Self {
            gravity: glam::Vec2::new(0.0, -9.81),
            dimension: PhysicsDimension::TwoD,
            backend: PhysicsBackendName::Rapier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigid_body_constructors_carry_the_named_kind() {
        assert_eq!(RigidBody::dynamic().kind, RigidBodyKind::Dynamic);
        assert_eq!(RigidBody::fixed().kind, RigidBodyKind::Fixed);
        assert_eq!(
            RigidBody::kinematic_position().kind,
            RigidBodyKind::KinematicPosition
        );
    }

    #[test]
    fn valid_colliders_pass_validation() {
        assert!(Collider::ball(1.0).validate().is_ok());
        assert!(Collider::cuboid([2.0, 3.0]).validate().is_ok());
        assert!(Collider::capsule(1.0, 0.5).validate().is_ok());
    }

    #[test]
    fn default_material_is_valid() {
        assert!(ColliderMaterial::default().validate().is_ok());
    }

    #[test]
    fn ball_with_negative_radius_returns_typed_error() {
        let error = Collider::ball(-1.0).validate().expect_err("must fail");

        assert_eq!(error, PhysicsError::InvalidRadius { radius: -1.0 });
    }

    #[test]
    fn ball_with_zero_or_nonfinite_radius_returns_typed_error() {
        assert_eq!(
            Collider::ball(0.0)
                .validate()
                .expect_err("zero radius must fail"),
            PhysicsError::InvalidRadius { radius: 0.0 }
        );
        assert!(matches!(
            Collider::ball(f32::NAN).validate(),
            Err(PhysicsError::InvalidRadius { .. })
        ));
        assert_eq!(
            Collider::ball(f32::INFINITY)
                .validate()
                .expect_err("infinite radius must fail"),
            PhysicsError::InvalidRadius {
                radius: f32::INFINITY,
            }
        );
    }

    #[test]
    fn cuboid_with_negative_half_extent_returns_typed_error() {
        let half_extents = [2.0, -0.5];

        let error = Collider::cuboid(half_extents)
            .validate()
            .expect_err("must fail");

        assert_eq!(error, PhysicsError::InvalidHalfExtents { half_extents });
    }

    #[test]
    fn cuboid_with_zero_or_nonfinite_half_extent_returns_typed_error() {
        assert_eq!(
            Collider::cuboid([0.0, 1.0])
                .validate()
                .expect_err("zero extent must fail"),
            PhysicsError::InvalidHalfExtents {
                half_extents: [0.0, 1.0],
            }
        );
        assert!(matches!(
            Collider::cuboid([1.0, f32::NAN]).validate(),
            Err(PhysicsError::InvalidHalfExtents { .. })
        ));
    }

    #[test]
    fn capsule_with_bad_half_height_or_radius_returns_typed_error() {
        assert_eq!(
            Collider::capsule(-1.0, 0.5)
                .validate()
                .expect_err("negative half-height must fail"),
            PhysicsError::InvalidHalfHeight { half_height: -1.0 }
        );
        assert_eq!(
            Collider::capsule(1.0, 0.0)
                .validate()
                .expect_err("zero radius must fail"),
            PhysicsError::InvalidRadius { radius: 0.0 }
        );
    }

    #[test]
    fn material_with_negative_friction_returns_typed_error() {
        let error = ColliderMaterial::new(-0.1, 0.0)
            .validate()
            .expect_err("must fail");

        assert_eq!(error, PhysicsError::InvalidFriction { friction: -0.1 });
    }

    #[test]
    fn material_with_out_of_range_restitution_returns_typed_error() {
        assert_eq!(
            ColliderMaterial::new(0.5, 1.5)
                .validate()
                .expect_err("restitution above one must fail"),
            PhysicsError::InvalidRestitution { restitution: 1.5 }
        );
        assert!(matches!(
            ColliderMaterial::new(0.5, f32::NAN).validate(),
            Err(PhysicsError::InvalidRestitution { .. })
        ));
    }

    #[test]
    fn capsule_with_nan_half_height_or_infinite_radius_returns_typed_error() {
        assert!(matches!(
            Collider::capsule(f32::NAN, 0.5).validate(),
            Err(PhysicsError::InvalidHalfHeight { .. })
        ));
        assert_eq!(
            Collider::capsule(0.0, 0.5)
                .validate()
                .expect_err("zero half-height must fail"),
            PhysicsError::InvalidHalfHeight { half_height: 0.0 }
        );
        assert_eq!(
            Collider::capsule(1.0, f32::INFINITY)
                .validate()
                .expect_err("infinite radius must fail"),
            PhysicsError::InvalidRadius {
                radius: f32::INFINITY,
            }
        );
    }

    #[test]
    fn cuboid_with_infinite_half_extent_returns_typed_error() {
        let half_extents = [f32::INFINITY, 1.0];

        let error = Collider::cuboid(half_extents)
            .validate()
            .expect_err("must fail");

        assert_eq!(error, PhysicsError::InvalidHalfExtents { half_extents });
    }

    #[test]
    fn material_boundaries_zero_friction_and_full_restitution_are_valid() {
        assert!(ColliderMaterial::new(0.0, 0.0).validate().is_ok());
        assert!(ColliderMaterial::new(0.0, 1.0).validate().is_ok());
        assert!(ColliderMaterial::new(10.0, 1.0).validate().is_ok());
    }

    #[test]
    fn material_with_nonfinite_friction_or_negative_restitution_fails_typed() {
        assert!(matches!(
            ColliderMaterial::new(f32::NAN, 0.0).validate(),
            Err(PhysicsError::InvalidFriction { .. })
        ));
        assert_eq!(
            ColliderMaterial::new(f32::INFINITY, 0.0)
                .validate()
                .expect_err("infinite friction must fail"),
            PhysicsError::InvalidFriction {
                friction: f32::INFINITY,
            }
        );
        assert_eq!(
            ColliderMaterial::new(0.5, -0.25)
                .validate()
                .expect_err("negative restitution must fail"),
            PhysicsError::InvalidRestitution { restitution: -0.25 }
        );
        assert_eq!(
            ColliderMaterial::new(0.5, f32::INFINITY)
                .validate()
                .expect_err("infinite restitution must fail"),
            PhysicsError::InvalidRestitution {
                restitution: f32::INFINITY,
            }
        );
    }

    // Validation property: any finite positive scalar describes a valid
    // shape, so no sequence of in-range authoring inputs can be rejected.
    // Mirrors the `canary-ecs` op-sequence `proptest` style (model-first
    // oracle, arbitrary inputs, exact agreement).
    proptest::proptest! {
        #[test]
        fn finite_positive_scalars_always_validate(
            radius in 1e-6f32..1e6f32,
            hx in 1e-6f32..1e6f32,
            hy in 1e-6f32..1e6f32,
            half_height in 1e-6f32..1e6f32,
        ) {
            proptest::prop_assert!(radius.is_finite() && radius > 0.0);
            proptest::prop_assert!(Collider::ball(radius).validate().is_ok());
            proptest::prop_assert!(Collider::cuboid([hx, hy]).validate().is_ok());
            proptest::prop_assert!(Collider::capsule(half_height, radius).validate().is_ok());
            proptest::prop_assert!(
                ColliderMaterial::new(radius, 1.0).validate().is_ok(),
                "any finite non-negative friction with restitution 1.0 is valid"
            );
        }
    }

    #[test]
    fn default_config_is_earth_like_2d_rapier() {
        let config = PhysicsConfig::default();

        assert_eq!(config.gravity, glam::Vec2::new(0.0, -9.81));
        assert_eq!(config.dimension, PhysicsDimension::TwoD);
        assert_eq!(config.backend, PhysicsBackendName::Rapier);
        assert_eq!(config.dimension.as_str(), "2d");
        assert_eq!(config.backend.as_str(), "rapier");
    }

    #[test]
    fn velocity_zero_and_gravity_scale_defaults_hold() {
        assert_eq!(
            Velocity::zero(),
            Velocity {
                linvel: [0.0, 0.0],
                angvel: 0.0,
            }
        );
        assert_eq!(Velocity::default(), Velocity::zero());
        assert_eq!(GravityScale::default(), GravityScale(1.0));
        assert_eq!(GravityScale::none(), GravityScale(0.0));
        assert_eq!(LockedAxes::unlocked(), LockedAxes::default());
        assert!(LockedAxes::rotation_locked().lock_rotation);
    }
}
