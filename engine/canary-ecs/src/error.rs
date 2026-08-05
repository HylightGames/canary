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

/// Errors returned by [`crate::World`] operations.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum EcsError {
    /// The given [`crate::Entity`] was never spawned in this `World`, or
    /// was despawned since the handle was obtained (its slot may since
    /// have been recycled for a different entity — see
    /// `docs/architecture/core-runtime.md#ecs-architecture`).
    #[error("entity is stale (despawned) or was never spawned in this World")]
    StaleOrUnknownEntity,
}
