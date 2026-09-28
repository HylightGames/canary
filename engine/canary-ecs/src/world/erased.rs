// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use super::World;
use crate::entity::Entity;
use std::any::{Any, TypeId};

impl World {
    /// Type-erased counterpart to [`World::is_alive`] plus
    /// [`World::get`], for a caller that only has a runtime `TypeId`
    /// (e.g. resolved via [`World::type_id_for_schema`]), not a
    /// concrete type at the call site — the "host adapter" the
    /// component identity registry
    /// (`docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md`)
    /// was built to eventually feed, now consumed by Tier A (`v0.0.3`)
    /// — see `docs/roadmap/v0.0.3-roadmap.md`'s component-data-ABI
    /// scope item. Returns `None` if `entity` isn't alive or doesn't
    /// have a component of type `type_id`.
    pub fn get_erased(&self, entity: Entity, type_id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
        let location = self.location_of(entity)?;
        let column = self.archetypes[location.archetype.0].column(type_id)?;
        Some(column.get_erased(location.row))
    }

    /// Type-erased counterpart to [`World::insert`], **narrowed to
    /// overwriting an existing component's value only** — it does not
    /// insert a new component type onto an entity, unlike
    /// [`World::insert`]. Returns `false` (and changes nothing) if
    /// `entity` isn't alive or doesn't already have a component of type
    /// `type_id`; `true` on a successful overwrite.
    ///
    /// This is deliberately narrower than [`World::insert`]'s full
    /// archetype-transition behavior — a first-cut boundary for Tier A
    /// specifically (an untrusted plugin changing an entity's
    /// *component set*, versus overwriting a value it was already
    /// granted access to, are different risk profiles), not a general
    /// ECS limitation. See `docs/roadmap/v0.0.3-roadmap.md`.
    ///
    /// # Panics
    /// If `value`'s concrete type doesn't match `type_id` — an internal
    /// invariant violation the caller is responsible for avoiding (e.g.
    /// a codec resolving the wrong `TypeId` for a schema), not a
    /// user-facing error.
    pub fn set_erased(
        &mut self,
        entity: Entity,
        type_id: TypeId,
        value: Box<dyn Any + Send + Sync>,
    ) -> bool {
        let Some(location) = self.location_of(entity) else {
            return false;
        };
        let current_tick = self.current_tick;
        let Some(column) = self.archetypes[location.archetype.0].column_mut(type_id) else {
            return false;
        };
        column.set_erased(location.row, value, current_tick);
        true
    }

    /// Type-erased counterpart to checking whether an alive entity has
    /// a component of a given (runtime-only) `TypeId`. `false` for a
    /// dead or unknown entity, matching [`World::get_erased`]'s
    /// `None`-on-dead-entity convention rather than treating it as a
    /// distinct error case.
    pub fn has_component_erased(&self, entity: Entity, type_id: TypeId) -> bool {
        self.location_of(entity)
            .is_some_and(|location| self.archetypes[location.archetype.0].has_component(type_id))
    }
}
