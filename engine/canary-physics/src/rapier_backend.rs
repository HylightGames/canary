// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The private Rapier2D backend: [`RapierBackend`], the Task 4 implementor
//! of [`PhysicsBackend`].
//!
//! This module is intentionally private (`mod rapier_backend;` in
//! `lib.rs`, with only the [`RapierBackend`] type re-exported): engine
//! and game code program against the [`PhysicsBackend`]
//! trait, never against this concrete type's rapier-flavored internals.
//! The re-export exists so the [`RapierBackend`] resource type can be
//! named in [`SystemAccess`] declarations
//! and in `World::resource::<RapierBackend>()` calls — naming the *type*
//! is not depending on the *solver*, the same way naming a trait is not
//! depending on its implementors. No rapier (or `nalgebra`) type appears
//! in any public signature here; the Task 6 leak recheck (`cargo doc`
//! plus `grep` over public signatures) pins that.
//!
//! # Mechanism (why this shape)
//!
//! Rapier owns three arenas the backend wraps without exposing:
//! [`RigidBodySet`](rapier::dynamics::RigidBodySet),
//! [`ColliderSet`](rapier::geometry::ColliderSet), and the pipeline
//! scratch ([`PhysicsPipeline`](rapier::pipeline::PhysicsPipeline) plus
//! its island/broad/narrow/joint/ccd companions). Canary code never sees
//! rapier handles: every [`BodyHandle`] is an opaque
//! generational key into a slot map owned here, mirroring
//! [`canary_ecs::Entity`]'s `index` + `generation` shape deliberately so
//! the aliasing-after-recycle reasoning (and its `proptest` style)
//! transfers wholesale. A slot's generation bumps on every remove, so a
//! handle to a removed body stays stale forever and can never resolve to
//! a later body recycled into the same slot — the stale-returns-`None`/
//! `false` contract in [`PhysicsBackend`], not an
//! accident of it.
//!
//! Gravity is passed to
//! [`PhysicsPipeline::step`]
//! on every fixed step (Rapier takes gravity as a per-step argument, not
//! stored world state), so [`set_gravity`]
//! only updates the stored vector the next [`step`]
//! forwards. Non-finite gravity input is ignored with the previous value
//! retained: gravity has a persistent current value worth keeping, unlike
//! one-shot velocity/impulse inputs where "apply nothing" is the honest
//! skip semantic.
//!
//! # Solver-tuning notes (librarian corroboration, verified at
//! implementation against rapier2d 0.35.3)
//!
//! - `IntegrationParameters::length_unit` stays at the default `1.0`:
//!   Canary world units are meters-scale (the default config's gravity is
//!   `[0.0, -9.81]`), which is exactly the regime the default targets.
//!   A pixel-scale game retunes this on its own backend instance later;
//!   baking a pixel assumption into the shared backend would silently
//!   rescale every meters-scale scene.
//! - Sustained forces are OUT of this cut (no force-accumulator API until
//!   a consumer needs multi-frame pushes), so there is no per-step force
//!   lifecycle to manage here — but `reset_forces` is still called after
//!   every step as hygiene against any force a future caller sets
//!   directly: forces persist until cleared, and a force that outlives
//!   the tick that set it is a leak by another name.
//! - NaN hygiene is enforced at the boundary that matters: every float
//!   entering solver state (poses, velocities, impulses, scales,
//!   materials) is finiteness-checked here and rejected before it touches
//!   Rapier. Defense in depth comes from polling the pipeline's
//!   [`Quarantine`] after every step (see
//!   [`step`](crate::PhysicsBackend::step)): if a future solver version
//!   ever auto-disables a body or collider whose state went non-finite
//!   mid-step, the backend destroys it solver-side so sync degrades to
//!   the stale-handle skip — and the Task 4 system re-creates it fresh
//!   from its `Transform` next tick — instead of freezing on a silently
//!   disabled body. Fuzz probing on 0.35.3 (max-magnitude velocities,
//!   extreme gravities, extreme spawn positions) never produced a
//!   quarantine report — the solver neutralizes finite extremes
//!   internally (CCD motion clamping) — so the drain is dormant
//!   version-drift armor, verified on its empty path by every step the
//!   suite runs. The sync side additionally refuses
//!   to write a non-finite pose into `Transform` (skip, never poison).
//! - Feature flags: this crate enables NO rapier features beyond the
//!   defaults (`dim2`, `f32`, `std`, `block-solver`). `parallel` pays off
//!   above ~50 bodies but introduces scheduler nondeterminism that would
//!   void the single-machine bit-identical determinism this task proves;
//!   `serde-serialize` has no persistence consumer yet. Both are deferred
//!   to the task that needs them, not speculated here.
//!
//! rapier2d is pure safe Rust, so this module contains no `unsafe` — and
//! no `unwrap()` outside tests either: every fallible rapier lookup
//! (`RigidBodySet::get`, `get_mut`) maps to the stale-handle skip the
//! trait promises.

use std::collections::HashMap;

use canary_ecs::Entity;

use crate::{
    BodyHandle, Collider, ColliderHandle, ColliderMaterial, PhysicsBackend, PhysicsError,
    RigidBody, RigidBodyKind, Velocity, FIXED_DT,
};

/// Re-export shim: keeps field/argument names in this module honest
/// without importing rapier names into scope (every rapier path below is
/// fully qualified, so a `grep` for bare solver types stays meaningful).
use rapier2d as rapier;

/// One generational slot in [`RapierBackend`]'s body store.
///
/// `generation` disambiguates recycles (see the module docs);
/// `rapier` is the solver-side handle, meaningful only inside this
/// module; `colliders` are the solver-side collider handles attached to
/// this body, removed together with it so no orphan collider survives
/// its body.
struct BodySlot {
    /// Solver-side body handle. Private: never leaves this module.
    rapier: rapier::dynamics::RigidBodyHandle,
    /// Generation this slot currently carries. A [`BodyHandle`] is live
    /// only while its generation matches.
    generation: u64,
    /// Whether this slot currently owns a live body.
    alive: bool,
    /// Solver-side colliders attached to this body.
    colliders: Vec<rapier::geometry::ColliderHandle>,
}

