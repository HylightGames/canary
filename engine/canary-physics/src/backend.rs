// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The object-safe, leak-free [`PhysicsBackend`] seam.
//!
//! Engine and gameplay code program against [`PhysicsBackend`]; concrete
//! solvers (Rapier2D in Task 4, Jolt/Rapier3D post-v0.1.0, or a
//! user-authored backend — `docs/architecture/physics.md` makes the trait
//! a public seam, not a first-party menu) sit behind it. "Leak-free"
//! means no third-party type appears in any public signature here:
//! backends translate Canary concepts in and 2D pose parts out, and
//! whatever the solver requires internally stays private. `cargo doc`
//! plus a grep for `rapier`/`nalgebra` in public signatures re-verifies
//! that on every change (Task 6 owns the automated recheck).

use crate::{Collider, ColliderMaterial, RigidBody, Velocity};

/// The fixed simulation timestep in seconds (60 Hz).
///
/// Every [`PhysicsBackend::step`] call must pass exactly this value —
/// the comparison is exact equality, not an epsilon band, so passing a
/// measured frame delta fails loudly instead of silently degrading the
/// determinism the fixed timestep exists to provide (see
/// `docs/architecture/physics.md`: reproducibility for multiplayer
/// prediction and record/replay tooling). The accumulator that turns
/// variable frame time into whole fixed steps (clamped, spiral-guarded)
/// is Task 4's `PhysicsClock`; this constant is shared so the trait's
/// contract and the clock cannot drift apart.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// Typed physics failures.
///
/// Validation errors name the offending value (never a bare bool), so a
/// failing spawn logs *which* extent was degenerate. Stale-handle
/// conditions are NOT errors — see [`PhysicsBackend`]'s method docs for
/// why liveness reports use `bool`/`Option` instead.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum PhysicsError {
    /// Ball radius or capsule cap radius was zero, negative, or
    /// non-finite. Carries the rejected value for diagnostics.
    #[error("collider radius must be finite and positive, got {radius}")]
    InvalidRadius {
        /// The rejected radius.
        radius: f32,
    },
    /// A cuboid half-extent was zero, negative, or non-finite. Carries
    /// the rejected pair for diagnostics.
    #[error("cuboid half-extents must be finite and positive, got {half_extents:?}")]
    InvalidHalfExtents {
        /// The rejected half-extents.
        half_extents: [f32; 2],
    },
    /// A capsule's cylindrical-section half-height was zero, negative,
    /// or non-finite. Carries the rejected value for diagnostics.
    #[error("capsule half-height must be finite and positive, got {half_height}")]
    InvalidHalfHeight {
        /// The rejected half-height.
        half_height: f32,
    },
    /// Friction was negative or non-finite. Carries the rejected value.
    #[error("friction must be finite and non-negative, got {friction}")]
    InvalidFriction {
        /// The rejected friction coefficient.
        friction: f32,
    },
    /// Restitution was outside `0.0..=1.0` or non-finite. Carries the
    /// rejected value.
    #[error("restitution must be finite and within 0.0..=1.0, got {restitution}")]
    InvalidRestitution {
        /// The rejected restitution.
        restitution: f32,
    },
    /// A body-creation pose was non-finite. Unlike
    /// [`PhysicsBackend::set_gravity`] (which retains the previous value
    /// on bad input), creation has no previous value to keep, so it
    /// fails instead of inventing a pose.
    #[error("body pose must be finite, got translation {translation:?} rotation {rotation}")]
    NonFinitePose {
        /// The rejected translation.
        translation: [f32; 2],
        /// The rejected rotation.
        rotation: f32,
    },
    /// [`PhysicsBackend::step`] was called with anything other than
    /// [`FIXED_DT`]. This is a *usage* error, not a simulation error:
    /// variable timesteps silently forfeit the reproducibility fixed
    /// stepping exists for, so the trait refuses them loudly and the
    /// Task 4 accumulator owns the frame-dt-to-fixed-steps conversion.
    #[error("physics must step at the fixed timestep {expected}, got {got}")]
    VariableTimestep {
        /// The timestep the caller passed.
        got: f32,
        /// The only accepted timestep ([`FIXED_DT`]).
        expected: f32,
    },
    /// A collider was attached to a body handle the backend does not
    /// know (never issued, removed, or recycled). Attaching to a dead
    /// body is always a caller ordering bug — unlike
    /// [`PhysicsBackend::remove_body`], where double-remove after a
    /// despawn race is expected — so it fails instead of returning a
    /// bool. Carries the rejected handle's raw parts for diagnostics.
    #[error("no live body for handle index {index} generation {generation}")]
    UnknownBody {
        /// The rejected handle's slot index.
        index: u32,
        /// The rejected handle's generation.
        generation: u64,
    },
}

