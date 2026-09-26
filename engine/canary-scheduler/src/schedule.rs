// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::collections::HashSet;

use canary_ecs::World;

use crate::access::SystemAccess;

/// A registered system's body: which kind of `World` access it needs to
/// run, matching exactly what its [`SystemAccess`] declared (read-only
/// declarations get `&World`; anything that writes gets `&mut World`).
enum SystemBody {
    Read(Box<dyn FnMut(&World) + Send>),
    Write(Box<dyn FnMut(&mut World) + Send>),
}

struct RegisteredSystem {
    access: SystemAccess,
    body: SystemBody,
}

/// A set of systems, run in the order [`Schedule::run`] can prove is
/// safe given their declared [`SystemAccess`] -- see
/// `docs/architecture/execution-model.md#the-scheduler` for the full
/// design this is a first cut of.
///
/// **What this does**: greedily batches systems, in registration order,
/// into *stages* -- a stage is either one or more read-only systems (any
/// number of shared reads are always safe to run concurrently,
/// regardless of what they read) or exactly one system that writes
/// anything. Stages run in order; within a multi-system stage, every
/// system runs on its own OS thread via [`std::thread::scope`], joined
/// before the next stage starts.
///
/// **What this deliberately doesn't do yet**: run two *write* systems
/// concurrently, even when their [`SystemAccess`] can prove they're
/// disjoint (e.g. one writes only `Position`, the other only
/// `Velocity`). Doing that safely means handing each system its own
/// provably-disjoint view of `World` rather than an exclusive `&mut
/// World` -- a real, substantially larger `unsafe` undertaking than
/// this crate's first release attempts (see `execution-model.md`'s
/// "Known limitations" for the full reasoning). Every write system runs
/// alone, sequentially relative to everything else, which is always
/// correct, just not always maximally parallel.
///
/// Threads are spawned fresh per multi-system stage via
/// [`std::thread::scope`] rather than pulled from a persistent
/// work-stealing pool -- correct, but real spawn overhead on every
/// parallel stage of every tick. Swapping in a persistent pool (or an
/// external work-stealing crate) later is an internal change to
/// [`Schedule::run`]; it doesn't change anything about [`SystemAccess`]
/// or how systems are registered.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<RegisteredSystem>,
}

impl Schedule {
    /// A schedule with no systems registered yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a read-only system: `access` must not declare any
    /// writes (component or resource) -- see [`SystemAccess::writes`]/
    /// [`SystemAccess::writes_resource`] and use
    /// [`Schedule::add_write_system`] instead if it needs to write
    /// anything.
    ///
    /// # Panics
    /// If `access.is_read_only()` is false -- a read-only system whose
    /// own declared access disagrees with which method registered it is
    /// exactly the kind of mismatch
    /// `docs/architecture/execution-model.md`'s Access invariant exists
    /// to prevent, so this is checked eagerly at registration rather
    /// than silently trusted.
    pub fn add_read_system(
        &mut self,
        access: SystemAccess,
        body: impl FnMut(&World) + Send + 'static,
    ) -> &mut Self {
        assert!(
            access.is_read_only(),
            "add_read_system: access declares a write; use add_write_system instead"
        );
        self.systems.push(RegisteredSystem {
            access,
            body: SystemBody::Read(Box::new(body)),
        });
        self
    }

    /// Registers a system that needs to write at least one component or
    /// resource type. Always runs alone in its own stage -- see
    /// [`Schedule`]'s own docs for why.
    ///
    /// # Panics
    /// If `access.is_read_only()` is true -- the mirror of
    /// [`Schedule::add_read_system`]'s eager check: a write system whose
    /// own declared access claims it writes nothing would silently get a
    /// solo stage it never needed while readers around it lose a chance
    /// to batch, so the mismatch fails loudly at registration rather
    /// than silently costing parallelism.
    pub fn add_write_system(
        &mut self,
        access: SystemAccess,
        body: impl FnMut(&mut World) + Send + 'static,
    ) -> &mut Self {
        assert!(
            !access.is_read_only(),
            "add_write_system: access declares no writes; use add_read_system instead"
        );
        self.systems.push(RegisteredSystem {
            access,
            body: SystemBody::Write(Box::new(body)),
        });
        self
    }

    /// Runs every registered system exactly once, in stages computed
    /// fresh from the current registration order and each system's
    /// declared [`SystemAccess`] -- see [`Schedule`]'s own docs for what
    /// "stages" means and what it does and doesn't parallelize.
    ///
    /// # Panics
    /// A panicking system body propagates out of `run` (a read-stage
    /// panic surfaces after [`std::thread::scope`] has joined the
    /// stage's threads). There is no rollback: a write system that
    /// panics mid-body may leave `World` half-mutated. Treat a
    /// panicking system as a bug to fix, not a recoverable error to
    /// catch -- `run` offers no atomicity guarantee across systems.
    pub fn run(&mut self, world: &mut World) {
        for stage in self.compute_stages() {
            self.run_stage(&stage, world);
        }
    }