/// The Rapier2D backend: [`PhysicsBackend`] over rapier2d 0.35's
/// [`RigidBodySet`](rapier::dynamics::RigidBodySet) /
/// [`ColliderSet`](rapier::geometry::ColliderSet), stepped at exactly
/// [`FIXED_DT`] through [`PhysicsPipeline`](rapier::pipeline::PhysicsPipeline).
///
/// The body sets, the pipeline scratch, the generational slot map, and
/// the entity↔handle map are all private fields: the trait's methods
/// plus [`RapierBackend::new`] are the entire public surface, and none
/// of them names a rapier (or `nalgebra`) type. `nalgebra` does not even
/// appear in this module's code — and rapier2d's own math layer
/// (`rapier::math::Vector`) is built on a NEWER `glam` than this
/// workspace's `glam = "0.30"`, so the two `Vec2`s are distinct types
/// despite the same name. Every crossing therefore converts
/// field-by-field (`Vector::new(x, y)` in, `.x`/`.y` out) — friction
/// that is load-bearing, not incidental: it forces the boundary to stay
/// explicit and keeps both math vocabularies out of every signature.
pub struct RapierBackend {
    /// Solver-side bodies.
    bodies: rapier::dynamics::RigidBodySet,
    /// Solver-side colliders.
    colliders: rapier::geometry::ColliderSet,
    /// Pipeline scratch, reused across steps (see rapier's own docs:
    /// recreating it per step works but wastes the reused buffers).
    pipeline: rapier::pipeline::PhysicsPipeline,
    /// Pipeline companions, all threaded through every
    /// [`step`] call.
    islands: rapier::dynamics::IslandManager,
    /// Broad phase (BVH variant: rapier 0.35's current default).
    broad_phase: rapier::geometry::BroadPhaseBvh,
    /// Narrow phase (contact manifolds).
    narrow_phase: rapier::geometry::NarrowPhase,
    /// Joint sets: empty in this cut (joints are a deferred item), but
    /// [`step`] requires them,
    /// so they live here rather than being rebuilt per step.
    impulse_joints: rapier::dynamics::ImpulseJointSet,
    /// See [`RapierBackend::impulse_joints`].
    multibody_joints: rapier::dynamics::MultibodyJointSet,
    /// Continuous-collision-detection solver.
    ccd_solver: rapier::dynamics::CCDSolver,
    /// Per-step solver knobs (`dt` is always [`FIXED_DT`];
    /// `length_unit` stays `1.0`, see the module docs).
    integration_parameters: rapier::dynamics::IntegrationParameters,
    /// Gravity forwarded to the pipeline on every step.
    gravity: [f32; 2],
    /// Generational body slots keyed by slot index.
    slots: HashMap<u32, BodySlot>,
    /// Next fresh slot index (slots are never reused by index — removal
    /// bumps the generation instead, so indices grow monotonically and
    /// aliasing-by-recycle is impossible even before generations are
    /// consulted).
    next_index: u32,
    /// Mint counter for [`ColliderHandle`] opaques (colliders need no
    /// liveness tracking — the trait has no collider-targeted reads, and
    /// colliders die with their body — so a bare unique index suffices).
    next_collider_index: u32,
    /// Live body count (mirrors the number of alive slots; maintained
    /// incrementally so [`body_count`] is
    /// O(1)).
    live: usize,
    /// Bodies destroyed by the quarantine drain so far (see
    /// [`step`](crate::PhysicsBackend::step)). Exists for tests and
    /// debug overlays — not for gameplay logic — the same rationale as
    /// [`body_count`](crate::PhysicsBackend::body_count).
    quarantine_drains: u64,
    /// Entity↔handle map: which live [`BodyHandle`] each ECS entity owns.
    /// Owned here (not in a separate resource) so creation, removal, and
    /// liveness checks share one bookkeeping site; the Task 4 system
    /// reaches it through the `pub(crate)` helpers below.
    entities: HashMap<Entity, BodyHandle>,
}

impl RapierBackend {
    /// Builds an empty backend with the given gravity acceleration
    /// (world units per second squared). Gravity is only *stored* here;
    /// it reaches the solver on the next
    /// [`step`](crate::PhysicsBackend::step), which forwards it to the
    /// pipeline. Non-finite components fall back to `[0.0, 0.0]` rather
    /// than failing: construction has no previous value to retain, and a
    /// zero-gravity backend is the honest degradation (the system
    /// normally passes [`PhysicsConfig`](crate::PhysicsConfig) gravity,
    /// which is finite by construction).
    pub fn new(gravity: [f32; 2]) -> Self {
        let sanitized = if gravity.iter().all(|c| c.is_finite()) {
            gravity
        } else {
            [0.0, 0.0]
        };
        let integration_parameters = rapier::dynamics::IntegrationParameters {
            dt: FIXED_DT,
            ..rapier::dynamics::IntegrationParameters::default()
        };
        Self {
            bodies: rapier::dynamics::RigidBodySet::new(),
            colliders: rapier::geometry::ColliderSet::new(),
            pipeline: rapier::pipeline::PhysicsPipeline::new(),
            islands: rapier::dynamics::IslandManager::new(),
            broad_phase: rapier::geometry::BroadPhaseBvh::new(),
            narrow_phase: rapier::geometry::NarrowPhase::new(),
            impulse_joints: rapier::dynamics::ImpulseJointSet::new(),
            multibody_joints: rapier::dynamics::MultibodyJointSet::new(),
            ccd_solver: rapier::dynamics::CCDSolver::new(),
            integration_parameters,
            gravity: sanitized,
            slots: HashMap::new(),
            next_index: 0,
            next_collider_index: 0,
            live: 0,
            quarantine_drains: 0,
            entities: HashMap::new(),
        }
    }

