// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use tracing_subscriber::EnvFilter;

/// Initializes global structured logging for a Canary process.
///
/// Honors the `RUST_LOG` environment variable (standard `tracing_subscriber`
/// `EnvFilter` syntax, e.g. `RUST_LOG=canary_ecs=debug,info`), defaulting to
/// `info` if unset. See `docs/architecture/core-runtime.md#logging--diagnostics`
/// for why structured (span/field-based) logging is a Layer 2 concern.
///
/// Safe to call more than once in the same process (for example, across
/// multiple `#[test]` functions in one test binary): subsequent calls are
/// silently ignored rather than panicking, since `tracing`'s global
/// subscriber can only be installed once.
pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // A failed `try_init` here almost always just means a subscriber was
    // already installed (e.g. a previous test in this binary called this
    // already) — that's fine, so we deliberately ignore the error rather
    // than panicking.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