    /// Greedily groups systems, in registration order, into stages: a
    /// system joins the current stage only if the stage is still
    /// entirely read-only, the new system is itself read-only, and its
    /// declared access doesn't conflict with anything already in the
    /// stage. Otherwise, the current stage closes and a new one starts.
    /// Returns each stage as the registration-order indices of the
    /// systems in it, since [`RegisteredSystem`]'s closures can't be
    /// cloned or moved out without disturbing `self.systems`' own
    /// storage.
    fn compute_stages(&self) -> Vec<Vec<usize>> {
        let mut stages: Vec<Vec<usize>> = Vec::new();
        let mut current_stage: Vec<usize> = Vec::new();
        let mut current_stage_access = SystemAccess::new();
        let mut current_stage_is_read_only = true;

        for (index, system) in self.systems.iter().enumerate() {
            let system_is_read_only = system.access.is_read_only();
            let must_start_new_stage = !current_stage.is_empty()
                && (!system_is_read_only
                    || !current_stage_is_read_only
                    || system.access.conflicts_with(&current_stage_access));

            if must_start_new_stage {
                stages.push(std::mem::take(&mut current_stage));
                current_stage_access = SystemAccess::new();
                current_stage_is_read_only = true;
            }

            current_stage.push(index);
            current_stage_access.merge(&system.access);
            current_stage_is_read_only &= system_is_read_only;
        }
        if !current_stage.is_empty() {
            stages.push(current_stage);
        }
        stages
    }

    /// Runs every system in `stage`. A single-system stage just calls
    /// it directly with the access its [`SystemBody`] variant demands.
    /// A multi-system stage is, by [`Schedule::compute_stages`]'s own
    /// construction, guaranteed to contain only [`SystemBody::Read`]
    /// systems -- each runs on its own thread via
    /// [`std::thread::scope`], all joined before this call returns.
    fn run_stage(&mut self, stage: &[usize], world: &mut World) {
        if let [index] = *stage {
            match &mut self.systems[index].body {
                SystemBody::Read(body) => body(world),
                SystemBody::Write(body) => body(world),
            }
            return;
        }

        let world_ref: &World = world;
        let stage_members: HashSet<usize> = stage.iter().copied().collect();
        std::thread::scope(|scope| {
            for (index, system) in self.systems.iter_mut().enumerate() {
                if !stage_members.contains(&index) {
                    continue;
                }
                let SystemBody::Read(body) = &mut system.body else {
                    unreachable!(
                        "compute_stages only ever puts read-only systems into a multi-member stage"
                    );
                };
                scope.spawn(move || body(world_ref));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Position;
    struct Velocity;
    struct Health;

    fn noop_read(_: &World) {}
    fn noop_write(_: &mut World) {}

    #[test]
    fn independent_read_only_systems_share_one_stage() {
        let mut schedule = Schedule::new();
        schedule.add_read_system(SystemAccess::new().reads::<Position>(), noop_read);
        schedule.add_read_system(SystemAccess::new().reads::<Velocity>(), noop_read);
        schedule.add_read_system(SystemAccess::new().reads::<Health>(), noop_read);

        let stages = schedule.compute_stages();
        assert_eq!(stages, vec![vec![0, 1, 2]]);
    }

    #[test]
    fn a_write_system_always_gets_its_own_stage() {
        let mut schedule = Schedule::new();
        schedule.add_read_system(SystemAccess::new().reads::<Position>(), noop_read);
        schedule.add_write_system(SystemAccess::new().writes::<Velocity>(), noop_write);
        schedule.add_read_system(SystemAccess::new().reads::<Health>(), noop_read);

        let stages = schedule.compute_stages();
        assert_eq!(stages, vec![vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn a_read_after_a_conflicting_write_starts_a_new_stage() {
        let mut schedule = Schedule::new();
        schedule.add_write_system(SystemAccess::new().writes::<Position>(), noop_write);
        // Reads what the write above writes -- must not be merged into
        // any stage that could run concurrently with or before it.
        schedule.add_read_system(SystemAccess::new().reads::<Position>(), noop_read);

        let stages = schedule.compute_stages();
        assert_eq!(stages, vec![vec![0], vec![1]]);
    }

    #[test]
    fn a_read_unrelated_to_an_earlier_write_still_cannot_join_its_stage() {
        // Position and Velocity don't conflict with each other, but a
        // write system never shares a stage with anything else at all
        // -- see Schedule's own docs for why (no provably-disjoint
        // concurrent writes yet, and by extension no mixing a write's
        // solo stage with an unrelated read either, to keep "a write
        // always runs in a stage by itself" a simple, checkable rule
        // rather than one with exceptions).
        let mut schedule = Schedule::new();
        schedule.add_write_system(SystemAccess::new().writes::<Position>(), noop_write);
        schedule.add_read_system(SystemAccess::new().reads::<Velocity>(), noop_read);

        let stages = schedule.compute_stages();
        assert_eq!(stages, vec![vec![0], vec![1]]);
    }

    #[test]
    fn empty_schedule_runs_without_panicking() {
        let mut schedule = Schedule::new();
        let mut world = World::new();
        schedule.run(&mut world);
    }
}
