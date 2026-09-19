// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Fixed-step physics scheduling: [`PhysicsClock`], [`SimulationTime`],
//! [`FrameDelta`], [`physics_step_system`], and its scheduler wiring.
//!
//! # Mechanism (read this before touching the registration order)
//!
//! A variable frame delta must never reach the solver: Rapier integrates
//! with `dt`-scaled forces, so stepping it with measured frame time makes
//! identical inputs diverge with the frame rate — the exact
//! reproducibility loss `docs/architecture/physics.md` forbids (it would
//! void multiplayer prediction and record/replay). The subsystem therefore
//! self-steps through an accumulator, with NO `App` redesign:
//!
//! 1. The owning subsystem inserts [`FrameDelta`] (the measured frame
//!    time) as a resource before `Schedule::run` each tick — see
//!    `engine/canary-runtime/src/main.rs`. The physics system is an
//!    ordinary scheduled system, not a privileged `App` hook.
//! 2. [`physics_step_system`] adds the frame time to
//!    [`PhysicsClock`]'s accumulator and quantizes it into whole
//!    [`FIXED_DT`] steps, clamped to
//!    [`MAX_STEPS_PER_TICK`] with the spiral guard (see
//!    `consume_accumulator()`).
//! 3. Each fixed step advances the solver via
//!    [`PhysicsBackend::step`] at exactly
//!    [`FIXED_DT`] — the backend refuses anything else
//!    by exact equality — and advances [`SimulationTime`] by the same
//!    quantum. `SimulationTime` is deliberately DISTINCT from ECS `Tick`:
//!    ticks count scheduler passes (including zero-physics ticks),
//!    simulation time counts integrated physics seconds. A paused game
//!    (zero steps per tick) advances ticks without advancing simulation
//!    time; a hitch tick (four steps) advances simulation time by four
//!    quanta in one tick. Conflating the two would make "how much physics
//!    happened" unanswerable from the tick count.
//! 4. After stepping, every live body syncs its 2D pose back into its
//!    entity's [`Transform`] — x/y plus the
//!    z-axis rotation ONLY, preserving `translation.z`, off-axis
//!    rotation, and scale (see `apply_synced_pose()`).
//!
//! # Why first position (the ordering law)
//!
//! Physics WRITES `Transform`; transform propagation READS `Transform`
//! (and writes `GlobalTransform`); the soup/mesh/textured bakes READ
//! `GlobalTransform`. Registration order is therefore load-bearing:
//!
//! ```text
//! physics-step → propagation → soup → mesh → textured
//! ```
//!
//! [`Schedule`] runs every write system alone
//! in its own stage, in registration order (solo-write staging — see
//! [`physics_step_access`]). Physics registered first means its stage
//! runs first, so propagation recomputes globals from the just-stepped
//! locals and the bakes snapshot fresh globals. Registering physics after
//! propagation would bake one-tick-stale globals every tick: the body
//! visibly lags its simulation by a frame, and the reversed-registration
//! test below proves it (mirroring the `canary-render-ecs` precedent,
//! where the same proof shape pins bake-after-propagation).
//!
//! # Solo-write staging (how the declaration enforces the order)
//!
//! [`physics_step_access`] declares `writes::<Transform>()` (plus the
//! `PhysicsClock`/`SimulationTime`/`RapierBackend` resource writes).
//! Propagation declares `reads::<Transform>, writes::<GlobalTransform>`.
//! A write followed by a read of the same component conflicts by
//! [`SystemAccess`] rules, so the two
//! systems can never share a stage: physics owns an earlier solo stage,
//! propagation a later one. Had physics declared only reads, it could
//! share a read stage and run after (or concurrently with) propagation —
//! the declaration IS the ordering mechanism's second half (registration
//! order is the first).
//!
//! # Spiral policy (what happens on a hitch)
//!
//! When a frame's accumulated time needs more than [`MAX_STEPS_PER_TICK`]
//! steps (debugger breakpoint, tab-switch, first-frame spike), the system
//! simulates exactly [`MAX_STEPS_PER_TICK`] steps and DROPS the entire
//! leftover — including the sub-step remainder, resetting the accumulator
//! to zero. Dropping (rather than carrying the debt) bounds the worst
//! tick cost at four steps: carrying it would let one hitch compound
//! into a permanently behind simulation that never recovers (the spiral
//! of death), each tick doing max work and still falling further behind.
//! The cost is honest and documented: simulation time runs slower than
//! wall clock until frames recover. Slow-motion-by-design would be a
//! different policy (debt carried, tick cost unbounded); this cut chooses
//! bounded ticks.
//!
//! # Preservation rule (what sync may touch)
//!
//! The 2D game lives on a z-pinned plane inside an always-3D
//! [`Transform`] (ADR 0017): the backend
//! only knows x/y plus one rotation angle, so sync writes exactly those
//! and preserves everything else — `translation.z` (the plane depth),
//! the off-axis (x/y) rotation component, and `scale`. The rotation merge
//! is a twist/swing recomposition (see `replace_z_twist()`): the
//! off-axis "swing" is preserved exactly, only the z "twist" is
//! replaced. A backend writing the full quaternion would clobber the
//! plane depth and sprite scale every step; a backend storing its own
//! copy of z/scale would double the pose state and drift from it. The
//! preservation test below pins all three.
//!
//! # Determinism scope (what is and is NOT claimed)
//!
//! Same steps → bit-identical trajectory, SINGLE machine (same binary,
//! same thread). This is repeatability for record/replay and regression
//! tests, NOT cross-platform determinism (float association, SIMD
//! codegen, and OS math libraries differ across targets). The crate
//! enables no rapier `parallel` feature — rayon work-stealing would trade
//! this property for throughput the v0.0.11 body counts do not need —
//! and no `enhanced-determinism` (which targets cross-platform libm
//! parity, a claim this task explicitly does not make).