    /// The [`BodyHandle`] tracked for `entity`, if the entity has a body
    /// whose slot is still live. A tracked handle whose slot died (or
    /// whose generation moved on) reports `None` — the caller treats it
    /// exactly like "no body" and re-creates, so a despawn the system has
    /// not cleaned up yet can never panic or alias.
    pub(crate) fn entity_handle(&self, entity: Entity) -> Option<BodyHandle> {
        self.entities
            .get(&entity)
            .copied()
            .filter(|handle| self.is_live(*handle))
    }

    /// Records that `entity` owns `handle`. Called right after a
    /// successful creation, while the handle is known live.
    pub(crate) fn track_entity(&mut self, entity: Entity, handle: BodyHandle) {
        self.entities.insert(entity, handle);
    }

    /// Forgets `entity`'s tracked handle (if any), returning it so the
    /// caller can destroy the solver-side body. Tracking removal and
    /// body destruction stay paired at the call site.
    pub(crate) fn untrack_entity(&mut self, entity: Entity) -> Option<BodyHandle> {
        self.entities.remove(&entity)
    }

    /// Every currently tracked entity. The system uses this to find map
    /// entries whose entity died or lost its body — snapshot form (`Vec`)
    /// so the caller can mutate tracking while iterating.
    pub(crate) fn tracked_entities(&self) -> Vec<Entity> {
        self.entities.keys().copied().collect()
    }

    /// How many bodies the quarantine drain has destroyed so far (see
    /// [`step`](crate::PhysicsBackend::step)). Zero in ordinary play —
    /// boundary hygiene rejects non-finite inputs before they reach the
    /// solver — so a nonzero count means huge-but-finite inputs overflowed
    /// mid-step and the affected bodies were recycled through the
    /// stale-handle path.
    pub fn quarantine_drain_count(&self) -> u64 {
        self.quarantine_drains
    }

