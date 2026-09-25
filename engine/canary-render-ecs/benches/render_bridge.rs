// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Benchmarks for the ECS-to-render bridge: extraction, CPU bake, and the
//! whole per-frame CPU chain (propagate -> extract -> bake) that runs
//! before a single GPU command is recorded.
//!
//! Nothing here touches Vulkan: the draw side needs a real device, so it
//! lives in this crate's `#[ignore]`-gated integration tests instead.
//! What is measured is exactly the CPU work every frame pays regardless
//! of backend — including the `extract_scene` vs `extract_scene_into`
//! difference the extract module's own docs call out.
//!
//! Run locally with `cargo bench -p canary-render-ecs`; in CI these are
//! measured by CodSpeed (see `.github/workflows/codspeed.yml`).

use canary_ecs::World;
use canary_render_ecs::{
    bake_scene_to_vertices, bake_scene_to_vertices_into, bake_scene_to_vertices_with_aspect,
    extract_scene, extract_scene_into, BakeScratch, RenderItem, Renderable,
};
use canary_transform::{propagate_transforms, GlobalTransform, Transform};
use divan::{black_box, Bencher};

fn main() {
    divan::main();
}

/// Scene sizes, as entity counts: a modest scene and a busy one. Each
/// entity carries [`TRIANGLES_PER_ENTITY`] triangles, so the larger size
/// is a 12k-triangle frame.
const ENTITY_COUNTS: &[usize] = &[100, 1_000];

/// Triangles per renderable entity — a cube's worth, so per-entity costs
/// (query, clone, matrix fetch) and per-triangle costs (transform,
/// project, sort) both show up.
const TRIANGLES_PER_ENTITY: usize = 12;

/// Triangle soup for one entity, placed in front of the camera so the
/// bake's depth rejection keeps all of it.
fn entity_soup(seed: usize) -> Vec<[f32; 3]> {
    let base = (seed % 16) as f32 * 0.05;
    (0..TRIANGLES_PER_ENTITY)
        .flat_map(|triangle| {
            let offset = triangle as f32 * 0.01;
            [
                [-0.5 + base + offset, -0.5 + base, 0.0],
                [0.5 + base + offset, -0.5 + base, 0.0],
                [0.0 + base + offset, 0.5 + base, offset],
            ]
        })
        .collect()
}

/// A world of `count` renderable entities, each with a `Transform`, a
/// propagated `GlobalTransform`, and a soup `Renderable`.
fn renderable_scene(count: usize) -> World {
    let mut world = World::new();
    for index in 0..count {
        let entity = world.spawn();
        let spread = (index % 32) as f32 * 0.1 - 1.6;
        world
            .insert(
                entity,
                Transform {
                    translation: glam::Vec3::new(spread, spread * 0.5, (index % 8) as f32 * 0.1),
                    rotation: glam::Quat::from_rotation_y(index as f32 * 0.01),
                    scale: glam::Vec3::splat(0.4),
                },
            )
            .expect("freshly spawned entity is alive");
        world
            .insert(entity, Renderable::new(entity_soup(index), [0.2, 0.6, 0.9]))
            .expect("freshly spawned entity is alive");
    }
    propagate_transforms(&mut world);
    world
}

/// The extracted snapshot for a scene of `count` entities.
fn extracted_items(count: usize) -> Vec<RenderItem> {
    extract_scene(&renderable_scene(count))
}

/// Extraction into a fresh `Vec`: one allocation per entity per frame,
/// which is the cost `extract_scene_into` exists to avoid.
#[divan::bench(args = ENTITY_COUNTS)]
fn extract_into_fresh_vec(bencher: Bencher, count: usize) {
    let world = renderable_scene(count);
    bencher.bench_local(|| black_box(extract_scene(&world)));
}

/// Extraction into last frame's scratch buffer: the steady-state path,
/// reusing every vertex buffer whose capacity still fits.
#[divan::bench(args = ENTITY_COUNTS)]
fn extract_into_reused_scratch(bencher: Bencher, count: usize) {
    let world = renderable_scene(count);
    let mut scratch = Vec::new();
    extract_scene_into(&world, &mut scratch);
    bencher.bench_local(|| {
        extract_scene_into(&world, &mut scratch);
        black_box(scratch.len())
    });
}