/// An opaque, generational key for a body owned by a [`PhysicsBackend`].
///
/// The shape mirrors [`canary_ecs::Entity`](https://github.com/HylightGames/canary/blob/dev/engine/canary-ecs/src/entity.rs)
/// (`index` + `generation`) deliberately: bare indices alias after
/// remove-plus-recreate (slot 2 freed, then handed to a new body, makes
/// an old handle to slot 2 drive the wrong body), and that aliasing
/// class is exactly what `Entity`'s generation field already solved for
/// this workspace. Reusing the proven shape means the handle/store
/// invariant reasoning — and its `proptest` style — transfers wholesale.
/// (The manual-`impl` ceremony of `AssetHandle` is unnecessary here:
/// with no type parameter there are no implicit `T: Trait` bounds for a
/// derive to add — the ADR 0010 manual-before-derive precedent only
/// bites generic handles.)
///
/// Handles are plain keys; the backend owns everything. Equality and
/// hashing consider `index` and `generation` only. Forging a handle via
/// [`BodyHandle::from_raw_parts`] is safe by construction: every method
/// checks liveness itself and reports stale handles as `None`/`false`/
/// [`PhysicsError::UnknownBody`], never as another body's data — the
/// same guarantee `Entity::from_raw_parts` gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyHandle {
    index: u32,
    generation: u64,
}

impl BodyHandle {
    /// The slot index this handle points at. Never a stable identifier
    /// on its own: slots are recycled and only
    /// [`BodyHandle::generation`] disambiguates reuses. Exposed for
    /// debugging, diagnostics, and boundaries that serialize handles.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The generation of the slot this handle was issued for. A handle
    /// is live only while the backend's slot still carries this exact
    /// generation; any mismatch (removed, or recycled for a new body)
    /// makes the handle stale, and stale handles resolve to
    /// `None`/`false`/ [`PhysicsError::UnknownBody`], never to another
    /// body's state.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Reconstructs a handle from raw `index` then `generation` parts,
    /// matching [`BodyHandle::index`]/[`BodyHandle::generation`]'s order.
    ///
    /// Exists for boundaries that cannot pass an opaque handle through
    /// directly (serialization, tests) and must rebuild one from bits.
    /// Like `Entity::from_raw_parts`, this verifies nothing: feeding the
    /// result to any [`PhysicsBackend`] method is exactly as safe as
    /// passing a genuinely stale handle, because the backend checks
    /// liveness itself. This is also the constructor external
    /// (out-of-crate) backends use to mint handles from their own slot
    /// bookkeeping — no `pub(crate)` constructor could serve them, and
    /// none is needed, because unguessable construction was never the
    /// safety mechanism; liveness checking is.
    pub fn from_raw_parts(index: u32, generation: u64) -> Self {
        Self { index, generation }
    }
}

/// An opaque, generational key for a collider owned by a
/// [`PhysicsBackend`].
///
/// Same contract as [`BodyHandle`] (generational liveness, safe
/// [`ColliderHandle::from_raw_parts`] reconstruction, backend-owned
/// storage); a separate type so a collider handle can never be passed
/// where a body handle is expected. The compiler refuses the mix-up
/// that identical `u32`/`u64` shapes would allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColliderHandle {
    index: u32,
    generation: u64,
}

impl ColliderHandle {
    /// The slot index this handle points at. See [`BodyHandle::index`].
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The generation of the slot this handle was issued for. See
    /// [`BodyHandle::generation`].
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Reconstructs a handle from raw `index` then `generation` parts.
    /// See [`BodyHandle::from_raw_parts`] for why this is public and why
    /// that is safe.
    pub fn from_raw_parts(index: u32, generation: u64) -> Self {
        Self { index, generation }
    }
}