    /// Destroys the body owned by slot `index` solver-side (body plus its
    /// colliders), marks the slot dead, and bumps its generation so
    /// pre-removal handles stay stale forever. Returns whether a live
    /// body was destroyed. The shared primitive behind
    /// [`remove_body`](crate::PhysicsBackend::remove_body) and the
    /// quarantine drain: both paths must leave exactly the same
    /// dead-slot-plus-stale-handle state, or the system's
    /// re-create-from-`Transform` recovery would diverge by removal
    /// cause.
    fn destroy_slot(&mut self, index: u32) -> bool {
        let rapier_handle = match self.slots.get_mut(&index) {
            Some(slot) if slot.alive => {
                slot.alive = false;
                slot.generation = slot.generation.wrapping_add(1);
                for collider in std::mem::take(&mut slot.colliders) {
                    self.colliders
                        .remove(collider, &mut self.islands, &mut self.bodies, true);
                }
                slot.rapier
            }
            _ => return false,
        };
        self.bodies.remove(
            rapier_handle,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
        self.live -= 1;
        true
    }

    /// Polls the pipeline's quarantine report and destroys every affected
    /// body solver-side (see the module docs for why removal — rather
    /// than leaving rapier's auto-disabled body in place — is the honest
    /// degradation). Runs after every step; fuzz probing on 0.35.3 never
    /// produced a report, so the cost in practice is one `is_empty`
    /// check and the slot scan only runs if a future solver version
    /// quarantines more eagerly.
    fn drain_quarantine(&mut self) {
        if self.pipeline.quarantine().is_empty() {
            return;
        }
        let bodies: Vec<rapier::dynamics::RigidBodyHandle> =
            self.pipeline.quarantine().bodies().to_vec();
        let colliders: Vec<rapier::geometry::ColliderHandle> =
            self.pipeline.quarantine().colliders().to_vec();
        let mut doomed: Vec<u32> = Vec::new();
        for (index, slot) in self.slots.iter() {
            if !slot.alive {
                continue;
            }
            if bodies.contains(&slot.rapier) || slot.colliders.iter().any(|c| colliders.contains(c))
            {
                doomed.push(*index);
            }
        }
        for index in doomed {
            if self.destroy_slot(index) {
                self.quarantine_drains += 1;
            }
        }
    }

    /// Whether `handle` names a live slot: the slot exists, is marked
    /// alive, and still carries the handle's generation. Every
    /// handle-targeted method routes through this; forging a handle via
    /// [`BodyHandle::from_raw_parts`] is safe by construction because
    /// this check (not unguessability) is the safety mechanism.
    fn is_live(&self, handle: BodyHandle) -> bool {
        self.slots
            .get(&handle.index())
            .is_some_and(|slot| slot.alive && slot.generation == handle.generation())
    }

    /// The live slot for `handle`, or `None` for a stale handle.
    fn live_slot(&self, handle: BodyHandle) -> Option<&BodySlot> {
        self.slots
            .get(&handle.index())
            .filter(|slot| slot.alive && slot.generation == handle.generation())
    }

    /// The live slot for `handle`, mutably, or `None` for stale.
    fn live_slot_mut(&mut self, handle: BodyHandle) -> Option<&mut BodySlot> {
        self.slots
            .get_mut(&handle.index())
            .filter(|slot| slot.alive && slot.generation == handle.generation())
    }

    /// Maps a [`RigidBodyKind`] to its rapier builder. Only one kinematic
    /// flavor exists (see [`RigidBodyKind`]'s docs for why
    /// velocity-kinematics is OUT of this cut).
    fn builder_for(kind: RigidBodyKind) -> rapier::dynamics::RigidBodyBuilder {
        match kind {
            RigidBodyKind::Dynamic => rapier::dynamics::RigidBodyBuilder::dynamic(),
            RigidBodyKind::Fixed => rapier::dynamics::RigidBodyBuilder::fixed(),
            RigidBodyKind::KinematicPosition => {
                rapier::dynamics::RigidBodyBuilder::kinematic_position_based()
            }
        }
    }

    /// Maps a [`Collider`] to its rapier builder. Shapes are described in
    /// half-measures on both sides of the boundary (see [`Collider`]'s
    /// docs), so this passes scalars through with no conversion math
    /// that could drift. Validation has already run (the trait method
    /// validates before reaching here); this function trusts it.
    fn collider_builder_for(collider: &Collider) -> rapier::geometry::ColliderBuilder {
        match *collider {
            Collider::Ball { radius } => rapier::geometry::ColliderBuilder::ball(radius),
            Collider::Capsule {
                half_height,
                radius,
            } => rapier::geometry::ColliderBuilder::capsule_y(half_height, radius),
            Collider::Cuboid { half_extents } => {
                rapier::geometry::ColliderBuilder::cuboid(half_extents[0], half_extents[1])
            }
        }
    }

    /// Maps [`LockedAxes`] to rapier's own
    /// `LockedAxes` bitflags. Same three degrees of freedom, different
    /// vocabulary — translated at exactly this boundary.
    pub(crate) fn rapier_locked_axes(locks: &crate::LockedAxes) -> rapier::dynamics::LockedAxes {
        let mut flags = rapier::dynamics::LockedAxes::empty();
        flags.set(
            rapier::dynamics::LockedAxes::TRANSLATION_LOCKED_X,
            locks.lock_translation_x,
        );
        flags.set(
            rapier::dynamics::LockedAxes::TRANSLATION_LOCKED_Y,
            locks.lock_translation_y,
        );
        flags.set(
            rapier::dynamics::LockedAxes::ROTATION_LOCKED_Z,
            locks.lock_rotation,
        );
        flags
    }

    /// Drives a position-kinematic body toward the game-authored pose:
    /// sets the next kinematic translation/rotation the solver moves the
    /// body to on the coming step. Returns whether anything was driven
    /// (`false` for stale handles, non-finite input, or bodies whose
    /// solver type is not position-kinematic — a dynamic body must never
    /// be teleported by the frame loop, which would fight integration).
    ///
    /// The Task 4 system calls this every tick for bodies whose
    /// [`RigidBodyKind`] is `KinematicPosition`, with the pose read from
    /// the entity's `Transform` — scripted platforms and doors are posed
    /// by game code, and this is the seam that carries the pose into the
    /// solver. Bodies of other kinds ignore their `Transform` as an
    /// input (the solver owns their pose; `Transform` is their output).
    pub(crate) fn drive_kinematic_pose(
        &mut self,
        handle: BodyHandle,
        translation: [f32; 2],
        rotation: f32,
    ) -> bool {
        if translation.iter().any(|c| !c.is_finite()) || !rotation.is_finite() {
            return false;
        }
        match self.live_slot(handle) {
            Some(slot) => {
                let rapier_handle = slot.rapier;
                match self.bodies.get_mut(rapier_handle) {
                    Some(body) if body.is_kinematic() => {
                        body.set_next_kinematic_translation(rapier::math::Vector::new(
                            translation[0],
                            translation[1],
                        ));
                        body.set_next_kinematic_rotation(rapier::math::Rotation::new(rotation));
                        true
                    }
                    _ => false,
                }
            }
            None => false,
        }
    }

    /// Overwrites a live body's gravity scale (the per-body multiplier on
    /// the world's gravity). Returns `false` (applying nothing) for stale
    /// handles or non-finite scales — the same skip semantics as
    /// [`set_velocity`].
    pub(crate) fn set_body_gravity_scale(&mut self, handle: BodyHandle, scale: f32) -> bool {
        if !scale.is_finite() {
            return false;
        }
        match self.live_slot(handle) {
            Some(slot) => {
                let rapier_handle = slot.rapier;
                match self.bodies.get_mut(rapier_handle) {
                    Some(body) => {
                        body.set_gravity_scale(scale, true);
                        true
                    }
                    None => false,
                }
            }
            None => false,
        }
    }

    /// Overwrites a live body's locked axes. Returns `false` (applying
    /// nothing) for stale handles. Applied every tick by the Task 4
    /// system (not just at creation) so editing the component stays live
    /// without tracking — idempotent solver-side, setup-path cost, never
    /// hot-path.
    pub(crate) fn set_body_locked_axes(
        &mut self,
        handle: BodyHandle,
        locks: &crate::LockedAxes,
    ) -> bool {
        let flags = Self::rapier_locked_axes(locks);
        match self.live_slot(handle) {
            Some(slot) => {
                let rapier_handle = slot.rapier;
                match self.bodies.get_mut(rapier_handle) {
                    Some(body) => {
                        body.set_locked_axes(flags, true);
                        true
                    }
                    None => false,
                }
            }
            None => false,
        }
    }
}

impl PhysicsBackend for RapierBackend {
    fn create_body(
        &mut self,
        body: &RigidBody,
        translation: [f32; 2],
        rotation: f32,
    ) -> Result<BodyHandle, PhysicsError> {
        if translation.iter().any(|c| !c.is_finite()) || !rotation.is_finite() {
            return Err(PhysicsError::NonFinitePose {
                translation,
                rotation,
            });
        }
        let rapier_handle = self.bodies.insert(
            Self::builder_for(body.kind)
                .translation(rapier::math::Vector::new(translation[0], translation[1]))
                .rotation(rotation)
                .build(),
        );
        let index = self.next_index;
        self.next_index = self.next_index.wrapping_add(1);
        self.slots.insert(
            index,
            BodySlot {
                rapier: rapier_handle,
                generation: 0,
                alive: true,
                colliders: Vec::new(),
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
        let slot = self.live_slot(body).ok_or(PhysicsError::UnknownBody {
            index: body.index(),
            generation: body.generation(),
        })?;
        let rapier_body = slot.rapier;
        let rapier_collider = Self::collider_builder_for(collider)
            .friction(material.friction)
            .restitution(material.restitution)
            .build();
        // `insert_with_parent` panics on an unknown parent internally,
        // so liveness is proven BEFORE reaching it (the `live_slot`
        // check above) — the panic path is unreachable, not handled.
        let rapier_collider_handle =
            self.colliders
                .insert_with_parent(rapier_collider, rapier_body, &mut self.bodies);
        if let Some(slot) = self.live_slot_mut(body) {
            slot.colliders.push(rapier_collider_handle);
        }
        let index = self.next_collider_index;
        self.next_collider_index = self.next_collider_index.wrapping_add(1);
        Ok(ColliderHandle::from_raw_parts(index, 0))
    }

    fn step(&mut self, dt: f32) -> Result<(), PhysicsError> {
        // Exact equality, not an epsilon band: the trait's determinism
        // contract (see `FIXED_DT`'s docs) requires refusing near-fixed
        // dt loudly rather than degrading reproducibility silently. The
        // accumulator in `systems.rs` owns converting frame time into
        // whole fixed steps; this method owns refusing anything else.
        if dt != FIXED_DT {
            return Err(PhysicsError::VariableTimestep {
                got: dt,
                expected: FIXED_DT,
            });
        }
        let gravity = rapier::math::Vector::new(self.gravity[0], self.gravity[1]);
        self.pipeline.step(
            gravity,
            &self.integration_parameters,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            &(),
            &(),
        );
        // Force hygiene: forces persist until cleared, and this cut has
        // no force-accumulator API (per-frame impulses cover the v0.0.11
        // scenes), so anything lingering in a body's accumulator past the
        // step that set it would be a leak — clear it while the body is
        // at hand. Collected handles first: `reset_forces` needs
        // `&mut RigidBody` while `bodies` is borrowed mutably per body,
        // so the two-phase form keeps the borrow checker honest without
        // holding two mutable paths at once.
        let handles: Vec<rapier::dynamics::RigidBodyHandle> =
            self.bodies.iter().map(|(handle, _)| handle).collect();
        for handle in handles {
            if let Some(body) = self.bodies.get_mut(handle) {
                body.reset_forces(true);
            }
        }
        // Defense in depth (see the module docs): rapier auto-disables
        // bodies/colliders whose state went non-finite mid-step. Destroy
        // each affected body so sync degrades to the stale-handle skip
        // instead of freezing on a silently disabled body — and the Task
        // 4 system re-creates it fresh from its `Transform` next tick.
        // Empty in ordinary play: one `is_empty` check, no scan.
        self.drain_quarantine();
        Ok(())
    }

    fn sync_transform(&self, body: BodyHandle) -> Option<(glam::Vec2, f32)> {
        let slot = self.live_slot(body)?;
        let rapier_body = self.bodies.get(slot.rapier)?;
        let position = rapier_body.translation();
        let angle = rapier_body.rotation().angle();
        // Sync-side NaN guard (the cheap quarantine equivalent — see the
        // module docs): boundary hygiene should make this unreachable,
        // but a non-finite pose must never be written into `Transform`
        // where it would poison rendering and hierarchy propagation. Skip,
        // never poison.
        if !position.x.is_finite() || !position.y.is_finite() || !angle.is_finite() {
            return None;
        }
        Some((glam::Vec2::new(position.x, position.y), angle))
    }

    fn remove_body(&mut self, body: BodyHandle) -> bool {
        let index = body.index();
        match self.slots.get(&index) {
            Some(slot) if slot.alive && slot.generation == body.generation() => {
                self.destroy_slot(index)
            }
            _ => false,
        }
    }

    fn set_velocity(&mut self, body: BodyHandle, velocity: &Velocity) -> bool {
        if !velocity.linvel.iter().all(|c| c.is_finite()) || !velocity.angvel.is_finite() {
            return false;
        }
        match self.live_slot(body) {
            Some(slot) => {
                let rapier_body = slot.rapier;
                match self.bodies.get_mut(rapier_body) {
                    Some(rapier_body) => {
                        rapier_body.set_linvel(
                            rapier::math::Vector::new(velocity.linvel[0], velocity.linvel[1]),
                            true,
                        );
                        rapier_body.set_angvel(velocity.angvel, true);
                        true
                    }
                    None => false,
                }
            }
            None => false,
        }
    }

    fn apply_impulse(&mut self, body: BodyHandle, impulse: [f32; 2], angular_impulse: f32) -> bool {
        if impulse.iter().any(|c| !c.is_finite()) || !angular_impulse.is_finite() {
            return false;
        }
        match self.live_slot(body) {
            Some(slot) => {
                let rapier_body = slot.rapier;
                match self.bodies.get_mut(rapier_body) {
                    Some(rapier_body) => {
                        rapier_body
                            .apply_impulse(rapier::math::Vector::new(impulse[0], impulse[1]), true);
                        rapier_body.apply_torque_impulse(angular_impulse, true);
                        true
                    }
                    None => false,
                }
            }
            None => false,
        }
    }

    fn set_gravity(&mut self, gravity: [f32; 2]) {
        if gravity.iter().all(|c| c.is_finite()) {
            self.gravity = gravity;
        }
    }

    fn gravity(&self) -> [f32; 2] {
        self.gravity
    }

    fn body_count(&self) -> usize {
        self.live
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body falls when stepped: the solver is really integrated, not
    /// stubbed — one fixed step under default gravity moves a dynamic
    /// body down.
    #[test]
    fn dynamic_body_falls_under_gravity_after_fixed_steps() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("finite pose must create");
        backend
            .attach_collider(body, &Collider::ball(0.5), &ColliderMaterial::default())
            .expect("valid collider must attach");

        let start_y = backend
            .sync_transform(body)
            .expect("live body must sync")
            .0
            .y;
        for _ in 0..60 {
            backend.step(FIXED_DT).expect("fixed step must succeed");
        }
        let end_y = backend
            .sync_transform(body)
            .expect("live body must sync")
            .0
            .y;

        assert!(
            end_y < start_y - 1.0,
            "one second of free fall must drop more than a meter: {start_y} -> {end_y}"
        );
    }

    /// A fixed body never moves no matter how long the simulation runs.
    #[test]
    fn fixed_body_holds_its_pose_across_many_steps() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let body = backend
            .create_body(&RigidBody::fixed(), [3.0, 7.0], 0.4)
            .expect("finite pose must create");

        for _ in 0..120 {
            backend.step(FIXED_DT).expect("fixed step must succeed");
        }

        let (position, angle) = backend.sync_transform(body).expect("live body must sync");
        assert!((position.x - 3.0).abs() < 1e-5);
        assert!((position.y - 7.0).abs() < 1e-5);
        assert!((angle - 0.4).abs() < 1e-4);
    }

    /// Stepping twice with the same inputs produces bit-identical poses:
    /// determinism scoped to repeatability on this machine (same binary,
    /// same thread), NOT cross-platform — see `systems.rs`'s determinism
    /// docs for why the claim stops there.
    #[test]
    fn same_steps_produce_bit_identical_poses() {
        fn trajectory() -> Vec<(f32, f32, f32)> {
            let mut backend = RapierBackend::new([0.0, -9.81]);
            let falling = backend
                .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.2)
                .expect("finite pose must create");
            backend
                .attach_collider(
                    falling,
                    &Collider::cuboid([0.5, 0.5]),
                    &ColliderMaterial::default(),
                )
                .expect("valid collider must attach");
            let ground = backend
                .create_body(&RigidBody::fixed(), [0.0, -1.0], 0.0)
                .expect("finite pose must create");
            backend
                .attach_collider(
                    ground,
                    &Collider::cuboid([10.0, 0.5]),
                    &ColliderMaterial::default(),
                )
                .expect("valid collider must attach");
            let mut poses = Vec::with_capacity(60);
            for _ in 0..60 {
                backend.step(FIXED_DT).expect("fixed step must succeed");
                let (position, angle) = backend
                    .sync_transform(falling)
                    .expect("live body must sync");
                poses.push((position.x, position.y, angle));
            }
            poses
        }

        let first = trajectory();
        let second = trajectory();

        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert!(
                a.0.to_bits() == b.0.to_bits()
                    && a.1.to_bits() == b.1.to_bits()
                    && a.2.to_bits() == b.2.to_bits(),
                "poses must be bit-identical: {a:?} vs {b:?}"
            );
        }
    }