use canary_ecs::{Entity, World};
use canary_scheduler::{Schedule, SystemAccess};
use canary_transform::{Parent, Transform};

use crate::{
    Collider, ColliderMaterial, GravityScale, LockedAxes, PhysicsBackend, PhysicsConfig,
    RapierBackend, RigidBody, RigidBodyKind, Velocity, FIXED_DT,
};

/// Maximum fixed steps simulated in one tick.
///
/// Four at 60 Hz covers a 66 ms hitch frame (four full quanta) before
/// the spiral guard engages. Larger caps raise the worst-tick cost
/// linearly for scenes that are already behind; smaller caps drop time
/// on ordinary 30 Hz displays (two steps per frame at 30 Hz leaves
/// headroom, but a single 40 ms spike would already shed). Four is the
/// bevy_rapier-style middle the work plan names.
pub const MAX_STEPS_PER_TICK: usize = 4;

/// The measured frame time, inserted as a resource by the owning
/// subsystem before every [`Schedule::run`].
///
/// This is how variable frame time reaches the fixed-step accumulator
/// WITHOUT an `App` redesign: the subsystem's `tick(dt)` already
/// receives the frame duration, and inserting it as a resource turns
/// "the scheduler needs the frame dt" into ordinary resource flow. No
/// `App`-level scheduler, no new `Subsystem` method, no signature
/// changes in `canary-core` — the plan's "subsystem self-stepping"
/// decision, exactly.
///
/// Absent [`FrameDelta`] (a test that registers the system without a
/// subsystem around it) reads as exactly one [`FIXED_DT`]:
/// the least surprising degradation, and deterministic under test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameDelta {
    /// The measured wall-clock time since the previous tick.
    pub dt: std::time::Duration,
}

impl FrameDelta {
    /// Builds a frame delta from an explicit duration.
    pub fn new(dt: std::time::Duration) -> Self {
        Self { dt }
    }

    /// The frame time in seconds (the unit the accumulator reasons in).
    /// `Duration` cannot be negative or NaN, so this is total — the
    /// system additionally clamps absurd values by stepping at most
    /// [`MAX_STEPS_PER_TICK`] and dropping the rest.
    pub fn seconds(&self) -> f32 {
        self.dt.as_secs_f32()
    }
}

impl Default for FrameDelta {
    /// Exactly one fixed step's worth of frame time.
    fn default() -> Self {
        Self {
            dt: std::time::Duration::from_secs_f32(FIXED_DT),
        }
    }
}

/// The fixed-step accumulator: turns variable frame time into whole
/// [`FIXED_DT`] steps, clamped by the spiral guard.
///
/// `fixed_dt` names the timestep the accumulator quantizes to and
/// mirrors [`FIXED_DT`]. It is a field (not a const
/// reference) so a future timestep does not move API — but TODAY the
/// system steps [`FIXED_DT`] unconditionally, because
/// the backend refuses any other `dt` by exact equality. Mutating this
/// field does not change the timestep; the trait's exact-equality law
/// wins over the resource's value, and this is stated here so no caller
/// discovers it by surprise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsClock {
    /// Unsimulated time carried across ticks (seconds, always `>= 0.0`).
    pub accumulator: f32,
    /// The timestep this clock quantizes to. See the type-level docs:
    /// informational today, always [`FIXED_DT`].
    pub fixed_dt: f32,
}

impl PhysicsClock {
    /// An empty clock quantizing to [`FIXED_DT`].
    pub fn new() -> Self {
        Self {
            accumulator: 0.0,
            fixed_dt: FIXED_DT,
        }
    }
}