/// The solver seam: what engine and game code may ask of ANY physics
/// backend, with no backend crate's types visible.
///
/// # Object safety (why these exact shapes)
///
/// Backends are user-swappable (`Box<dyn PhysicsBackend>` behind
/// configuration, per ADR 0019), so every method here must be
/// dispatchable through a vtable: no generic type parameters, no
/// `Self`-returning constructors, no associated types. Concretely, that
/// rules out the conveniences a concrete API would reach for (`fn
/// create<T: Into<...>>`, builder returns, `associated World` types) —
/// each would silently break `dyn` compatibility, which the
/// `is_object_safe_through_dyn` test below pins. Adding a method later
/// is additive for the trait but breaking for external implementors;
/// that is the known cost of a public seam, and the reason the Task 3
/// cut stays minimal (every method here has a v0.0.11 consumer).
///
/// # Leak freedom (why primitives + glam + own types only)
///
/// Rapier speaks `nalgebra`; Jolt will speak its own math. If either
/// appeared in these signatures, every game crate would transitively
/// depend on that solver's vocabulary and swapping backends would move
/// game-facing API — the exact future ADR 0019 forbids. So: poses cross
/// as `[f32; 2]` arrays plus an `f32` z-rotation (the most boundary-safe
/// spelling — no dependency needed on the far side, including a future
/// WASM/plugin boundary), with `glam::Vec2` only where the ECS-adjacent
/// side already speaks `glam` (the sync return). Handles are crate-owned
/// opaques, never solver IDs.
///
/// # Stale handles are signals, not errors (why `bool`/`Option`)
///
/// Despawn racing a step is normal ECS life, not a caller bug, so
/// handle-targeted methods report liveness the way `World` does —
/// "missing is `None`/`false`, not a distinct error case" (see
/// `World::resource`/`remove_resource` docs): [`PhysicsBackend::sync_transform`]
/// returns `None`, `remove_body`/`set_velocity`/`apply_impulse` return
/// `false`, and the Task 4 system skips. Creation and stepping keep
/// `Result` because degenerate input or a variable timestep there IS a
/// caller bug worth naming loudly.
pub trait PhysicsBackend {
    /// Creates a simulated body from its [`RigidBody`] role and initial
    /// 2D pose, returning the opaque handle all later calls use.
    ///
    /// `translation` is world x/y; `rotation` is radians around the
    /// z-axis (the single 2D rotation). Both must be finite —
    /// non-finite poses fail with [`PhysicsError::NonFinitePose`]
    /// because creation has no previous state worth retaining.
    /// Component inputs arrive by reference (not by value) even though
    /// they are `Copy`: if a later shape (compound colliders, joint
    /// descriptors) outgrows `Copy`, no game-facing signature moves.
    ///
    /// Solver semantics worth knowing at this seam: a colliderless
    /// dynamic body has no mass (Rapier derives mass from colliders), so
    /// gravity cannot move it until `attach_collider` runs. That is the
    /// solver's rule, not an implementor's bug — but it surprises every
    /// first caller, hence stated here rather than discovered.
    fn create_body(
        &mut self,
        body: &RigidBody,
        translation: [f32; 2],
        rotation: f32,
    ) -> Result<BodyHandle, PhysicsError>;

    /// Attaches a validated [`Collider`] (with its [`ColliderMaterial`])
    /// to a live body. Runs [`Collider::validate`] and
    /// [`ColliderMaterial::validate`] first — invalid shapes fail with
    /// the validator's typed error before touching solver state — and
    /// fails with [`PhysicsError::UnknownBody`] for a stale body handle,
    /// which is always a caller ordering bug (contrast
    /// [`PhysicsBackend::remove_body`], where double-remove is routine).
    fn attach_collider(
        &mut self,
        body: BodyHandle,
        collider: &Collider,
        material: &ColliderMaterial,
    ) -> Result<ColliderHandle, PhysicsError>;

    /// Advances simulation by exactly [`FIXED_DT`], and nothing else.
    ///
    /// Any other `dt` — a measured frame delta, an accumulated
    /// remainder, a "close enough" `0.016` literal — fails with
    /// [`PhysicsError::VariableTimestep`]. Rationale: fixed stepping is
    /// the precondition for every reproducibility claim in
    /// `docs/architecture/physics.md` (multiplayer prediction,
    /// record/replay), and silently accepting near-fixed dt would let a
    /// well-meaning caller degrade determinism one frame at a time with
    /// no signal. The Task 4 accumulator owns converting frame time
    /// into whole fixed steps; this method owns refusing anything else.
    fn step(&mut self, dt: f32) -> Result<(), PhysicsError>;

