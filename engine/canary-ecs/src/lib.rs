// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine ECS — v0.0.1-pre1 minimal implementation.
//!
//! **This is a deliberate placeholder, not the target design.** It is a
//! generational-index [`World`] with per-type component storage and
//! synchronous, uncached queries — not the archetype-based, parallel-
//! scheduled ECS described in `docs/architecture/core-runtime.md#ecs-architecture`.
//! The public API (`spawn`, `insert`, `query`, ...) is shaped so that
//! migrating to archetype storage later changes the implementation behind
//! these calls more than it changes call sites, but that's an intent, not
//! a guarantee — see `docs/roadmap/v0.0.1-roadmap.md` for what's tracked
//! as follow-up work.

mod entity;
mod error;
mod world;

pub use entity::Entity;
pub use error::EcsError;
pub use world::World;
