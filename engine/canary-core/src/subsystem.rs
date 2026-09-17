// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use crate::error::SubsystemError;

/// A Layer 3 engine piece (ECS, rendering, physics, networking, ...) that
/// can be registered with an [`App`](crate::App).
///
/// See `docs/architecture/core-runtime.md#the-appengine-bootstrap` and
/// `docs/architecture/engine-overview.md` for how this fits into the
/// engine's layering.
pub trait Subsystem: 'static {
    /// A short, human-readable name used in logging and error messages.
    fn name(&self) -> &str;

    /// Called once, in registration order, before the tick loop starts.
    ///
    /// The default implementation does nothing; override it for
    /// subsystems that need to allocate resources or validate
    /// configuration before ticking.
    fn init(&mut self) -> Result<(), SubsystemError> {
        Ok(())
    }

    /// Called once per tick, in registration order, with how much time
    /// elapsed since the previous tick.
    ///
    /// [`App::run_for`](crate::App::run_for) passes a caller-specified,
    /// fixed `dt` (deterministic, for tests and headless/CI use);
    /// [`App::run`](crate::App::run) passes the real wall-clock elapsed
    /// time each iteration. Either way, a subsystem should treat `dt` as
    /// the only source of truth for elapsed time — reading the system
    /// clock directly inside `tick` would silently break determinism
    /// under `run_for`, which is the entire reason it exists.
    ///
    /// Ticks are still sequential, on the calling thread, in
    /// registration order — see `canary_scheduler::Schedule` for the
    /// actual concurrent-execution model this is expected to delegate
    /// to internally once a subsystem has systems worth scheduling that
    /// way (see `docs/architecture/execution-model.md#the-scheduler`);
    /// `Subsystem::tick` itself stays this simple on purpose; it's the
    /// per-subsystem entry point, not the scheduler.
    fn tick(&mut self, dt: std::time::Duration) {
        let _ = dt;
    }

    /// Called once during shutdown, in **reverse** registration order (the
    /// last subsystem started is the first shut down), mirroring the usual
    /// resource-teardown convention.
    fn shutdown(&mut self) {}
}