    /// Reads a body's current 2D pose WITHOUT touching ECS storage:
    /// position plus z-rotation, or `None` for a stale handle (skip it).
    ///
    /// Why the split — return parts instead of applying them? Three
    /// reasons. First, single-writer discipline: only systems own ECS
    /// component storage, so only the Task 4 system may write
    /// `Transform` (a backend writing components behind the scheduler's
    /// back would break access tracking). Second, z-preservation: the
    /// 2D game lives on a z-pinned plane and the backend must never see
    /// — let alone clobber — the z, off-axis rotation, or scale the
    /// system preserves when it applies these parts. Third,
    /// testability: a backend proving "same steps, same poses" needs no
    /// `World` at all. Task 4 implements the application; this method
    /// is deliberately the read half.
    fn sync_transform(&self, body: BodyHandle) -> Option<(glam::Vec2, f32)>;

    /// Destroys a body (and its attached colliders). Returns whether a
    /// live body was removed: `false` for a stale or unknown handle.
    ///
    /// `false`-on-stale is load-bearing, not lenient: despawn racing a
    /// step means double-remove is routine ECS life, and failing it
    /// would turn every teardown race into a spurious error. Removal
    /// bumps the slot's generation, so pre-remove handles stay stale
    /// forever and can never alias a later body recycled into the slot.
    fn remove_body(&mut self, body: BodyHandle) -> bool;

    /// Overwrites a live body's velocity. Returns `false` (applying
    /// nothing) for a stale handle or non-finite input — both mean
    /// "nothing was applied," which is all the caller needs for skip
    /// semantics, and a poisoned (NaN) velocity must never reach solver
    /// state. Validate-and-report needs live at the call site via
    /// [`Velocity`]'s construction; the bool keeps the per-frame write
    /// path infallible-shaped like the rest of the handle-targeted API.
    fn set_velocity(&mut self, body: BodyHandle, velocity: &Velocity) -> bool;

    /// Adds an instantaneous momentum change to a live body (kicks,
    /// explosions, jump impulses). Returns `false` (applying nothing)
    /// for a stale handle or non-finite components, for the same
    /// skip-semantics reason as [`PhysicsBackend::set_velocity`].
    /// Sustained forces are OUT of this cut (no force-accumulator API
    /// until a consumer needs multi-frame pushes); per-frame re-applied
    /// impulses cover the v0.0.11 scenes.
    fn apply_impulse(&mut self, body: BodyHandle, impulse: [f32; 2], angular_impulse: f32) -> bool;

    /// Replaces the world's gravity acceleration (world units per
    /// second squared). Non-finite components are ignored with the
    /// previous gravity retained — gravity has a persistent current
    /// value worth keeping, unlike the one-shot inputs above. The
    /// default source of this value is [`crate::PhysicsConfig`]; this
    /// method is the runtime override (pause menus zeroing gravity,
    /// space levels flipping it).
    fn set_gravity(&mut self, gravity: [f32; 2]);

    /// The current gravity acceleration. Round-trips
    /// [`PhysicsBackend::set_gravity`].
    fn gravity(&self) -> [f32; 2];

