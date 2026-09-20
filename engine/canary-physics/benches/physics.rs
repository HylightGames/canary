// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Benchmarks for 2D physics: the Rapier backend on its own (body
//! creation, fixed steps, pose reads) and the scheduled
//! [`physics_step_system`], which adds the ECS snapshot/drive/sync work
//! around each step.
//!
//! Both levels are measured because they regress for different reasons:
//! the backend numbers move when the solver or its configuration
//! changes, the system numbers move when the ECS bridge around it does.
//!
//! Run locally with `cargo bench -p canary-physics`; in CI these are
//! measured by CodSpeed (see `.github/workflows/codspeed.yml`).

use canary_ecs::World;
use canary_physics::{
    physics_step_system, BodyHandle, Collider, ColliderMaterial, FrameDelta, PhysicsBackend,
    RapierBackend, RigidBody, Velocity, FIXED_DT,
};
use canary_transform::Transform;
use divan::{black_box, Bencher};

fn main() {
    divan::main();
}

/// Body counts the benchmarks run at: a small scene, and one past the
/// point where broad-phase and island management start to matter.
const BODY_COUNTS: &[usize] = &[64, 256];

/// A dynamic body's starting pose, spread over a grid so the scene has
/// real contact work rather than one degenerate pile.
fn body_pose(index: usize) -> [f32; 2] {
    let column = (index % 16) as f32;
    let row = (index / 16) as f32;
    [column * 1.5 - 12.0, row * 1.5 + 2.0]
}

/// A backend holding `count` dynamic balls above a fixed ground cuboid,
/// plus the handles to those dynamic bodies.
fn populated_backend(count: usize) -> (RapierBackend, Vec<BodyHandle>) {
    let mut backend = RapierBackend::new([0.0, -9.81]);
    let ground = backend
        .create_body(&RigidBody::fixed(), [0.0, -1.0], 0.0)
        .expect("a finite fixed-body pose is valid");
    backend
        .attach_collider(
            ground,
            &Collider::cuboid([50.0, 1.0]),
            &ColliderMaterial::default(),
        )
        .expect("a positive-extent cuboid is valid");
    let mut bodies = Vec::with_capacity(count);
    for index in 0..count {
        let body = backend
            .create_body(&RigidBody::dynamic(), body_pose(index), 0.0)
            .expect("a finite dynamic-body pose is valid");
        backend
            .attach_collider(body, &Collider::ball(0.5), &ColliderMaterial::new(0.5, 0.2))
            .expect("a positive-radius ball is valid");
        bodies.push(body);
    }
    (backend, bodies)
}

/// A world of `count` dynamic entities plus ground, ready for
/// [`physics_step_system`].
fn populated_world(count: usize) -> World {
    let mut world = World::new();

    let ground = world.spawn();
    world
        .insert(
            ground,
            Transform::from_translation(glam::Vec3::new(0.0, -1.0, 0.0)),
        )
        .expect("freshly spawned entity is alive");
    world
        .insert(ground, RigidBody::fixed())
        .expect("freshly spawned entity is alive");
    world
        .insert(ground, Collider::cuboid([50.0, 1.0]))
        .expect("freshly spawned entity is alive");

    for index in 0..count {
        let [x, y] = body_pose(index);
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(x, y, 0.0)),
            )
            .expect("freshly spawned entity is alive");
        world
            .insert(entity, RigidBody::dynamic())
            .expect("freshly spawned entity is alive");
        world
            .insert(entity, Collider::ball(0.5))
            .expect("freshly spawned entity is alive");
        world
            .insert(entity, Velocity::zero())
            .expect("freshly spawned entity is alive");
    }

    world.insert_resource(FrameDelta::new(std::time::Duration::from_secs_f32(
        FIXED_DT,
    )));
    world
}

/// Scene construction on the backend: body creation plus collider
/// attachment, which a level load pays once per body.
#[divan::bench(args = BODY_COUNTS)]
fn backend_create_bodies(bencher: Bencher, count: usize) {
    bencher.bench_local(|| black_box(populated_backend(count)));
}

/// One fixed solver step on a settled-ish scene: the per-tick cost that
/// dominates a physics-heavy game.
#[divan::bench(args = BODY_COUNTS)]
fn backend_step(bencher: Bencher, count: usize) {
    let (mut backend, _) = populated_backend(count);
    for _ in 0..10 {
        backend.step(FIXED_DT).expect("fixed timestep is accepted");
    }
    bencher.bench_local(|| backend.step(FIXED_DT).expect("fixed timestep is accepted"));
}

/// Reading every body's pose back out of the solver — the sync half of a
/// tick, without the ECS writes around it.
#[divan::bench(args = BODY_COUNTS)]
fn backend_sync_poses(bencher: Bencher, count: usize) {
    let (mut backend, handles) = populated_backend(count);
    backend.step(FIXED_DT).expect("fixed timestep is accepted");
    bencher.bench_local(|| {
        let mut found = 0usize;
        for handle in &handles {
            if backend.sync_transform(*handle).is_some() {
                found += 1;
            }
        }
        black_box(found)
    });
}

/// The first scheduled tick: every entity is untracked, so this includes
/// discovery and body creation on top of the step.
#[divan::bench(args = BODY_COUNTS)]
fn physics_system_first_tick(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| populated_world(count))
        .bench_local_values(|mut world| {
            physics_step_system(&mut world);
            world
        });
}

/// Steady-state scheduled ticks: snapshot, drive, one fixed step, and
/// write every pose back into `Transform`.
#[divan::bench(args = BODY_COUNTS)]
fn physics_system_steady_tick(bencher: Bencher, count: usize) {
    let mut world = populated_world(count);
    for _ in 0..10 {
        physics_step_system(&mut world);
    }
    bencher.bench_local(|| physics_step_system(&mut world));
}