impl Default for PhysicsClock {
    /// Same as [`PhysicsClock::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// Integrated simulation time in seconds: advanced by exactly
/// [`FIXED_DT`] per completed fixed step, and ONLY per
/// completed step — zero steps (a sub-quantum frame, a paused world)
/// leaves it untouched.
///
/// Distinct from ECS `Tick` by design (see the module docs): ticks count
/// scheduler passes, this counts simulated physics seconds. Gameplay
/// that needs "how long has the world been simulating" (replay clocks,
/// timed kinematic scripts) reads this; gameplay that needs "how many
/// frames ran" reads the tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SimulationTime {
    /// Simulated seconds accumulated so far.
    pub seconds: f32,
}

impl SimulationTime {
    /// Advances the clock by one fixed step. Called by
    /// [`physics_step_system`] only — game code reads, never writes.
    pub fn advance_step(&mut self) {
        self.seconds += FIXED_DT;
    }
}

/// Converts accumulated time into a step count, enforcing the spiral
/// guard. Adds `frame_dt` (clamped at zero: negative or non-finite input
/// simulates nothing rather than rewinding), takes as many whole
/// [`FIXED_DT`] quanta as fit, and:
///
/// - fits within [`MAX_STEPS_PER_TICK`]: subtracts the consumed time,
///   keeping the sub-step remainder for the next tick;
/// - exceeds it: returns [`MAX_STEPS_PER_TICK`] and resets the
///   accumulator to ZERO, dropping the entire leftover (see the module
///   docs' spiral policy — bounded ticks over fair catch-up).
///
/// Pure in everything but the accumulator write, so the accumulator
/// table test below exercises the policy without a solver.
fn consume_accumulator(accumulator: &mut f32, frame_dt: f32) -> usize {
    let frame_dt = if frame_dt.is_finite() && frame_dt > 0.0 {
        frame_dt
    } else {
        0.0
    };
    *accumulator += frame_dt;
    let steps = (*accumulator / FIXED_DT).floor() as usize;
    if steps > MAX_STEPS_PER_TICK {
        *accumulator = 0.0;
        MAX_STEPS_PER_TICK
    } else {
        *accumulator = (*accumulator - steps as f32 * FIXED_DT).max(0.0);
        steps
    }
}

/// The z-axis angle (radians) carried by a quaternion: the twist around
/// z, ignoring any off-axis swing. Used to seed body creation (and
/// kinematic driving) from an existing `Transform` — the solver's single
/// rotation degree of freedom, read out of the 3D representation.
fn z_twist_angle(rotation: glam::Quat) -> f32 {
    let components = rotation.normalize().to_array();
    if components[2] == 0.0 && components[3] == 0.0 {
        return 0.0;
    }
    2.0 * components[2].atan2(components[3])
}

/// Replaces the z-twist of `current` with `angle`, preserving the
/// off-axis swing EXACTLY.
///
/// Decomposes `current` into `twist * swing` (twist around z, swing the
/// off-axis remainder) and returns `Quat::from_rotation_z(angle) *
/// swing`. When the existing twist is degenerate (a ~180° off-axis
/// rotation leaves the `(z, w)` projection near zero, so no meaningful
/// swing can be extracted), falls back to a pure z-rotation — the honest
/// degradation, since inventing a swing would be worse than dropping one
/// no 2D scene should have.
///
/// Why not euler angles: XYZ-euler decomposition/rebuild re-parameterizes
/// the rotation (composition-order dependent, gimbal-adjacent), while
/// twist/swing keeps the off-axis component bit-stable: `swing(result)
/// == swing(input)` up to float rounding, which is exactly what the
/// preservation test asserts.
fn replace_z_twist(current: glam::Quat, angle: f32) -> glam::Quat {
    let normalized = current.normalize();
    let components = normalized.to_array();
    let twist = glam::Quat::from_xyzw(0.0, 0.0, components[2], components[3]);
    let swing = if twist.length_squared() < 1e-12 {
        glam::Quat::IDENTITY
    } else {
        twist.normalize().inverse() * normalized
    };
    glam::Quat::from_rotation_z(angle) * swing
}

/// Writes a synced 2D pose into a 3D [`Transform`]:
/// `translation.x/y` plus the z-axis rotation ONLY, preserving
/// `translation.z` (the z-pinned plane depth), the off-axis rotation
/// swing, and `scale` — the preservation rule (see the module docs).
fn apply_synced_pose(transform: &mut Transform, position: glam::Vec2, angle: f32) {
    transform.translation.x = position.x;
    transform.translation.y = position.y;
    transform.rotation = replace_z_twist(transform.rotation, angle);
}

