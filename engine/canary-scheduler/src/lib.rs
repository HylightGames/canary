// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine ECS scheduler.
//!
//! [`SystemAccess`] is a system's declared reads/writes (component and
//! resource types, tracked separately); [`Schedule`] uses that
//! declaration, and nothing else, to run systems safely -- batching
//! non-conflicting read-only systems to run concurrently, and running
//! anything that writes alone. See
//! `docs/architecture/execution-model.md#the-scheduler` for the full
//! design, including what this first release deliberately does not
//! attempt yet (concurrent *writes*, even provably disjoint ones).
//!
//! This crate depends only on `canary-ecs`'s public API (`World`,
//! queries, resources) -- no privileged access to `World`'s internals,
//! per this project's "no privileged built-ins" principle: a
//! third-party scheduler could be written the exact same way.

mod access;
mod schedule;

pub use access::SystemAccess;
pub use schedule::Schedule;

#[cfg(test)]
mod tests {
    use super::*;
    use canary_ecs::World;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Position {
        x: f32,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Velocity {
        dx: f32,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct DoubledPosition {
        x: f32,
    }

    #[test]
    fn a_single_write_system_runs_and_mutates_the_world() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 0.0 }).unwrap();
        world.insert(entity, Velocity { dx: 5.0 }).unwrap();

        let mut schedule = Schedule::new();
        schedule.add_write_system(
            SystemAccess::new().reads::<Velocity>().writes::<Position>(),
            |world: &mut World| {
                for (_, position, velocity) in world.query2_mut::<Position, Velocity>() {
                    position.x += velocity.dx;
                }
            },
        );

        schedule.run(&mut world);
        assert_eq!(world.get::<Position>(entity), Some(&Position { x: 5.0 }));
    }

    #[test]
    fn a_system_sees_an_earlier_conflicting_systems_write() {
        // Registration order matters here: the second system reads
        // what the first writes, so it must observe the first's
        // effect, not a stale value -- a real correctness property,
        // not just a scheduling detail, since both are write systems
        // and every write system already runs in its own solo stage in
        // strict registration order (see schedule.rs's own stage-level
        // tests for why).
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0 }).unwrap();
        world.insert(entity, DoubledPosition { x: 0.0 }).unwrap();

        let mut schedule = Schedule::new();
        schedule.add_write_system(
            SystemAccess::new().writes::<Position>(),
            move |world: &mut World| {
                if let Some(position) = world.get_mut::<Position>(entity) {
                    position.x = 10.0;
                }
            },
        );
        schedule.add_write_system(
            SystemAccess::new()
                .reads::<Position>()
                .writes::<DoubledPosition>(),
            move |world: &mut World| {
                let current_x = world.get::<Position>(entity).map(|p| p.x);
                if let (Some(x), Some(doubled)) =
                    (current_x, world.get_mut::<DoubledPosition>(entity))
                {
                    doubled.x = x * 2.0;
                }
            },
        );

        schedule.run(&mut world);
        assert_eq!(
            world.get::<DoubledPosition>(entity),
            Some(&DoubledPosition { x: 20.0 })
        );
    }

    #[test]
    fn independent_read_only_systems_actually_run_concurrently() {
        // Two read-only systems that each take noticeably longer than
        // spawning a thread costs. If they truly run concurrently, the
        // whole schedule takes roughly one system's duration, not the
        // sum of both -- a real, if timing-based, proof that this
        // isn't secretly just sequential execution dressed up as a
        // "schedule". A generous threshold (well under the sum, well
        // over one duration) keeps this reliable on a loaded CI runner
        // without risking a flaky failure in either direction.
        let mut world = World::new();
        world.insert_resource(Position { x: 0.0 });

        let concurrent_count = Arc::new(AtomicU32::new(0));
        let observed_concurrency = Arc::new(AtomicU32::new(0));
        let sleep_duration = Duration::from_millis(120);

        let mut schedule = Schedule::new();
        for _ in 0..2 {
            let concurrent_count = Arc::clone(&concurrent_count);
            let observed_concurrency = Arc::clone(&observed_concurrency);
            schedule.add_read_system(
                SystemAccess::new().reads_resource::<Position>(),
                move |_world: &World| {
                    let now_running = concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;
                    observed_concurrency.fetch_max(now_running, Ordering::SeqCst);
                    std::thread::sleep(sleep_duration);
                    concurrent_count.fetch_sub(1, Ordering::SeqCst);
                },
            );
        }

        let start = Instant::now();
        schedule.run(&mut world);
        let elapsed = start.elapsed();

        assert_eq!(
            observed_concurrency.load(Ordering::SeqCst),
            2,
            "both read-only systems should have been in flight at the same time"
        );
        assert!(
            elapsed < sleep_duration * 2,
            "two 120ms read-only systems took {elapsed:?}; expected well under 240ms if they ran concurrently"
        );
    }

    #[test]
    fn a_write_system_never_overlaps_with_a_concurrent_read_system() {
        // Even when access would technically be disjoint enough to
        // parallelize with more machinery than this crate builds yet
        // (see Schedule's own docs), a write system must never run at
        // the same time as anything else -- checked here with a
        // shared flag a concurrent overlap would actually catch, not
        // just inferred from the scheduling algorithm's intent.
        let mut world = World::new();
        world.insert_resource(Velocity { dx: 0.0 });
        let entity_count_holder = Arc::new(AtomicU32::new(0));

        let mut schedule = Schedule::new();
        let flag_for_read = Arc::clone(&entity_count_holder);
        schedule.add_read_system(
            SystemAccess::new().reads_resource::<Velocity>(),
            move |_: &World| {
                flag_for_read.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                flag_for_read.fetch_sub(1, Ordering::SeqCst);
            },
        );
        let flag_for_write = Arc::clone(&entity_count_holder);
        schedule.add_write_system(
            SystemAccess::new().writes::<Position>(),
            move |_: &mut World| {
                // If this ever ran concurrently with the read system above,
                // this would observe a nonzero value here.
                assert_eq!(flag_for_write.load(Ordering::SeqCst), 0);
            },
        );

        schedule.run(&mut world);
    }
}
