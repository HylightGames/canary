// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Benchmarks for the archetype-backed [`World`]: entity lifecycle,
//! component insertion (including archetype moves), and the query paths
//! every system in the engine runs through on every tick.
//!
//! Run locally with `cargo bench -p canary-ecs`; in CI these are measured
//! by CodSpeed (see `.github/workflows/codspeed.yml`). Sizes are kept in
//! the thousands: large enough that per-entity costs dominate one-off
//! setup, small enough that a full run stays quick under instrumentation.

use canary_ecs::{Entity, World};
use divan::{black_box, Bencher};

fn main() {
    divan::main();
}

/// Entity counts each scaling benchmark is run at.
const ENTITY_COUNTS: &[usize] = &[1_000, 10_000];

#[derive(Debug, Clone, Copy, PartialEq)]
struct Position {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Velocity {
    dx: f32,
    dy: f32,
    dz: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Health(f32);

/// A world of `count` entities that all carry `Position` and `Velocity`,
/// plus the handles to them (in spawn order) for random-access lookups.
fn world_with_moving_entities(count: usize) -> (World, Vec<Entity>) {
    let mut world = World::new();
    let mut entities = Vec::with_capacity(count);
    for index in 0..count {
        let entity = world.spawn();
        let value = index as f32;
        world
            .insert(
                entity,
                Position {
                    x: value,
                    y: value * 2.0,
                    z: value * 3.0,
                },
            )
            .expect("freshly spawned entity is alive");
        world
            .insert(
                entity,
                Velocity {
                    dx: 1.0,
                    dy: 0.5,
                    dz: 0.25,
                },
            )
            .expect("freshly spawned entity is alive");
        entities.push(entity);
    }
    (world, entities)
}

/// Bare entity allocation: no components, so no archetype moves — this is
/// the slot/generation bookkeeping on its own.
#[divan::bench(args = ENTITY_COUNTS)]
fn spawn_entities(bencher: Bencher, count: usize) {
    bencher.bench_local(|| {
        let mut world = World::new();
        for _ in 0..count {
            black_box(world.spawn());
        }
        world
    });
}

/// The realistic spawn path: allocate the entity, then give it two
/// components, which walks it through two archetype transitions.
#[divan::bench(args = ENTITY_COUNTS)]
fn spawn_with_two_components(bencher: Bencher, count: usize) {
    bencher.bench_local(|| black_box(world_with_moving_entities(count)));
}

/// Adding a third component type to fully populated entities: every
/// insert moves a row to a wider archetype, carrying its other columns.
#[divan::bench(args = ENTITY_COUNTS)]
fn insert_component_archetype_move(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| world_with_moving_entities(count))
        .bench_local_values(|(mut world, entities)| {
            for entity in &entities {
                world
                    .insert(*entity, Health(100.0))
                    .expect("entity is alive");
            }
            world
        });
}

/// Removing a component: the mirror archetype move, to a narrower row.
#[divan::bench(args = ENTITY_COUNTS)]
fn remove_component_archetype_move(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| world_with_moving_entities(count))
        .bench_local_values(|(mut world, entities)| {
            for entity in &entities {
                black_box(world.remove::<Velocity>(*entity));
            }
            world
        });
}

/// Single-component iteration: the tightest query shape, a straight walk
/// over one packed column.
#[divan::bench(args = ENTITY_COUNTS)]
fn query_one_component(bencher: Bencher, count: usize) {
    let (world, _) = world_with_moving_entities(count);
    bencher.bench_local(|| {
        let mut total = 0.0;
        for (_, position) in world.query::<Position>() {
            total += position.x;
        }
        black_box(total)
    });
}

/// Two-component iteration: an archetype-set intersection plus a zipped
/// walk over two columns — the shape most gameplay systems use.
#[divan::bench(args = ENTITY_COUNTS)]
fn query_two_components(bencher: Bencher, count: usize) {
    let (world, _) = world_with_moving_entities(count);
    bencher.bench_local(|| {
        let mut total = 0.0;
        for (_, position, velocity) in world.query2::<Position, Velocity>() {
            total += position.x * velocity.dx;
        }
        black_box(total)
    });
}

/// Three-component iteration, on a world where every entity matches.
#[divan::bench(args = ENTITY_COUNTS)]
fn query_three_components(bencher: Bencher, count: usize) {
    let (mut world, entities) = world_with_moving_entities(count);
    for entity in &entities {
        world
            .insert(*entity, Health(100.0))
            .expect("entity is alive");
    }
    bencher.bench_local(|| {
        let mut total = 0.0;
        for (_, position, velocity, health) in world.query3::<Position, Velocity, Health>() {
            total += position.x * velocity.dx + health.0;
        }
        black_box(total)
    });
}

/// The write-side query: integrate positions from velocities in place,
/// which is the canonical per-tick mutation an engine runs.
#[divan::bench(args = ENTITY_COUNTS)]
fn query_two_components_mutable(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| world_with_moving_entities(count).0)
        .bench_local_values(|mut world| {
            for (_, position, velocity) in world.query2_mut::<Position, Velocity>() {
                position.x += velocity.dx;
                position.y += velocity.dy;
                position.z += velocity.dz;
            }
            world
        });
}

/// Change-detection-filtered iteration where only a tenth of the entities
/// were touched since the captured tick — the case the filter exists for.
#[divan::bench(args = ENTITY_COUNTS)]
fn query_changed_since_sparse(bencher: Bencher, count: usize) {
    let (mut world, entities) = world_with_moving_entities(count);
    let baseline = world.change_tick();
    world.advance_tick();
    for entity in entities.iter().step_by(10) {
        if let Some(position) = world.get_mut::<Position>(*entity) {
            position.x += 1.0;
        }
    }
    bencher.bench_local(|| {
        let mut total = 0.0;
        for (_, position) in world.query_changed_since::<Position>(baseline) {
            total += position.x;
        }
        black_box(total)
    });
}

/// Random-access reads by handle: the location lookup plus a downcast,
/// per entity, with no iteration order to help the cache.
#[divan::bench(args = ENTITY_COUNTS)]
fn get_component_by_handle(bencher: Bencher, count: usize) {
    let (world, entities) = world_with_moving_entities(count);
    bencher.bench_local(|| {
        let mut total = 0.0;
        for entity in &entities {
            if let Some(position) = world.get::<Position>(*entity) {
                total += position.x;
            }
        }
        black_box(total)
    });
}

/// Random-access writes by handle, which also stamp the change tick.
#[divan::bench(args = ENTITY_COUNTS)]
fn get_mut_component_by_handle(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| world_with_moving_entities(count))
        .bench_local_values(|(mut world, entities)| {
            for entity in &entities {
                if let Some(position) = world.get_mut::<Position>(*entity) {
                    position.x += 1.0;
                }
            }
            world
        });
}

/// Tearing a populated world down entity by entity: each despawn frees a
/// row and swaps the archetype's last row into the hole.
#[divan::bench(args = ENTITY_COUNTS)]
fn despawn_entities(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| world_with_moving_entities(count))
        .bench_local_values(|(mut world, entities)| {
            for entity in &entities {
                world.despawn(*entity).expect("entity is alive");
            }
            world
        });
}

/// Resource storage: the typed, globally-unique side of the world, read
/// once per system per tick.
#[divan::bench]
fn resource_insert_and_read() {
    let mut world = World::new();
    world.insert_resource(Health(100.0));
    black_box(world.resource::<Health>().copied());
}