    /// How many live bodies the backend currently owns. Exists for
    /// tests, debug overlays, and the benchmark harness — not for
    /// gameplay logic, which reasons about entities, never backend
    /// internals.
    fn body_count(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// Minimal in-memory [`PhysicsBackend`] proving the trait's
    /// contracts without any solver: a generational slot map plus
    /// gravity. Task 4's Rapier backend must honor the same observable
    /// behavior; this stub is the executable form of "honor."
    struct StubBackend {
        slots: HashMap<u32, StubSlot>,
        next_index: u32,
        live: usize,
        gravity_value: [f32; 2],
    }

    struct StubSlot {
        generation: u64,
        alive: bool,
        translation: [f32; 2],
        rotation: f32,
    }

    impl StubBackend {
        fn new() -> Self {
            Self {
                slots: HashMap::new(),
                next_index: 0,
                live: 0,
                gravity_value: [0.0, -9.81],
            }
        }

        fn live_slot(&self, handle: BodyHandle) -> Option<&StubSlot> {
            self.slots
                .get(&handle.index)
                .filter(|slot| slot.alive && slot.generation == handle.generation)
        }

        fn live_slot_mut(&mut self, handle: BodyHandle) -> Option<&mut StubSlot> {
            self.slots
                .get_mut(&handle.index)
                .filter(|slot| slot.alive && slot.generation == handle.generation)
        }
    }

    impl PhysicsBackend for StubBackend {
        fn create_body(
            &mut self,
            _body: &RigidBody,
            translation: [f32; 2],
            rotation: f32,
        ) -> Result<BodyHandle, PhysicsError> {
            if translation.iter().any(|c| !c.is_finite()) || !rotation.is_finite() {
                return Err(PhysicsError::NonFinitePose {
                    translation,
                    rotation,
                });
            }
            let index = self.next_index;
            self.next_index = self
                .next_index
                .checked_add(1)
                .expect("test stub ran out of body indices (matches RapierBackend policy)");
            self.slots.insert(
                index,
                StubSlot {
                    generation: 0,
                    alive: true,
                    translation,
                    rotation,
                },
            );
            self.live += 1;
            Ok(BodyHandle::from_raw_parts(index, 0))
        }

        fn attach_collider(
            &mut self,
            body: BodyHandle,
            collider: &Collider,
            material: &ColliderMaterial,
        ) -> Result<ColliderHandle, PhysicsError> {
            collider.validate()?;
            material.validate()?;
            if self.live_slot(body).is_none() {
                return Err(PhysicsError::UnknownBody {
                    index: body.index(),
                    generation: body.generation(),
                });
            }
            Ok(ColliderHandle::from_raw_parts(body.index(), 0))
        }

        fn step(&mut self, dt: f32) -> Result<(), PhysicsError> {
            if dt != FIXED_DT {
                return Err(PhysicsError::VariableTimestep {
                    got: dt,
                    expected: FIXED_DT,
                });
            }
            Ok(())
        }

        fn sync_transform(&self, body: BodyHandle) -> Option<(glam::Vec2, f32)> {
            self.live_slot(body).map(|slot| {
                (
                    glam::Vec2::new(slot.translation[0], slot.translation[1]),
                    slot.rotation,
                )
            })
        }

        fn remove_body(&mut self, body: BodyHandle) -> bool {
            match self.slots.get_mut(&body.index()) {
                Some(slot) if slot.alive && slot.generation == body.generation() => {
                    slot.alive = false;
                    slot.generation = slot.generation.wrapping_add(1);
                    self.live -= 1;
                    true
                }
                _ => false,
            }
        }

        fn set_velocity(&mut self, body: BodyHandle, velocity: &Velocity) -> bool {
            if !velocity.linvel.iter().all(|c| c.is_finite()) || !velocity.angvel.is_finite() {
                return false;
            }
            self.live_slot_mut(body).is_some()
        }

        fn apply_impulse(
            &mut self,
            body: BodyHandle,
            impulse: [f32; 2],
            angular_impulse: f32,
        ) -> bool {
            if impulse.iter().any(|c| !c.is_finite()) || !angular_impulse.is_finite() {
                return false;
            }
            self.live_slot_mut(body).is_some()
        }

        fn set_gravity(&mut self, gravity: [f32; 2]) {
            if gravity.iter().all(|c| c.is_finite()) {
                self.gravity_value = gravity;
            }
        }

        fn gravity(&self) -> [f32; 2] {
            self.gravity_value
        }

        fn body_count(&self) -> usize {
            self.live
        }
    }

    /// Compile-time proof the trait is object-safe: if any method grew
    /// a generic parameter, returned `Self`, or added an associated
    /// type, this function (and the `Box` below) would stop compiling —
    /// which is exactly the tripwire user-swappable backends need.
    fn assert_dyn_compatible(_: &dyn PhysicsBackend) {}

    #[test]
    fn trait_is_object_safe_through_dyn() {
        let mut backend: Box<dyn PhysicsBackend> = Box::new(StubBackend::new());
        assert_dyn_compatible(&*backend);

        let handle = backend
            .create_body(&RigidBody::dynamic(), [1.0, 2.0], 0.5)
            .expect("valid body must be created");
        backend.step(FIXED_DT).expect("fixed step must succeed");

        let (position, angle) = backend.sync_transform(handle).expect("live body must sync");
        assert!((position.x - 1.0).abs() < f32::EPSILON);
        assert!((position.y - 2.0).abs() < f32::EPSILON);
        assert!((angle - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn step_with_variable_timestep_is_a_usage_error() {
        let mut backend = StubBackend::new();

        let error = backend
            .step(1.0 / 30.0)
            .expect_err("half-rate dt must fail");

        assert_eq!(
            error,
            PhysicsError::VariableTimestep {
                got: 1.0 / 30.0,
                expected: FIXED_DT,
            }
        );
    }

    #[test]
    fn create_body_with_nonfinite_pose_fails_typed() {
        let mut backend = StubBackend::new();

        assert!(matches!(
            backend.create_body(&RigidBody::dynamic(), [f32::NAN, 0.0], 0.0),
            Err(PhysicsError::NonFinitePose { .. })
        ));
    }

    #[test]
    fn attach_collider_validates_shape_before_touching_solver_state() {
        let mut backend = StubBackend::new();
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("valid body must be created");

        let error = backend
            .attach_collider(
                body,
                &Collider::cuboid([1.0, -2.0]),
                &ColliderMaterial::default(),
            )
            .expect_err("negative half-extent must fail");

        assert_eq!(
            error,
            PhysicsError::InvalidHalfExtents {
                half_extents: [1.0, -2.0],
            }
        );
    }

    #[test]
    fn attach_collider_to_stale_body_fails_with_unknown_body() {
        let mut backend = StubBackend::new();
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("valid body must be created");
        assert!(backend.remove_body(body));

        let error = backend
            .attach_collider(body, &Collider::ball(1.0), &ColliderMaterial::default())
            .expect_err("attach to removed body must fail");

        assert_eq!(
            error,
            PhysicsError::UnknownBody {
                index: body.index(),
                generation: body.generation(),
            }
        );
    }

    #[test]
    fn removed_handles_stay_stale_across_every_method() {
        let mut backend = StubBackend::new();
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("valid body must be created");
        assert!(backend.remove_body(body));

        assert!(backend.sync_transform(body).is_none());
        assert!(!backend.set_velocity(body, &Velocity::zero()));
        assert!(!backend.apply_impulse(body, [1.0, 0.0], 0.0));
        assert!(!backend.remove_body(body));
        assert_eq!(backend.body_count(), 0);
    }

    #[test]
    fn forged_handles_never_resolve_to_live_bodies() {
        let mut backend = StubBackend::new();
        let forged = BodyHandle::from_raw_parts(999, 0);

        assert!(backend.sync_transform(forged).is_none());
        assert!(!backend.set_velocity(forged, &Velocity::zero()));
        assert!(!backend.remove_body(forged));
    }

    #[test]
    fn nonfinite_velocity_and_impulse_apply_nothing() {
        let mut backend = StubBackend::new();
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("valid body must be created");

        assert!(!backend.set_velocity(
            body,
            &Velocity {
                linvel: [f32::INFINITY, 0.0],
                angvel: 0.0,
            }
        ));
        assert!(!backend.apply_impulse(body, [0.0, f32::NAN], 0.0));
    }

    #[test]
    fn gravity_round_trips_and_ignores_nonfinite_input() {
        let mut backend = StubBackend::new();
        assert_eq!(backend.gravity(), [0.0, -9.81]);

        backend.set_gravity([0.0, -1.62]);
        assert_eq!(backend.gravity(), [0.0, -1.62]);

        backend.set_gravity([0.0, f32::NAN]);
        assert_eq!(backend.gravity(), [0.0, -1.62]);
    }

    // Handle/slot lifecycle property: arbitrary create/remove sequences
    // keep the backend's live set exactly equal to a model set, so no
    // sequence of operations can alias, leak, or resurrect a body. The
    // `proptest` style mirrors `canary-ecs`'s own world op-sequence test
    // by design (same aliasing class, same proof shape).
    proptest::proptest! {
        #[test]
        fn live_set_matches_model_across_create_remove_sequences(
            ops in proptest::collection::vec(
                proptest::sample::select(&[0u8, 1]),
                1..60,
            ),
        ) {
            let mut backend = StubBackend::new();
            let mut issued: Vec<BodyHandle> = Vec::new();
            let mut model: HashSet<(u32, u64)> = HashSet::new();

            for op in ops {
                if op == 0 || issued.is_empty() {
                    let handle = backend
                        .create_body(&RigidBody::dynamic(), [0.0, 0.0], 0.0)
                        .expect("stub creation cannot fail on finite input");
                    model.insert((handle.index(), handle.generation()));
                    issued.push(handle);
                } else {
                    let handle = issued[issued.len() - 1];
                    let was_live = model.remove(&(handle.index(), handle.generation()));
                    proptest::prop_assert_eq!(backend.remove_body(handle), was_live);
                }
            }

            proptest::prop_assert_eq!(backend.body_count(), model.len());
            for handle in issued {
                let expect_live = model.contains(&(handle.index(), handle.generation()));
                proptest::prop_assert_eq!(backend.sync_transform(handle).is_some(), expect_live);
            }
        }
    }
}
