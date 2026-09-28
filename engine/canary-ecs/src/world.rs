// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The ECS [`World`]: entities, components, resources, and queries.
//!
//! Split from a single `world.rs` (pre-`.14` hygiene): the [`World`]
//! struct itself plus tick management and `Default` live here; each
//! responsibility cluster of the old `impl World` block is a submodule
//! (`entities`, `components`, `queries`, `resources`, `identity`,
//! `erased`, `archetypes`) and private storage primitives live in
//! `storage`. Items shared between submodules are `pub(crate)`; the
//! external surface is unchanged (`World` plus its public methods,
//! re-exported from the crate root).

mod archetypes;
mod components;
mod entities;
mod erased;
mod identity;
mod queries;
mod resources;
mod storage;
#[cfg(test)]
mod tests;

use std::any::TypeId;
use std::collections::HashMap;

use crate::archetype::{Archetype, ArchetypeId};
use crate::column::Tick;
use storage::{ResourceEntry, Slot};

/// The ECS World: owns entities and their components.
///
/// Backed by **archetype storage**: entities that share the same set of
/// component types live together in one archetype table, with each
/// component type stored as its own contiguous column, row-parallel to
/// the entities -- see `docs/architecture/core-runtime.md#ecs-architecture`
/// for the target design this implements. [`World::query`] and
/// [`World::query_changed_since`] resolve which archetypes to scan
/// through a `TypeId -> Archetype` cache maintained incrementally as
/// archetypes are created, rather than checking every archetype's
/// signature on every call -- the "cached queries" target-design
/// commitment.
///
/// Change detection is a first-class query filter (see
/// [`World::query_changed_since`]), and a first cut of stable,
/// language-agnostic component identity is available via
/// [`World::register_component`] -- see
/// `docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md`
/// (ADR 0010). Neither of those requires anything from a component type
/// beyond what was already required: `T: Send + Sync + 'static` for
/// anything stored via [`World::insert`], full stop.
///
/// Component storage requires `T: Send + Sync` (see [`World::insert`]),
/// which makes `World` itself automatically `Send + Sync` -- required
/// by the target parallel job-system design in
/// `docs/architecture/core-runtime.md#threading--the-job-system`, and
/// cheap to require now, before any component types exist outside this
/// workspace, versus a breaking change later. See
/// `docs/reviews/2026-08-senior-architecture-review.md`, Finding 2.2.
///
/// **Not (yet) covered here**: the parallel job-stealing scheduler
/// itself -- deliberately deferred, see `docs/roadmap/v0.0.2-roadmap.md`,
/// "Explicitly not in v0.0.2" -- and the rest of the Tier A (WASM)
/// loading path beyond the identity registry above.
pub struct World {
    pub(crate) slots: Vec<Slot>,
    pub(crate) free_indices: Vec<u32>,
    pub(crate) archetypes: Vec<Archetype>,
    /// Canonical `signature -> ArchetypeId` lookup. Keys are always
    /// sorted (see [`World::get_or_create_archetype`]), so two
    /// archetypes with the same component types, inserted in any order,
    /// always resolve to the same entry.
    pub(crate) archetype_index: HashMap<Vec<TypeId>, ArchetypeId>,
    /// The query cache: every archetype that contains a column of a
    /// given `TypeId`. Grows as new archetypes are created; archetypes
    /// are never removed once created (an emptied archetype is simply
    /// an archetype with zero rows, ready to be reused), so this never
    /// needs invalidating, only appending to.
    pub(crate) type_to_archetypes: HashMap<TypeId, Vec<ArchetypeId>>,
    /// The archetype for "no components" -- every entity passes through
    /// it at [`World::spawn`], before its first [`World::insert`].
    pub(crate) empty_archetype: ArchetypeId,
    pub(crate) current_tick: Tick,
    /// ADR 0010's proposed registry: stable schema identity -> the
    /// host's `TypeId` for that type, populated by
    /// [`World::register_component`].
    pub(crate) schema_registry: HashMap<&'static str, TypeId>,
    /// Globally-unique, engine-owned state addressed by type rather
    /// than by entity -- see
    /// `docs/architecture/execution-model.md#resources` (`v0.0.7`).
    /// A single-slot analogue of a component column: one value, one
    /// [`Tick`], per type, rather than a `Vec` of them.
    pub(crate) resources: HashMap<TypeId, ResourceEntry>,
    /// Cached count of currently-alive entities, so
    /// [`World::entity_count`] is O(1) instead of scanning every slot
    /// ever created (including long-dead ones) on every call.
    pub(crate) alive_count: usize,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// The tick that will be stamped on the *next* write ([`World::insert`]
    /// moving an entity into a component for the first time, an
    /// [`World::insert`] overwrite, or a [`World::get_mut`] access).
    /// Callers doing change detection typically capture this value, do
    /// some work across one or more [`World::advance_tick`] boundaries,
    /// and later call [`World::query_changed_since`] with it.
    pub fn change_tick(&self) -> Tick {
        self.current_tick
    }

    /// Advances the world's logical tick by one simulation run.
    ///
    /// The app or simulation runner owns this boundary and should advance
    /// once before running the systems for that simulation step. A
    /// scheduler executes systems but does not own time. Do not advance
    /// once per system or once per presentation frame when a frame can
    /// contain zero or multiple simulation steps. See ADR 0021's
    /// tick-ownership rule.
    pub fn advance_tick(&mut self) {
        self.current_tick.increment();
    }
}
