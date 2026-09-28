// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use super::storage::{EntityLocation, Slot};
use super::World;
use crate::archetype::{Archetype, ArchetypeId};
use crate::column::Tick;
use crate::entity::Entity;
use crate::error::EcsError;
use std::collections::HashMap;

impl World {
    /// Creates an empty `World`.
    pub fn new() -> Self {
        let mut archetype_index = HashMap::new();
        archetype_index.insert(Vec::new(), ArchetypeId(0));
        Self {
            slots: Vec::new(),
            free_indices: Vec::new(),
            archetypes: vec![Archetype::from_parts(Vec::new(), HashMap::new())],
            archetype_index,
            type_to_archetypes: HashMap::new(),
            empty_archetype: ArchetypeId(0),
            current_tick: Tick::default(),
            schema_registry: HashMap::new(),
            resources: HashMap::new(),
            alive_count: 0,
        }
    }

    /// Spawns a new entity with no components and returns its handle.
    ///
    /// Reuses a despawned slot's index when one is available, bumping
    /// that slot's generation so old handles into it remain detectably
    /// stale (see [`World::is_alive`]). The new entity starts in the
    /// empty archetype, moving to progressively larger (or, after a
    /// [`World::remove`], smaller) archetypes as components are added.
    pub fn spawn(&mut self) -> Entity {
        // Every successful spawn below increments `alive_count`
        // exactly once (and `despawn` decrements it) — see
        // [`World::entity_count`].
        let entity = if let Some(index) = self.free_indices.pop() {
            let slot = &mut self.slots[index as usize];
            slot.alive = true;
            Entity {
                index,
                generation: slot.generation,
            }
        } else {
            // `slots` only grows (despawned slots are recycled via
            // `free_indices`, never removed), so its length is the
            // total number of slots ever created. Past `u32::MAX`
            // slots, two slots would share an index and corrupt
            // `is_alive`/location bookkeeping -- unreachable in any
            // realistic workload (4.29B spawns), but a silent `as`
            // truncation would hide it entirely, so this fails loudly
            // instead, matching the `u64`-generation reasoning on
            // `Entity` (fail visibly at the limit rather than alias).
            let index = u32::try_from(self.slots.len())
                .expect("entity slot index space exhausted (2^32 slots ever created)");
            self.slots.push(Slot {
                generation: 0,
                alive: true,
                location: None,
            });
            Entity {
                index,
                generation: 0,
            }
        };

        let empty_archetype = self.empty_archetype;
        self.alive_count += 1;
        let row = {
            let archetype = &mut self.archetypes[empty_archetype.0];
            archetype.insert_row(entity, Vec::new());
            archetype.last_row_index()
        };
        self.slots[entity.index as usize].location = Some(EntityLocation {
            archetype: empty_archetype,
            row,
        });
        entity
    }

    /// Despawns an entity, removing all of its components and marking
    /// its slot free for reuse by a future [`World::spawn`]. The slot's
    /// generation is bumped first, so `entity` (and any copy of it) is
    /// reported as not alive by [`World::is_alive`] from this point on,
    /// even after the slot is recycled.
    ///
    /// Returns [`EcsError::StaleOrUnknownEntity`] if `entity` was
    /// already not alive.
    pub fn despawn(&mut self, entity: Entity) -> Result<(), EcsError> {
        let location = self
            .location_of(entity)
            .ok_or(EcsError::StaleOrUnknownEntity)?;

        let extracted = self.archetypes[location.archetype.0].extract_row(location.row);
        if let Some(moved) = extracted.moved_into_row {
            self.set_location(
                moved,
                EntityLocation {
                    archetype: location.archetype,
                    row: location.row,
                },
            );
        }

        let slot = &mut self.slots[entity.index as usize];
        slot.alive = false;
        // `wrapping_add` on a `u64` is defense in depth, not a real
        // expectation of ever wrapping -- see the type-level docs on
        // `crate::Entity` for why `u64` (rather than the original `u32`)
        // makes that distinction meaningful instead of theoretical.
        slot.generation = slot.generation.wrapping_add(1);
        slot.location = None;
        self.alive_count -= 1;
        self.free_indices.push(entity.index);

        Ok(())
    }

    /// Whether `entity` refers to a currently-alive entity in this world
    /// (i.e. was spawned and has not since been despawned).
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.slots
            .get(entity.index as usize)
            .is_some_and(|slot| slot.alive && slot.generation == entity.generation)
    }

    /// The number of currently-alive entities, maintained
    /// incrementally by [`World::spawn`]/[`World::despawn`] (O(1)).
    pub fn entity_count(&self) -> usize {
        self.alive_count
    }

    pub(crate) fn location_of(&self, entity: Entity) -> Option<EntityLocation> {
        let slot = self.slots.get(entity.index as usize)?;
        if slot.alive && slot.generation == entity.generation {
            slot.location
        } else {
            None
        }
    }

    pub(crate) fn set_location(&mut self, entity: Entity, location: EntityLocation) {
        self.slots[entity.index as usize].location = Some(location);
    }
}
