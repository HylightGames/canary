// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine ECS.
//!
//! [`World`] is backed by archetype storage: entities that share the same
//! set of component types are stored contiguously, with cached
//! (`TypeId`-indexed) queries and change detection as a first-class query
//! filter — see `docs/architecture/core-runtime.md#ecs-architecture` for
//! the target design this now implements. A first cut of stable,
//! language-agnostic component identity (for the plugin/replication/
//! marketplace boundary) is available via [`CanaryComponent`] and
//! [`World::register_component`] — see
//! `docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md`
//! (ADR 0010).
//!
//! **Not yet here**: the parallel job-stealing scheduler itself, and the
//! rest of the Tier A (WASM) plugin-loading path beyond the identity
//! registry above — both deliberately out of scope for this pass, see
//! `docs/roadmap/v0.0.2-roadmap.md`.

mod archetype;
mod column;
mod component_identity;
mod entity;
mod error;
mod world;

pub use column::Tick;
pub use component_identity::CanaryComponent;
pub use entity::Entity;
pub use error::EcsError;
pub use world::World;
