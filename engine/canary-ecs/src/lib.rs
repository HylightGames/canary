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
//! Since `v0.0.3`, that identity registry has a real external consumer:
//! `canary-plugin-api`'s Tier A (WASM Component Model) plugin loader,
//! which is what [`World::get_erased`]/[`World::set_erased`]/
//! [`World::has_component_erased`] and [`Entity::from_raw_parts`] exist
//! for — type-erased ECS access and entity-handle reconstruction for a
//! caller on the other side of a language boundary, who only has a
//! runtime `TypeId` (resolved via [`World::type_id_for_schema`]) and raw
//! index/generation bits, not a concrete Rust type or an opaque `Entity`
//! value. See each method's own doc comment for the specific safety
//! properties involved.
//!
//! **Not yet here**: the parallel job-stealing scheduler — see
//! `docs/architecture/core-runtime.md#threading--the-job-system` — still
//! the one piece of the target ECS design this crate doesn't implement.

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
