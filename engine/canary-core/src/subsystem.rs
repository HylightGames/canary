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

    /// Called once per [`App::run_for`](crate::App::run_for) tick, in
    /// registration order.
    ///
    /// v0.0.1-pre1 runs every subsystem's `tick` sequentially on the
    /// calling thread. See
    /// `docs/architecture/core-runtime.md#threading--the-job-system` for
    /// the parallel, dependency-scheduled target design this is expected
    /// to grow into.
    fn tick(&mut self) {}

    /// Called once during shutdown, in **reverse** registration order (the
    /// last subsystem started is the first shut down), mirroring the usual
    /// resource-teardown convention.
    fn shutdown(&mut self) {}
}