/// Extraction from a world where half the entities are not renderable:
/// the archetype intersection has to skip them without touching them.
#[divan::bench(args = ENTITY_COUNTS)]
fn extract_mixed_world(bencher: Bencher, count: usize) {
    let mut world = renderable_scene(count);
    for index in 0..count {
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform {
                    translation: glam::Vec3::splat(index as f32 * 0.01),
                    rotation: glam::Quat::IDENTITY,
                    scale: glam::Vec3::ONE,
                },
            )
            .expect("freshly spawned entity is alive");
        world
            .insert(entity, GlobalTransform(glam::Mat4::IDENTITY))
            .expect("freshly spawned entity is alive");
    }
    let mut scratch = Vec::new();
    bencher.bench_local(|| {
        extract_scene_into(&world, &mut scratch);
        black_box(scratch.len())
    });
}

/// The bake itself: world transform, camera shift, perspective project,
/// painter sort, and vertex emission for every triangle in the frame.
#[divan::bench(args = ENTITY_COUNTS)]
fn bake_frame(bencher: Bencher, count: usize) {
    let items = extracted_items(count);
    bencher.bench_local(|| black_box(bake_scene_to_vertices(black_box(&items))));
}

/// The bake into last frame's scratch + output buffers: the steady-state
/// path the scheduled bake system takes every tick, reusing both the
/// pending-triangle intermediates and the frame's own vertex allocation.
#[divan::bench(args = ENTITY_COUNTS)]
fn bake_frame_into_reused_scratch(bencher: Bencher, count: usize) {
    let items = extracted_items(count);
    let mut scratch = BakeScratch::default();
    let mut out = Vec::new();
    bake_scene_to_vertices_into(&items, &mut out, &mut scratch);
    bencher.bench_local(|| {
        bake_scene_to_vertices_into(black_box(&items), &mut out, &mut scratch);
        black_box(out.len())
    });
}

/// The same bake against a non-square target, which is what a real
/// window-sized draw uses.
#[divan::bench(args = ENTITY_COUNTS)]
fn bake_frame_with_aspect(bencher: Bencher, count: usize) {
    let items = extracted_items(count);
    bencher.bench_local(|| {
        black_box(bake_scene_to_vertices_with_aspect(
            black_box(&items),
            16.0 / 9.0,
        ))
    });
}

/// One full CPU frame on a scene whose transforms changed: propagate the
/// hierarchy, extract into the reused scratch, then bake. This is the
/// end-to-end number a rendering change should be judged by.
#[divan::bench(args = ENTITY_COUNTS)]
fn full_frame_propagate_extract_bake(bencher: Bencher, count: usize) {
    let mut world = renderable_scene(count);
    let mut scratch = Vec::new();
    extract_scene_into(&world, &mut scratch);
    // A tenth of the scene moves each frame, so propagation has real
    // writes to do rather than confirming an unchanged world.
    let movers: Vec<_> = world
        .query::<Renderable>()
        .map(|(entity, _)| entity)
        .step_by(10)
        .collect();
    bencher.bench_local(|| {
        world.advance_tick();
        for entity in &movers {
            if let Some(transform) = world.get_mut::<Transform>(*entity) {
                transform.translation.x += 0.001;
            }
        }
        propagate_transforms(&mut world);
        extract_scene_into(&world, &mut scratch);
        black_box(bake_scene_to_vertices(&scratch))
    });
}

/// One full CPU frame reusing both scratch buffers: propagate the
/// hierarchy, extract into the reused extract scratch, then bake into the
/// reused bake scratch + frame buffer. This is the end-to-end steady-state
/// number the scheduled bake system actually pays per tick — compare
/// against `full_frame_propagate_extract_bake` (fresh bake buffers) for
/// the reuse delta.
#[divan::bench(args = ENTITY_COUNTS)]
fn full_frame_reused_bake_scratch(bencher: Bencher, count: usize) {
    let mut world = renderable_scene(count);
    let mut scratch = Vec::new();
    extract_scene_into(&world, &mut scratch);
    let mut bake_scratch = BakeScratch::default();
    let mut frame = Vec::new();
    bake_scene_to_vertices_into(&scratch, &mut frame, &mut bake_scratch);
    // A tenth of the scene moves each frame, so propagation has real
    // writes to do rather than confirming an unchanged world.
    let movers: Vec<_> = world
        .query::<Renderable>()
        .map(|(entity, _)| entity)
        .step_by(10)
        .collect();
    bencher.bench_local(|| {
        world.advance_tick();
        for entity in &movers {
            if let Some(transform) = world.get_mut::<Transform>(*entity) {
                transform.translation.x += 0.001;
            }
        }
        propagate_transforms(&mut world);
        extract_scene_into(&world, &mut scratch);
        bake_scene_to_vertices_into(&scratch, &mut frame, &mut bake_scratch);
        black_box(frame.len())
    });
}