/// Declares [`physics_step_system`]'s data access: reads bodies and
/// their inputs, writes poses and clocks.
///
/// Every clause earns its place in the ordering mechanism (registration
/// order + solo-write staging — see the module docs):
///
/// - `writes::<Transform>()` (with a `reads::<Transform>()` for the
///   kinematic-drive/preservation reads) conflicts with propagation's
///   `reads::<Transform>`, so physics can never share a stage with
///   propagation and — registered first — always runs before it. The
///   read+write pair on one system is honest: the system genuinely reads
///   current poses (kinematic driving, z/scale preservation) and writes
///   stepped ones.
/// - `reads::<Parent>()` documents the hierarchy skip (entities WITH a
///   parent are render-only children the solver never sees — risk R-9:
///   a child body driven by both propagation and physics would fight
///   itself every tick).
/// - `writes_resource::<PhysicsClock>()` /
///   `writes_resource::<SimulationTime>()` /
///   `writes_resource::<RapierBackend>()` name the mutated resources;
///   `reads_resource::<PhysicsConfig>()` /
///   `reads_resource::<FrameDelta>()` the consumed ones. The backend as
///   a resource (rather than a system-captured singleton) keeps the
///   system signature at `fn(&mut World)` — the scheduler's doctrine —
///   and lets tests replace or inspect the backend through the world.
pub fn physics_step_access() -> SystemAccess {
    SystemAccess::new()
        .reads::<RigidBody>()
        .reads::<Transform>()
        .writes::<Transform>()
        .reads::<Parent>()
        .reads::<Collider>()
        .reads::<ColliderMaterial>()
        .reads::<Velocity>()
        .reads::<GravityScale>()
        .reads::<LockedAxes>()
        .reads_resource::<PhysicsConfig>()
        .reads_resource::<FrameDelta>()
        .writes_resource::<PhysicsClock>()
        .writes_resource::<SimulationTime>()
        .writes_resource::<RapierBackend>()
}

/// Owned per-entity snapshot: everything the step needs from ECS,
/// collected through shared queries BEFORE the backend borrow begins.
/// `World::resource_mut::<RapierBackend>()` holds `&mut World`, which
/// forbids concurrent queries — so the system works in strict phases
/// (snapshot → mutate backend → write poses), never interleaved. All
/// component types here are `Copy`, so the snapshot is plain data with
/// no lifetimes.
struct BodyRecord {
    /// The entity owning this body.
    entity: Entity,
    /// The solver role (drives creation mapping + kinematic routing).
    body: RigidBody,
    /// Current local pose: creation seed, kinematic-drive source, and
    /// preservation baseline.
    transform: Transform,
    /// Attached shape, if the entity carries one. `None` means a bare
    /// body (valid: sensors-by-convention and not-yet-shaped spawns step
    /// fine without colliders).
    collider: Option<Collider>,
    /// Surface response for the collider; defaults at attach time when
    /// the entity carries none.
    material: Option<ColliderMaterial>,
    /// Seeded at creation and re-applied every tick the component is
    /// present: the game drives motion through this component, never by
    /// integrating poses itself (which would fight the fixed-step
    /// solver).
    velocity: Option<Velocity>,
    /// Re-applied every tick when present.
    gravity_scale: Option<GravityScale>,
    /// Re-applied every tick when present (idempotent solver-side).
    locks: Option<LockedAxes>,
}

