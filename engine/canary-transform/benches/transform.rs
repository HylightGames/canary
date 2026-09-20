// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Benchmarks for the transform subsystem: local-to-matrix composition,
//! hierarchy edits, and [`propagate_transforms`] — the per-tick pass whose
//! cost scales with both entity count and hierarchy depth (the crate docs
//! call out that it re-derives depth ordering on every run, which is
//! exactly what these numbers make visible).
//!
//! Run locally with `cargo bench -p canary-transform`; in CI these are
//! measured by CodSpeed (see `.github/workflows/codspeed.yml`).

use canary_ecs::{Entity, World};
use canary_transform::{propagate_transforms, set_parent, GlobalTransform, Transform};
use divan::{black_box, Bencher};

fn main() {
    divan::main();
}

/// Entity counts each scaling benchmark is run at.
const ENTITY_COUNTS: &[usize] = &[1_000, 10_000];

/// Chain lengths for the deep-hierarchy propagation benchmark: each is a
/// single parent-to-child chain that long, so depth (not breadth) is what
/// the propagation pass has to order.
const CHAIN_DEPTHS: &[usize] = &[64, 512];

/// Child counts for the hierarchy-*building* benchmark. Deliberately
/// smaller than [`ENTITY_COUNTS`]: `set_parent` scans the parent's
/// existing `Children` list on every call, so attaching `n` children to
/// one root is quadratic in `n` — measuring it at 10k would dominate the
/// whole suite's runtime without saying anything the smaller sizes don't.
const HIERARCHY_BUILD_COUNTS: &[usize] = &[500, 2_000];

/// A `Transform` that is neither identity nor degenerate, so composition
/// actually does the multiply work a real scene would.
fn sample_transform(index: usize) -> Transform {
    let value = index as f32;
    Transform {
        translation: glam::Vec3::new(value, value * 0.5, value * 0.25),
        rotation: glam::Quat::from_rotation_y(value * 0.01),
        scale: glam::Vec3::splat(1.0 + (value % 4.0) * 0.1),
    }
}

/// `count` parentless entities, each with a local `Transform` only: the
/// flat-scene baseline for propagation.
fn flat_scene(count: usize) -> World {
    let mut world = World::new();
    for index in 0..count {
        let entity = world.spawn();
        world
            .insert(entity, sample_transform(index))
            .expect("freshly spawned entity is alive");
    }
    world
}

/// A single root with `count - 1` direct children — the widest hierarchy
/// shape, where every child composes against the same parent global.
fn wide_scene(count: usize) -> World {
    let mut world = flat_scene(count);
    let entities: Vec<Entity> = world
        .query::<Transform>()
        .map(|(entity, _)| entity)
        .collect();
    let Some((root, children)) = entities.split_first() else {
        return world;
    };
    for child in children {
        set_parent(&mut world, *child, Some(*root)).expect("both entities are alive");
    }
    world
}

/// A single chain of `depth` entities, each parented to the previous one.
fn deep_scene(depth: usize) -> World {
    let mut world = World::new();
    let mut previous: Option<Entity> = None;
    for index in 0..depth {
        let entity = world.spawn();
        world
            .insert(entity, sample_transform(index))
            .expect("freshly spawned entity is alive");
        if let Some(parent) = previous {
            set_parent(&mut world, entity, Some(parent)).expect("both entities are alive");
        }
        previous = Some(entity);
    }
    world
}

/// Local TRS composition on its own: the innermost math every propagation
/// run repeats once per entity.
#[divan::bench]
fn transform_to_matrix() {
    let transform = sample_transform(7);
    black_box(black_box(&transform).to_matrix());
}

/// Matrix composition, the other half of a child's global transform.
#[divan::bench]
fn compose_parent_and_child_matrices() {
    let parent = sample_transform(3).to_matrix();
    let child = sample_transform(11).to_matrix();
    black_box(black_box(parent) * black_box(child));
}

/// First propagation over a flat scene: every entity is a root, and every
/// `GlobalTransform` has to be inserted (an archetype move per entity).
#[divan::bench(args = ENTITY_COUNTS)]
fn propagate_flat_scene_first_run(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| flat_scene(count))
        .bench_local_values(|mut world| {
            propagate_transforms(&mut world);
            world
        });
}

/// Steady-state propagation over a flat scene: `GlobalTransform`s already
/// exist and nothing moved, so this measures the recompute-and-compare
/// path that runs on every quiet tick.
#[divan::bench(args = ENTITY_COUNTS)]
fn propagate_flat_scene_steady_state(bencher: Bencher, count: usize) {
    let mut world = flat_scene(count);
    propagate_transforms(&mut world);
    bencher.bench_local(|| propagate_transforms(&mut world));
}

/// Steady-state propagation over one root with many children: the same
/// entity count as the flat case, but every entity now composes against a
/// parent global.
#[divan::bench(args = ENTITY_COUNTS)]
fn propagate_wide_hierarchy_steady_state(bencher: Bencher, count: usize) {
    let mut world = wide_scene(count);
    propagate_transforms(&mut world);
    bencher.bench_local(|| propagate_transforms(&mut world));
}

/// Steady-state propagation over a deep chain: depth ordering, not
/// breadth, is what dominates here.
#[divan::bench(args = CHAIN_DEPTHS)]
fn propagate_deep_chain_steady_state(bencher: Bencher, depth: usize) {
    let mut world = deep_scene(depth);
    propagate_transforms(&mut world);
    bencher.bench_local(|| propagate_transforms(&mut world));
}

/// A moving root in a deep chain: one local edit that every descendant's
/// global has to follow, which is the case that actually writes.
#[divan::bench(args = CHAIN_DEPTHS)]
fn propagate_deep_chain_after_root_move(bencher: Bencher, depth: usize) {
    let mut world = deep_scene(depth);
    propagate_transforms(&mut world);
    let root = world
        .query::<Transform>()
        .map(|(entity, _)| entity)
        .next()
        .expect("the chain has at least one entity");
    bencher.bench_local(|| {
        world.advance_tick();
        if let Some(transform) = world.get_mut::<Transform>(root) {
            transform.translation.x += 1.0;
        }
        propagate_transforms(&mut world);
    });
}

/// Building a hierarchy: `set_parent` per child, including the cycle check
/// and the `Children` list maintenance on the parent side.
#[divan::bench(args = HIERARCHY_BUILD_COUNTS)]
fn build_wide_hierarchy(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| flat_scene(count))
        .bench_local_values(|mut world| {
            let entities: Vec<Entity> = world
                .query::<Transform>()
                .map(|(entity, _)| entity)
                .collect();
            if let Some((root, children)) = entities.split_first() {
                for child in children {
                    set_parent(&mut world, *child, Some(*root)).expect("both entities are alive");
                }
            }
            world
        });
}

/// Reading back composed world matrices, the consumer side of the cache
/// (rendering extraction does exactly this every frame).
#[divan::bench(args = ENTITY_COUNTS)]
fn read_global_transforms(bencher: Bencher, count: usize) {
    let mut world = wide_scene(count);
    propagate_transforms(&mut world);
    bencher.bench_local(|| {
        let mut total = 0.0;
        for (_, global) in world.query::<GlobalTransform>() {
            total += global.matrix().w_axis.x;
        }
        black_box(total)
    });
}
