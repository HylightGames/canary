// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Benchmarks for the stage-based scheduler: the per-tick cost of turning
//! declared [`SystemAccess`] into stages and running them, separated from
//! the cost of the system bodies themselves.
//!
//! The system bodies here are deliberately tiny, so what these measure is
//! the scheduler's own overhead — stage computation, dispatch, and (for
//! read-only stages) the thread scope `Schedule::run` opens per stage.
//! That overhead is what `docs/architecture/execution-model.md` flags as
//! the thing a persistent work-stealing pool would later replace.
//!
//! Run locally with `cargo bench -p canary-scheduler`; in CI these are
//! measured by CodSpeed (see `.github/workflows/codspeed.yml`).

use canary_ecs::World;
use canary_scheduler::{Schedule, SystemAccess};
use divan::{black_box, Bencher};

fn main() {
    divan::main();
}

/// System counts each scheduling benchmark is run at: a handful of
/// systems (a small game's tick) and a couple of dozen (a realistic
/// engine tick once subsystems each register their own).
const SYSTEM_COUNTS: &[usize] = &[8, 32];

/// Entity count for the worlds these schedules run over.
const ENTITY_COUNT: usize = 1_000;

#[derive(Debug, Clone, Copy)]
struct Position {
    x: f32,
}

#[derive(Debug, Clone, Copy)]
struct Velocity {
    dx: f32,
}

#[derive(Debug, Clone, Copy)]
struct Frame(u64);

/// A world of [`ENTITY_COUNT`] entities carrying `Position` + `Velocity`,
/// plus a `Frame` resource for the resource-access benchmarks.
fn populated_world() -> World {
    let mut world = World::new();
    for index in 0..ENTITY_COUNT {
        let entity = world.spawn();
        world
            .insert(entity, Position { x: index as f32 })
            .expect("freshly spawned entity is alive");
        world
            .insert(entity, Velocity { dx: 1.0 })
            .expect("freshly spawned entity is alive");
    }
    world.insert_resource(Frame(0));
    world
}

/// `count` read-only systems that all read the same components, so they
/// never conflict and the scheduler batches them into a single stage.
fn read_only_schedule(count: usize) -> Schedule {
    let mut schedule = Schedule::new();
    for _ in 0..count {
        schedule.add_read_system(
            SystemAccess::new().reads::<Position>().reads::<Velocity>(),
            |world: &World| {
                let mut total = 0.0;
                for (_, position, velocity) in world.query2::<Position, Velocity>() {
                    total += position.x * velocity.dx;
                }
                black_box(total);
            },
        );
    }
    schedule
}

/// `count` write systems, each of which the scheduler must run alone in
/// its own stage — the worst case for stage count.
fn write_only_schedule(count: usize) -> Schedule {
    let mut schedule = Schedule::new();
    for _ in 0..count {
        schedule.add_write_system(
            SystemAccess::new().reads::<Velocity>().writes::<Position>(),
            |world: &mut World| {
                for (_, position, velocity) in world.query2_mut::<Position, Velocity>() {
                    position.x += velocity.dx;
                }
            },
        );
    }
    schedule
}

/// Alternating readers and writers: every writer closes the batch the
/// readers before it had opened, which is the realistic mixed tick.
fn mixed_schedule(count: usize) -> Schedule {
    let mut schedule = Schedule::new();
    for index in 0..count {
        if index % 4 == 3 {
            schedule.add_write_system(
                SystemAccess::new()
                    .reads::<Velocity>()
                    .writes::<Position>()
                    .writes_resource::<Frame>(),
                |world: &mut World| {
                    for (_, position, velocity) in world.query2_mut::<Position, Velocity>() {
                        position.x += velocity.dx;
                    }
                    if let Some(frame) = world.resource_mut::<Frame>() {
                        frame.0 += 1;
                    }
                },
            );
        } else {
            schedule.add_read_system(
                SystemAccess::new()
                    .reads::<Position>()
                    .reads_resource::<Frame>(),
                |world: &World| {
                    let mut total = 0.0;
                    for (_, position) in world.query::<Position>() {
                        total += position.x;
                    }
                    black_box(total);
                },
            );
        }
    }
    schedule
}

/// One tick of a fully parallel schedule: a single read-only stage whose
/// systems all run concurrently under `std::thread::scope`.
#[divan::bench(args = SYSTEM_COUNTS)]
fn run_read_only_schedule(bencher: Bencher, count: usize) {
    let mut world = populated_world();
    let mut schedule = read_only_schedule(count);
    bencher.bench_local(|| schedule.run(&mut world));
}

/// One tick of a fully serial schedule: `count` solo write stages.
#[divan::bench(args = SYSTEM_COUNTS)]
fn run_write_only_schedule(bencher: Bencher, count: usize) {
    let mut world = populated_world();
    let mut schedule = write_only_schedule(count);
    bencher.bench_local(|| schedule.run(&mut world));
}

/// One tick of a mixed schedule: read batches broken up by solo writers.
#[divan::bench(args = SYSTEM_COUNTS)]
fn run_mixed_schedule(bencher: Bencher, count: usize) {
    let mut world = populated_world();
    let mut schedule = mixed_schedule(count);
    bencher.bench_local(|| schedule.run(&mut world));
}

/// Registration cost on its own: building the schedule a tick will run.
#[divan::bench(args = SYSTEM_COUNTS)]
fn register_systems(bencher: Bencher, count: usize) {
    bencher.bench_local(|| black_box(mixed_schedule(count)));
}

/// Building the access declarations themselves: every system hands one
/// of these to the scheduler, and the stage planner reads nothing else.
#[divan::bench]
fn build_access_declarations() {
    let reader = SystemAccess::new()
        .reads::<Position>()
        .reads::<Velocity>()
        .reads_resource::<Frame>();
    let writer = SystemAccess::new()
        .reads::<Velocity>()
        .writes::<Position>()
        .writes_resource::<Frame>();
    black_box((reader, writer));
}