/// Steps the physics world once per tick: discover → drive → accumulate
/// → step → sync.
///
/// Phase order and why:
///
/// 1. **Ensure resources.** Missing `PhysicsConfig`/`PhysicsClock`/
///    `SimulationTime`/`RapierBackend` are inserted from defaults (never
///    overwriting): a test registering only the system gets a working
///    world, and the subsystem constructor does not own physics
///    defaults. Backend gravity seeds from the config.
/// 2. **Snapshot** every root entity carrying `RigidBody` + `Transform`
///    (shared borrows only). Entities WITH a [`Parent`] are skipped —
///    render-only children (risk R-9).
/// 3. **Reap** tracked entities that died or lost their body: untrack +
///    destroy, so despawn-racing-step degrades to a skip, never a panic.
/// 4. **Create** untracked bodies (seed pose/velocity/scale/locks from
///    the snapshot; attach collider with default material when the
///    entity carries none; skip creation on invalid input — a
///    non-finite `Transform` or degenerate collider is caller error the
///    trait reports, and the system answers by not simulating that
///    entity rather than by failing the tick).
/// 5. **Drive** per-tick inputs: `Velocity` → `set_velocity`,
///    `GravityScale` → scale, `LockedAxes` → locks, and
///    `KinematicPosition` bodies ← current `Transform` (the ONLY bodies
///    whose `Transform` is an input; dynamic/fixed bodies' `Transform`
///    is output-only — writing solver poses from game edits each tick
///    would teleport them through contacts).
/// 6. **Accumulate** frame time into whole steps (spiral-guarded),
///    **step** the backend that many times at exactly
///    [`FIXED_DT`], advancing [`SimulationTime`] per
///    completed step.
/// 7. **Sync** each live body's 2D pose into its `Transform` via
///    `apply_synced_pose()` (stale handles skip — the reap in phase 3
///    plus generational liveness make "live at snapshot, dead at sync"
///    a skip, not a panic).
///
/// Config gravity is forwarded to the backend every tick (before
/// stepping), so tuning gravity is data — set the resource once, or
/// hot-tweak it live — never a backend reconstruction.
pub fn physics_step_system(world: &mut World) {
    let frame_dt = world
        .resource::<FrameDelta>()
        .map_or(FIXED_DT, FrameDelta::seconds);

    if world.resource::<PhysicsConfig>().is_none() {
        world.insert_resource(PhysicsConfig::default());
    }
    if world.resource::<PhysicsClock>().is_none() {
        world.insert_resource(PhysicsClock::default());
    }
    if world.resource::<SimulationTime>().is_none() {
        world.insert_resource(SimulationTime::default());
    }
    let gravity: [f32; 2] = world
        .resource::<PhysicsConfig>()
        .map_or(PhysicsConfig::default().gravity.to_array(), |config| {
            config.gravity.to_array()
        });
    if world.resource::<RapierBackend>().is_none() {
        world.insert_resource(RapierBackend::new(gravity));
    }

    // Snapshot (shared borrows only): roots with a body and a pose.
    let records: Vec<BodyRecord> = world
        .query::<RigidBody>()
        .filter_map(|(entity, body)| {
            if world.get::<Parent>(entity).is_some() {
                return None;
            }
            let transform = *world.get::<Transform>(entity)?;
            Some(BodyRecord {
                entity,
                body: *body,
                transform,
                collider: world.get::<Collider>(entity).copied(),
                material: world.get::<ColliderMaterial>(entity).copied(),
                velocity: world.get::<Velocity>(entity).copied(),
                gravity_scale: world.get::<GravityScale>(entity).copied(),
                locks: world.get::<LockedAxes>(entity).copied(),
            })
        })
        .collect();

    // Reap list (shared borrows only): tracked entities that died or
    // lost their RigidBody since the last tick.
    let reap: Vec<Entity> = world
        .resource::<RapierBackend>()
        .map_or(Vec::new(), |backend| {
            backend
                .tracked_entities()
                .into_iter()
                .filter(|entity| {
                    !world.is_alive(*entity) || world.get::<RigidBody>(*entity).is_none()
                })
                .collect()
        });

    // Quantize frame time BEFORE the backend borrow begins:
    // `World::resource_mut` holds `&mut World`, which forbids any
    // concurrent borrow — so clocks resolve here, owned, while only
    // shared-or-sequential borrows are live.
    let steps = match world.resource_mut::<PhysicsClock>() {
        Some(clock) => consume_accumulator(&mut clock.accumulator, frame_dt),
        None => 0,
    };

    // Mutate + step under the backend borrow; collect owned sync
    // results so Transform writes happen AFTER the borrow ends.
    let (syncs, completed): (Vec<(Entity, glam::Vec2, f32)>, usize) =
        match world.resource_mut::<RapierBackend>() {
            Some(backend) => {
                for entity in reap {
                    if let Some(handle) = backend.untrack_entity(entity) {
                        backend.remove_body(handle);
                    }
                }
                backend.set_gravity(gravity);

                for record in &records {
                    let handle = match backend.entity_handle(record.entity) {
                        Some(handle) => handle,
                        None => {
                            let seed_angle = z_twist_angle(record.transform.rotation);
                            let created = backend.create_body(
                                &record.body,
                                [
                                    record.transform.translation.x,
                                    record.transform.translation.y,
                                ],
                                seed_angle,
                            );
                            let handle = match created {
                                Ok(handle) => handle,
                                Err(_) => continue,
                            };
                            if let Some(velocity) = &record.velocity {
                                backend.set_velocity(handle, velocity);
                            }
                            if let Some(scale) = &record.gravity_scale {
                                backend.set_body_gravity_scale(handle, scale.0);
                            }
                            if let Some(locks) = &record.locks {
                                backend.set_body_locked_axes(handle, locks);
                            }
                            if let Some(collider) = &record.collider {
                                let material = record.material.unwrap_or_default();
                                let _ = backend.attach_collider(handle, collider, &material);
                            }
                            backend.track_entity(record.entity, handle);
                            handle
                        }
                    };
                    if let Some(velocity) = &record.velocity {
                        backend.set_velocity(handle, velocity);
                    }
                    if let Some(scale) = &record.gravity_scale {
                        backend.set_body_gravity_scale(handle, scale.0);
                    }
                    if let Some(locks) = &record.locks {
                        backend.set_body_locked_axes(handle, locks);
                    }
                    if record.body.kind == RigidBodyKind::KinematicPosition {
                        backend.drive_kinematic_pose(
                            handle,
                            [
                                record.transform.translation.x,
                                record.transform.translation.y,
                            ],
                            z_twist_angle(record.transform.rotation),
                        );
                    }
                }

                let mut completed = 0;
                for _ in 0..steps {
                    if backend.step(FIXED_DT).is_err() {
                        break;
                    }
                    completed += 1;
                }

                let syncs: Vec<(Entity, glam::Vec2, f32)> = records
                    .iter()
                    .filter_map(|record| {
                        backend
                            .entity_handle(record.entity)
                            .and_then(|handle| backend.sync_transform(handle))
                            .map(|(position, angle)| (record.entity, position, angle))
                    })
                    .collect();
                (syncs, completed)
            }
            None => (Vec::new(), 0),
        };

    // Simulation time advances per COMPLETED step, after the backend
    // borrow ends (separate `&mut World` borrow, sequential not nested).
    if completed > 0 {
        if let Some(time) = world.resource_mut::<SimulationTime>() {
            for _ in 0..completed {
                time.advance_step();
            }
        }
    }

    for (entity, position, angle) in syncs {
        if let Some(transform) = world.get_mut::<Transform>(entity) {
            apply_synced_pose(transform, position, angle);
        }
    }
}

