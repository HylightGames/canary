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

    /// [`crate::World::register_component`] was called for a component
    /// type whose [`crate::CanaryComponent::SCHEMA_ID`] is already
    /// registered to a *different* Rust type — see
    /// `docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md`
    /// (ADR 0010). The stable schema identity is meant to be unique across the
    /// whole plugin/replication/marketplace boundary, so this is a real
    /// naming conflict between two component authors, not a transient
    /// condition worth retrying.
    #[error("schema id {0:?} is already registered to a different component type")]
    DuplicateSchemaId(&'static str),
}
