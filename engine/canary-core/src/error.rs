// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use thiserror::Error;

/// The error type a [`Subsystem`](crate::Subsystem) reports from `init`.
///
/// Deliberately a boxed dynamic error rather than an associated type on
/// the trait: `App` holds a heterogeneous collection of subsystems and
/// needs one error type to report failures through, and subsystem
/// implementers should feel free to use their own typed errors internally
/// and box them at this boundary. See
/// `docs/architecture/core-runtime.md#error-handling-conventions`.
pub type SubsystemError = Box<dyn std::error::Error + Send + Sync>;

/// Errors surfaced by [`App`](crate::App) itself.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A subsystem's [`Subsystem::init`](crate::Subsystem::init) returned
    /// an error.
    #[error("subsystem `{name}` failed to initialize: {source}")]
    SubsystemInit {
        /// The failing subsystem's [`Subsystem::name`](crate::Subsystem::name).
        name: String,
        /// The underlying error the subsystem reported.
        #[source]
        source: SubsystemError,
    },
}