/// Registers [`physics_step_system`] on `schedule` as a write system
/// with [`physics_step_access`]'s declaration.
///
/// MUST be called FIRST in the subsystem constructor — before
/// [`register_transform_propagation`](canary_transform::register_transform_propagation),
/// before every render bake. Registration order plus solo-write staging
/// is the ordering mechanism (see the module docs); registering physics
/// anywhere later bakes one-tick-stale globals. The subsystem
/// constructor in `engine/canary-runtime/src/main.rs` owns this order.
pub fn register_physics_step(schedule: &mut Schedule) {
    schedule.add_write_system(physics_step_access(), physics_step_system);
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_transform::GlobalTransform;

    /// Drives one system tick with an explicit frame dt: inserts
    /// [`FrameDelta`], runs [`physics_step_system`], and returns nothing
    /// — callers read the world afterwards. Each tick overwrites the
    /// previous frame's delta (resources hold one value per type), so no
    /// stale dt can survive across ticks.
    fn tick_with(world: &mut World, frame_dt: f32) {
        world.insert_resource(FrameDelta::new(std::time::Duration::from_secs_f32(
            frame_dt,
        )));
        physics_step_system(world);
    }

    /// Spawns a dynamic body entity with a ball collider. The collider
    /// is load-bearing, not decorative: in Rapier mass comes from
    /// colliders, so a colliderless dynamic body has no mass and gravity
    /// cannot move it. No ground is spawned, so the loop is pure gravity
    /// integration with no contact solving.
    fn spawn_falling(world: &mut World, y: f32) -> Entity {
        let entity = world.spawn();
        world.insert(entity, RigidBody::dynamic()).unwrap();
        world.insert(entity, Collider::ball(0.5)).unwrap();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(0.0, y, 0.0)),
            )
            .unwrap();
        entity
    }

    /// Current translation of an entity's `Transform`.
    fn translation_of(world: &World, entity: Entity) -> glam::Vec3 {
        world
            .get::<Transform>(entity)
            .expect("entity must still carry Transform")
            .translation
    }

    #[test]
    fn accumulator_table_quantizes_frame_time_into_whole_steps() {
        // (frame dt, expected steps, expected leftover): the policy as a
        // table, each row independent (fresh accumulator).
        let rows = [
            (0.0, 0, 0.0),
            (FIXED_DT * 0.5, 0, FIXED_DT * 0.5),
            (FIXED_DT, 1, 0.0),
            (FIXED_DT * 2.5, 2, FIXED_DT * 0.5),
            (1.0 / 30.0, 2, 0.0),
            (FIXED_DT * 4.0, 4, 0.0),
        ];
        for (frame_dt, expected_steps, expected_leftover) in rows {
            let mut accumulator = 0.0;
            let steps = consume_accumulator(&mut accumulator, frame_dt);
            assert_eq!(steps, expected_steps, "frame_dt={frame_dt}");
            assert!(
                (accumulator - expected_leftover).abs() < 1e-6,
                "frame_dt={frame_dt}: leftover {accumulator} != {expected_leftover}"
            );
        }
    }

    #[test]
    fn accumulator_carries_remainders_across_ticks() {
        let mut accumulator = 0.0;
        assert_eq!(consume_accumulator(&mut accumulator, FIXED_DT * 0.5), 0);
        assert_eq!(consume_accumulator(&mut accumulator, FIXED_DT * 0.5), 1);
        assert!((accumulator).abs() < 1e-6);
    }

    #[test]
    fn clamp_drops_everything_beyond_four_steps() {
        let mut accumulator = 0.0;
        // Ten quanta at once (a ~166 ms hitch): four steps run, the six
        // leftover quanta are DROPPED — including the sub-step remainder,
        // the accumulator resets to zero (the spiral policy).
        let steps = consume_accumulator(&mut accumulator, FIXED_DT * 10.0);
        assert_eq!(steps, MAX_STEPS_PER_TICK);
        assert_eq!(accumulator, 0.0);
        // And the next tick starts clean, not in debt.
        assert_eq!(consume_accumulator(&mut accumulator, FIXED_DT), 1);
    }

    #[test]
    fn nonpositive_or_nonfinite_frame_time_simulates_nothing() {
        for frame_dt in [0.0, -1.0 / 60.0, f32::NAN, f32::INFINITY] {
            let mut accumulator = FIXED_DT * 0.5;
            assert_eq!(consume_accumulator(&mut accumulator, frame_dt), 0);
            assert!((accumulator - FIXED_DT * 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn simulation_time_advances_by_fixed_increments_only() {
        let mut world = World::new();
        spawn_falling(&mut world, 5.0);

        tick_with(&mut world, FIXED_DT);
        let after_one = world
            .resource::<SimulationTime>()
            .expect("system must ensure SimulationTime")
            .seconds;
        assert!((after_one - FIXED_DT).abs() < 1e-6);

        // A sub-quantum frame steps nothing and advances nothing.
        tick_with(&mut world, FIXED_DT * 0.25);
        let after_idle = world.resource::<SimulationTime>().unwrap().seconds;
        assert!((after_idle - after_one).abs() < 1e-9);

        // A double frame advances exactly two quanta — never wall time.
        tick_with(&mut world, FIXED_DT * 2.0);
        let after_double = world.resource::<SimulationTime>().unwrap().seconds;
        assert!((after_double - after_one - 2.0 * FIXED_DT).abs() < 1e-6);
    }

    #[test]
    fn falling_body_moves_and_time_tracks_steps() {
        let mut world = World::new();
        let body = spawn_falling(&mut world, 5.0);

        for _ in 0..60 {
            tick_with(&mut world, FIXED_DT);
        }

        let end = translation_of(&world, body);
        assert!(
            end.y < 5.0 - 1.0,
            "one simulated second of free fall must drop more than a meter: {end:?}"
        );
        assert!((end.x).abs() < 1e-5);
        let time = world.resource::<SimulationTime>().unwrap().seconds;
        assert!(
            (time - 1.0).abs() < 1e-4,
            "60 steps must read ~1.0 s: {time}"
        );
    }

    #[test]
    fn sync_preserves_z_off_axis_rotation_and_scale() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, RigidBody::dynamic()).unwrap();
        // A pose no 2D solver would produce on its own: plane depth 5,
        // off-axis tilt, non-unit scale.
        let tilted = glam::Quat::from_rotation_x(0.3) * glam::Quat::from_rotation_z(0.1);
        world
            .insert(
                entity,
                Transform {
                    translation: glam::Vec3::new(1.0, 4.0, 5.0),
                    rotation: tilted,
                    scale: glam::Vec3::new(2.0, 3.0, 4.0),
                },
            )
            .unwrap();

        tick_with(&mut world, FIXED_DT);

        let after = world.get::<Transform>(entity).unwrap();
        assert_eq!(after.translation.z, 5.0, "plane depth is solver-invisible");
        assert_eq!(after.scale, glam::Vec3::new(2.0, 3.0, 4.0));
        // The off-axis swing survives exactly: strip the z-twist from
        // both rotations and compare what remains.
        let strip = |rotation: glam::Quat| {
            let components = rotation.normalize().to_array();
            let twist = glam::Quat::from_xyzw(0.0, 0.0, components[2], components[3]).normalize();
            twist.inverse() * rotation.normalize()
        };
        let before_swing = strip(tilted);
        let after_swing = strip(after.rotation);
        for (a, b) in before_swing.to_array().iter().zip(after_swing.to_array()) {
            assert!(
                (a - b).abs() < 1e-5,
                "swing must survive: {before_swing:?} vs {after_swing:?}"
            );
        }
    }

    #[test]
    fn same_ticks_produce_bit_identical_trajectories() {
        // Determinism scope: repeatability on THIS machine (same binary,
        // same thread) — NOT cross-platform (see the module docs).
        fn trajectory() -> Vec<[f32; 3]> {
            let mut world = World::new();
            let body = spawn_falling(&mut world, 5.0);
            let mut poses = Vec::with_capacity(30);
            for _ in 0..30 {
                tick_with(&mut world, FIXED_DT);
                let at = translation_of(&world, body);
                let angle = z_twist_angle(world.get::<Transform>(body).unwrap().rotation);
                poses.push([at.x, at.y, angle]);
            }
            poses
        }

        let first = trajectory();
        let second = trajectory();
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert!(
                a[0].to_bits() == b[0].to_bits()
                    && a[1].to_bits() == b[1].to_bits()
                    && a[2].to_bits() == b[2].to_bits(),
                "trajectories must be bit-identical: {a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn despawned_body_reaps_cleanly_with_no_panic() {
        let mut world = World::new();
        let body = spawn_falling(&mut world, 5.0);
        tick_with(&mut world, FIXED_DT);
        assert_eq!(
            world
                .resource::<RapierBackend>()
                .expect("system must ensure the backend")
                .body_count(),
            1
        );

        world.despawn(body).unwrap();
        tick_with(&mut world, FIXED_DT);

        assert_eq!(
            world.resource::<RapierBackend>().unwrap().body_count(),
            0,
            "the reaped body must be destroyed solver-side"
        );
    }

    #[test]
    fn children_with_parents_are_never_simulated() {
        // Risk R-9: a child body driven by both propagation and physics
        // would fight itself. Physics owns roots; hierarchy owns the
        // rest.
        let mut world = World::new();
        let root = world.spawn();
        world.insert(root, RigidBody::fixed()).unwrap();
        world
            .insert(root, Transform::from_translation(glam::Vec3::ZERO))
            .unwrap();
        let child = world.spawn();
        world.insert(child, RigidBody::dynamic()).unwrap();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(0.0, 5.0, 0.0)),
            )
            .unwrap();
        canary_transform::set_parent(&mut world, child, Some(root)).unwrap();

        tick_with(&mut world, FIXED_DT);

        assert_eq!(
            world.resource::<RapierBackend>().unwrap().body_count(),
            1,
            "only the root owns a solver body"
        );
        assert!(
            (translation_of(&world, child).y - 5.0).abs() < 1e-9,
            "the skipped child must not move"
        );
    }

    #[test]
    fn step_before_propagation_sees_fresh_global() {
        // The ordering law, correct direction: physics registered FIRST,
        // so propagation recomputes globals from just-stepped locals.
        let mut world = World::new();
        let body = spawn_falling(&mut world, 5.0);
        world
            .insert(body, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        world.insert_resource(FrameDelta::default());

        let mut schedule = Schedule::new();
        register_physics_step(&mut schedule);
        canary_transform::register_transform_propagation(&mut schedule);
        schedule.run(&mut world);

        let local_y = translation_of(&world, body).y;
        assert!(local_y < 5.0, "one step must move the body down");
        let global_y = world
            .get::<GlobalTransform>(body)
            .expect("propagation must write globals")
            .matrix()
            .to_scale_rotation_translation()
            .2
            .y;
        assert!(
            (global_y - local_y).abs() < 1e-4,
            "global must reflect the stepped local, not the stale identity: global={global_y} local={local_y}"
        );
    }

    #[test]
    fn reversed_registration_bakes_stale_global() {
        // The ordering law, failure proof (mirrors the render-ecs
        // precedent): propagation registered FIRST sees the pre-step
        // local, so the baked global is one tick stale while the local
        // already moved. If this test ever goes green-by-failure (global
        // fresh despite reversed order), the scheduler's staging changed
        // and every ordering claim needs re-proof.
        let mut world = World::new();
        let body = spawn_falling(&mut world, 5.0);
        world
            .insert(body, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        world.insert_resource(FrameDelta::default());

        let mut schedule = Schedule::new();
        canary_transform::register_transform_propagation(&mut schedule);
        register_physics_step(&mut schedule);
        schedule.run(&mut world);

        let local_y = translation_of(&world, body).y;
        assert!(local_y < 5.0, "physics still steps, wherever it runs");
        let global_y = world
            .get::<GlobalTransform>(body)
            .expect("propagation must write globals")
            .matrix()
            .to_scale_rotation_translation()
            .2
            .y;
        assert!(
            (global_y - 5.0).abs() < 1e-6,
            "reversed order must bake the PRE-step pose: global={global_y} local={local_y}"
        );
    }

    #[test]
    fn clock_remainder_survives_the_tick_that_earned_it() {
        // The accumulator is a resource write, not a local: a 1.5-quanta
        // frame steps once and banks half a quantum for next tick.
        let mut world = World::new();
        spawn_falling(&mut world, 5.0);

        tick_with(&mut world, FIXED_DT * 1.5);
        let leftover = world
            .resource::<PhysicsClock>()
            .expect("system must ensure PhysicsClock")
            .accumulator;
        assert!((leftover - FIXED_DT * 0.5).abs() < 1e-6);

        // The banked half plus another full quantum steps once more and
        // banks half again — no drift across ticks.
        tick_with(&mut world, FIXED_DT);
        let time = world.resource::<SimulationTime>().unwrap().seconds;
        assert!((time - 2.0 * FIXED_DT).abs() < 1e-6);
    }
}