    /// Stale handles skip everywhere and never panic: remove, then every
    /// handle-targeted method reports absence; stepping with no live
    /// bodies still succeeds.
    #[test]
    fn stale_handles_skip_everywhere_and_never_panic() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("finite pose must create");
        assert!(backend.remove_body(body));

        assert!(backend.sync_transform(body).is_none());
        assert!(!backend.set_velocity(body, &Velocity::zero()));
        assert!(!backend.apply_impulse(body, [1.0, 0.0], 0.0));
        assert!(!backend.remove_body(body));
        assert_eq!(backend.body_count(), 0);
        backend
            .step(FIXED_DT)
            .expect("step with no bodies must succeed");
    }

    /// Forged handles (never issued) resolve to nothing, never to a live
    /// body — the liveness check, not unguessability, is the mechanism.
    #[test]
    fn forged_handles_never_resolve_to_live_bodies() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let live = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("finite pose must create");
        let forged = BodyHandle::from_raw_parts(live.index().wrapping_add(1000), 0);

        assert!(backend.sync_transform(forged).is_none());
        assert!(!backend.set_velocity(forged, &Velocity::zero()));
        assert!(!backend.remove_body(forged));
        assert!(backend.sync_transform(live).is_some());
    }

    /// Entity tracking pairs creation with bookkeeping: tracked while
    /// live, forgotten on untrack, and a stale tracked handle reads as
    /// absent (the system's despawn-cleanup path depends on this).
    #[test]
    fn entity_tracking_reports_live_and_forgets_on_untrack() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let entity = Entity::from_raw_parts(7, 0);
        assert_eq!(backend.entity_handle(entity), None);

        let body = backend
            .create_body(&RigidBody::dynamic(), [1.0, 2.0], 0.0)
            .expect("finite pose must create");
        backend.track_entity(entity, body);
        assert_eq!(backend.entity_handle(entity), Some(body));

        assert_eq!(backend.untrack_entity(entity), Some(body));
        assert_eq!(backend.entity_handle(entity), None);
        // The body itself is untouched by untracking: tracking removal
        // and body destruction stay paired at the call site.
        assert!(backend.sync_transform(body).is_some());
    }

    /// A tracked handle whose body was destroyed reads as absent, so the
    /// system re-creates instead of driving a dead body.
    #[test]
    fn tracked_handle_of_a_removed_body_reads_as_absent() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let entity = Entity::from_raw_parts(3, 0);
        let body = backend
            .create_body(&RigidBody::dynamic(), [1.0, 2.0], 0.0)
            .expect("finite pose must create");
        backend.track_entity(entity, body);
        assert!(backend.remove_body(body));

        assert_eq!(backend.entity_handle(entity), None);
    }

    /// Backend parity with the trait contract: non-finite poses fail
    /// typed (creation has no previous state worth retaining), and any
    /// non-fixed dt — including zero — fails as a usage error.
    #[test]
    fn rapier_rejects_nonfinite_pose_and_nonfixed_dt_typed() {
        let mut backend = RapierBackend::new([0.0, -9.81]);

        assert!(matches!(
            backend.create_body(&RigidBody::dynamic(), [f32::NAN, 0.0], 0.0),
            Err(PhysicsError::NonFinitePose { .. })
        ));
        assert!(matches!(
            backend.create_body(&RigidBody::dynamic(), [0.0, 0.0], f32::INFINITY),
            Err(PhysicsError::NonFinitePose { .. })
        ));

        for bad_dt in [0.0, 1.0 / 30.0, 1.0 / 60.0 + 1e-6] {
            let error = backend
                .step(bad_dt)
                .expect_err("only exactly FIXED_DT may step");
            assert_eq!(
                error,
                PhysicsError::VariableTimestep {
                    got: bad_dt,
                    expected: FIXED_DT,
                },
                "dt={bad_dt}"
            );
        }
        assert!(matches!(
            backend.step(f32::NAN),
            Err(PhysicsError::VariableTimestep { .. })
        ));
    }

    /// Backend parity: construction with non-finite gravity degrades to
    /// zero gravity (documented in `RapierBackend::new`), while
    /// `set_gravity` with non-finite input retains the previous value.
    #[test]
    fn rapier_gravity_hygiene_falls_back_on_build_and_retains_on_set() {
        let backend = RapierBackend::new([0.0, f32::NAN]);
        assert_eq!(backend.gravity(), [0.0, 0.0]);

        let mut backend = RapierBackend::new([0.0, -9.81]);
        backend.set_gravity([f32::INFINITY, 0.0]);
        assert_eq!(backend.gravity(), [0.0, -9.81]);
        backend.set_gravity([0.0, -1.62]);
        assert_eq!(backend.gravity(), [0.0, -1.62]);
    }

    /// Backend parity: attach validates shape AND material before
    /// touching solver state, fails typed on a stale body, and mints a
    /// distinct collider handle per attach.
    #[test]
    fn rapier_attach_validates_material_and_body_liveness() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("finite pose must create");

        assert_eq!(
            backend
                .attach_collider(
                    body,
                    &Collider::ball(0.5),
                    &ColliderMaterial::new(-0.1, 0.0),
                )
                .expect_err("negative friction must fail"),
            PhysicsError::InvalidFriction { friction: -0.1 }
        );
        assert_eq!(
            backend
                .attach_collider(body, &Collider::ball(0.5), &ColliderMaterial::new(0.5, 1.5),)
                .expect_err("restitution above one must fail"),
            PhysicsError::InvalidRestitution { restitution: 1.5 }
        );

        let first = backend
            .attach_collider(body, &Collider::ball(0.5), &ColliderMaterial::default())
            .expect("valid attach must succeed");
        let second = backend
            .attach_collider(body, &Collider::ball(0.5), &ColliderMaterial::default())
            .expect("valid attach must succeed");
        assert_ne!(first, second, "each attach mints a distinct handle");

        assert!(backend.remove_body(body));
        assert_eq!(
            backend
                .attach_collider(body, &Collider::ball(0.5), &ColliderMaterial::default())
                .expect_err("attach to a removed body must fail"),
            PhysicsError::UnknownBody {
                index: body.index(),
                generation: body.generation(),
            }
        );
    }

    /// Kinematic driving routes only to position-kinematic bodies: a
    /// dynamic body must never be teleported by the frame loop (it would
    /// fight integration), and stale/non-finite inputs drive nothing.
    #[test]
    fn kinematic_drive_reaches_only_position_kinematic_bodies() {
        let mut backend = RapierBackend::new([0.0, 0.0]);
        let platform = backend
            .create_body(&RigidBody::kinematic_position(), [0.0, 0.0], 0.0)
            .expect("finite pose must create");
        let falling = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("finite pose must create");

        assert!(backend.drive_kinematic_pose(platform, [3.0, 1.0], 0.0));
        assert!(!backend.drive_kinematic_pose(falling, [3.0, 1.0], 0.0));
        assert!(!backend.drive_kinematic_pose(platform, [f32::NAN, 1.0], 0.0));
        assert!(!backend.drive_kinematic_pose(
            BodyHandle::from_raw_parts(9999, 0),
            [1.0, 1.0],
            0.0
        ));

        backend.step(FIXED_DT).expect("fixed step must succeed");
        let (position, _) = backend
            .sync_transform(platform)
            .expect("live body must sync");
        assert!(
            (position.x - 3.0).abs() < 1e-4 && (position.y - 1.0).abs() < 1e-4,
            "the driven platform must reach its scripted pose: {position:?}"
        );
    }

    /// Per-body gravity scale and axis locks apply to live bodies and
    /// skip stale handles and non-finite scales without touching solver
    /// state.
    #[test]
    fn gravity_scale_and_locks_apply_live_and_skip_stale() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("finite pose must create");
        let stale = BodyHandle::from_raw_parts(body.index(), body.generation() + 1);

        assert!(backend.set_body_gravity_scale(body, 0.0));
        assert!(!backend.set_body_gravity_scale(body, f32::NAN));
        assert!(!backend.set_body_gravity_scale(stale, 1.0));
        assert!(backend.set_body_locked_axes(body, &crate::LockedAxes::rotation_locked()));
        assert!(!backend.set_body_locked_axes(stale, &crate::LockedAxes::rotation_locked()));
    }

    /// Soak: fifty bodies stepped for four simulated seconds stay live,
    /// countable, and finite — no panic, no leak, no NaN poisoning.
    #[test]
    fn fifty_bodies_survive_four_simulated_seconds_finite() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let mut bodies = Vec::with_capacity(50);
        for i in 0..50 {
            let body = backend
                .create_body(
                    &RigidBody::dynamic(),
                    [(i as f32 % 10.0) - 5.0, 5.0 + (i as f32 / 10.0)],
                    0.0,
                )
                .expect("finite pose must create");
            backend
                .attach_collider(body, &Collider::ball(0.3), &ColliderMaterial::default())
                .expect("valid collider must attach");
            bodies.push(body);
        }

        for _ in 0..240 {
            backend.step(FIXED_DT).expect("fixed step must succeed");
        }

        assert_eq!(backend.body_count(), 50);
        for body in bodies {
            let (position, angle) = backend
                .sync_transform(body)
                .expect("every soaked body must still sync");
            assert!(
                position.x.is_finite() && position.y.is_finite() && angle.is_finite(),
                "soaked poses must stay finite: {position:?} {angle}"
            );
        }
    }

    /// Huge-but-finite inputs never poison sync and never panic: a body
    /// driven at `f32::MAX` velocity steps cleanly (the solver clamps
    /// finite extremes internally — observed: near-ordinary fall), and
    /// whatever the solver does with the extreme, every sync stays
    /// finite-or-absent while the drain count plus body count stay
    /// mutually consistent (drained bodies read as removed, never linger).
    #[test]
    fn huge_finite_velocity_never_poisons_sync_or_panics() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("finite pose must create");
        backend
            .attach_collider(body, &Collider::ball(0.3), &ColliderMaterial::default())
            .expect("valid collider must attach");
        assert!(backend.set_velocity(
            body,
            &Velocity {
                linvel: [f32::MAX, 0.0],
                angvel: 0.0,
            }
        ));

        for _ in 0..120 {
            backend
                .step(FIXED_DT)
                .expect("step must succeed even as state overflows");
            if let Some((position, angle)) = backend.sync_transform(body) {
                assert!(
                    position.x.is_finite() && position.y.is_finite() && angle.is_finite(),
                    "sync must stay finite while the body is live"
                );
            }
        }

        let drained = backend.quarantine_drain_count();
        if drained > 0 {
            assert_eq!(
                backend.body_count(),
                0,
                "a drained body must read as removed, not linger"
            );
            assert!(
                backend.sync_transform(body).is_none(),
                "the drained handle must stay stale"
            );
        } else {
            assert_eq!(backend.body_count(), 1);
        }
    }

    /// Chaos: five hundred bodies stepped for two simulated seconds,
    /// then a removal storm destroying every other body mid-run, then
    /// more stepping — no panic, exact counts, every survivor finite.
    /// Exercises the shared `destroy_slot` path at volume (bulk removal
    /// plus quarantine-drain removal must agree on slot state).
    #[test]
    fn five_hundred_body_soak_with_removal_storm_stays_exact() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let mut bodies = Vec::with_capacity(500);
        for i in 0..500 {
            let body = backend
                .create_body(
                    &RigidBody::dynamic(),
                    [(i % 50) as f32 - 25.0, 5.0 + (i / 50) as f32],
                    0.0,
                )
                .expect("finite pose must create");
            backend
                .attach_collider(body, &Collider::ball(0.3), &ColliderMaterial::default())
                .expect("valid collider must attach");
            bodies.push(body);
        }
        for _ in 0..120 {
            backend.step(FIXED_DT).expect("fixed step must succeed");
        }
        assert_eq!(backend.body_count(), 500);

        // Removal storm: destroy every other body, including double-remove
        // attempts (despawn races are routine, not errors).
        for (i, body) in bodies.iter().enumerate() {
            if i % 2 == 0 {
                assert!(backend.remove_body(*body), "live body {i} must remove");
                assert!(!backend.remove_body(*body), "double-remove {i} must skip");
            }
        }
        assert_eq!(backend.body_count(), 250);

        for _ in 0..120 {
            backend
                .step(FIXED_DT)
                .expect("post-storm steps must succeed");
        }

        assert_eq!(backend.body_count(), 250);
        for (i, body) in bodies.iter().enumerate() {
            if i % 2 == 0 {
                assert!(
                    backend.sync_transform(*body).is_none(),
                    "storm-removed {i} must stay stale"
                );
            } else if let Some((position, angle)) = backend.sync_transform(*body) {
                assert!(
                    position.x.is_finite() && position.y.is_finite() && angle.is_finite(),
                    "survivor {i} must stay finite"
                );
            }
        }
    }

    /// Fuzz: extreme-but-valid collider params (max-magnitude finite
    /// extents) attach and step without panic; degenerate params fail
    /// typed without touching solver state. Arbitrary game content must
    /// produce typed errors or clamped simulation — never a panic.
    #[test]
    fn extreme_collider_params_never_panic_and_stay_consistent() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let body = backend
            .create_body(&RigidBody::dynamic(), [0.0, 5.0], 0.0)
            .expect("finite pose must create");
        // Max-magnitude finite extents are VALID params (finite and
        // positive): attach must succeed, and stepping with them must
        // neither panic nor poison neighbours.
        for collider in [
            Collider::ball(f32::MAX),
            Collider::cuboid([f32::MAX, f32::MAX]),
            Collider::capsule(f32::MAX, f32::MAX),
        ] {
            assert!(collider.validate().is_ok());
            backend
                .attach_collider(body, &collider, &ColliderMaterial::default())
                .expect("finite-positive params must attach");
        }
        let neighbour = backend
            .create_body(&RigidBody::dynamic(), [10.0, 5.0], 0.0)
            .expect("finite pose must create");
        backend
            .attach_collider(
                neighbour,
                &Collider::ball(0.3),
                &ColliderMaterial::default(),
            )
            .expect("valid collider must attach");
        for _ in 0..60 {
            backend.step(FIXED_DT).expect("step must succeed");
        }
        if let Some((position, angle)) = backend.sync_transform(neighbour) {
            assert!(
                position.x.is_finite() && position.y.is_finite() && angle.is_finite(),
                "the neighbour must stay finite beside extreme colliders"
            );
        }
        // Degenerate params fail typed, before solver state is touched.
        for collider in [
            Collider::ball(0.0),
            Collider::ball(f32::NAN),
            Collider::cuboid([1.0, f32::INFINITY]),
            Collider::capsule(f32::NEG_INFINITY, 0.5),
        ] {
            assert!(
                collider.validate().is_err(),
                "degenerate params must fail typed: {collider:?}"
            );
            assert!(
                backend
                    .attach_collider(body, &collider, &ColliderMaterial::default())
                    .is_err(),
                "degenerate attach must fail typed: {collider:?}"
            );
        }
    }

    /// Removing one body mid-soak leaves its neighbours live and synced:
    /// despawn-racing-step degrades to a skip, never cross-talk.
    #[test]
    fn mid_soak_removal_leaves_neighbours_live() {
        let mut backend = RapierBackend::new([0.0, -9.81]);
        let mut bodies = Vec::with_capacity(8);
        for i in 0..8 {
            let body = backend
                .create_body(&RigidBody::dynamic(), [i as f32 - 4.0, 5.0], 0.0)
                .expect("finite pose must create");
            backend
                .attach_collider(body, &Collider::ball(0.3), &ColliderMaterial::default())
                .expect("valid collider must attach");
            bodies.push(body);
        }
        for _ in 0..30 {
            backend.step(FIXED_DT).expect("fixed step must succeed");
        }

        let removed = bodies[3];
        assert!(backend.remove_body(removed));
        for _ in 0..30 {
            backend.step(FIXED_DT).expect("fixed step must succeed");
        }

        assert_eq!(backend.body_count(), 7);
        assert!(backend.sync_transform(removed).is_none());
        for (i, body) in bodies.iter().enumerate() {
            if i == 3 {
                continue;
            }
            assert!(
                backend.sync_transform(*body).is_some(),
                "neighbour {i} must stay live"
            );
        }
    }
}
