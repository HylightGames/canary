//! Canary Engine core runtime.
//!
//! This is Layer 2 in `docs/architecture/engine-overview.md`: the
//! `App`/subsystem bootstrap, structured logging, and the error-handling
//! conventions the rest of the engine follows. It depends on nothing above
//! it and is deliberately small.
//!
//! See `docs/architecture/core-runtime.md` for the full design, and
//! `docs/roadmap/v0.0.1-roadmap.md` for what's implemented today versus
//! planned (notably: there is no job system / parallel scheduler yet —
//! [`App::run_for`] runs subsystems sequentially).

mod app;
mod error;
mod logging;
mod subsystem;

pub use app::App;
pub use error::{CoreError, SubsystemError};
pub use logging::init_logging;
pub use subsystem::Subsystem;
