// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine core runtime.
//!
//! This is Layer 2 in `docs/architecture/engine-overview.md`: the
//! `App`/subsystem bootstrap, structured logging, and the error-handling
//! conventions the rest of the engine follows. It depends on nothing above
//! it and is deliberately small.
//!
//! See `docs/architecture/core-runtime.md` for the full design, and
//! `docs/roadmap/status.md` for what's implemented today versus
//! planned (notably: the stage-based scheduler lives in the standalone
//! `canary-scheduler` crate -- [`App::run_for`] itself still runs
//! subsystems sequentially and is not wired into a `Schedule`).

mod app;
mod error;
mod logging;
mod subsystem;

pub use app::App;
pub use error::{CoreError, SubsystemError};
pub use logging::init_logging;
pub use subsystem::Subsystem;
