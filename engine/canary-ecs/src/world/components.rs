// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use super::storage::EntityLocation;
use super::World;
use crate::column::{ColumnOps, TypedColumn};
use crate::entity::Entity;
use crate::error::EcsError;
use std::any::{Any, TypeId};

impl World {
    /// Inserts (or replaces) a component of type `T` on `entity`.
    ///
    /// If `entity` doesn't yet have a `T`, this moves it to a different
    /// archetype -- creating that archetype on first use -- carrying
    /// every one of its other components along unchanged (values *and*
    /// change ticks; see [`World::query_changed_since`]). If it already
    /// has a `T`, the value is overwritten in place with no archetype
    /// move.
    ///
    /// `T: Send + Sync` is required so `World` itself can be `Send +
    /// Sync` -- see the type-level docs above. Returns
    /// [`EcsError::StaleOrUnknownEntity`] if `entity` is not alive.
    pub fn insert<T: Send + Sync + 'static>(
        &mut self,
        entity: Entity,
        component: T,
    ) -> Result<(), EcsError> {
        let location = self
            .location_of(entity)
            .ok_or(EcsError::StaleOrUnknownEntity)?;
        let type_id = TypeId::of::<T>();
        let current_tick = self.current_tick;
        let old_archetype = location.archetype;

        if self.archetypes[old_archetype.0].has_component(type_id) {
            let column = self.archetypes[old_archetype.0]
                .column_mut(type_id)
                .and_then(|c| c.as_any_mut().downcast_mut::<TypedColumn<T>>())
                .expect("archetype signature says this TypeId is present; its column must exist and match");
            column.set(location.row, component, current_tick);
            return Ok(());
        }

        let mut new_signature: Vec<TypeId> = self.archetypes[old_archetype.0].signature().to_vec();
        new_signature.push(type_id);
        let fresh_column: Box<dyn ColumnOps> = Box::new(TypedColumn::<T>::new());
        let new_archetype = self.get_or_create_archetype(
            new_signature,
            old_archetype,
            Some((type_id, fresh_column)),
        );

        let mut extracted = self.archetypes[old_archetype.0].extract_row(location.row);
        extracted.values.push((
            type_id,
            Box::new(component) as Box<dyn Any + Send + Sync>,
            current_tick,
        ));

        let new_row = {
            let archetype = &mut self.archetypes[new_archetype.0];
            archetype.insert_row(extracted.entity, extracted.values);
            archetype.last_row_index()
        };
        self.set_location(
            entity,
            EntityLocation {
                archetype: new_archetype,
                row: new_row,
            },
        );
        if let Some(moved) = extracted.moved_into_row {
            self.set_location(
                moved,
                EntityLocation {
                    archetype: old_archetype,
                    row: location.row,
                },
            );
        }

        Ok(())
    }

    /// Returns a reference to `entity`'s component of type `T`, if it is
    /// alive and has one.
    pub fn get<T: 'static>(&self, entity: Entity) -> Option<&T> {
        let location = self.location_of(entity)?;
        let type_id = TypeId::of::<T>();
        self.archetypes[location.archetype.0]
            .column(type_id)?
            .as_any()
            .downcast_ref::<TypedColumn<T>>()?
            .values()
            .get(location.row)
    }

    /// Returns a mutable reference to `entity`'s component of type `T`,
    /// if it is alive and has one. Marks that component as changed at
    /// the current [`World::change_tick`] -- see
    /// [`World::query_changed_since`] -- unconditionally, since a caller
    /// receiving `&mut T` is conservatively assumed to write through it.
    pub fn get_mut<T: 'static>(&mut self, entity: Entity) -> Option<&mut T> {
        let location = self.location_of(entity)?;
        let type_id = TypeId::of::<T>();
        let current_tick = self.current_tick;
        let column = self.archetypes[location.archetype.0]
            .column_mut(type_id)?
            .as_any_mut()
            .downcast_mut::<TypedColumn<T>>()?;
        column.mark_changed(location.row, current_tick);
        column.get_mut(location.row)
    }

    /// Removes and returns `entity`'s component of type `T`, if it is
    /// alive and has one. Moves `entity` to a smaller archetype,
    /// carrying every remaining component along unchanged (values and
    /// change ticks alike).
    ///
    /// Idempotent by design: a stale/unknown entity and an alive
    /// entity lacking `T` both yield `None`, identically. `remove`
    /// answers "give me the component if there is one to take," not
    /// "prove this handle is live" — callers that need the distinction
    /// (use-after-despawn detection) check [`World::is_alive`] first,
    /// the way [`World::insert`]/[`World::despawn`] enforce it with
    /// [`EcsError::StaleOrUnknownEntity`]. Do not "fix" this into a
    /// `Result` without also updating every call site that relies on
    /// the current shape.
    pub fn remove<T: 'static>(&mut self, entity: Entity) -> Option<T> {
        let location = self.location_of(entity)?;
        let type_id = TypeId::of::<T>();
        let old_archetype = location.archetype;

        if !self.archetypes[old_archetype.0].has_component(type_id) {
            return None;
        }

        let mut new_signature: Vec<TypeId> = self.archetypes[old_archetype.0].signature().to_vec();
        new_signature.retain(|&t| t != type_id);
        let new_archetype = self.get_or_create_archetype(new_signature, old_archetype, None);

        let mut extracted = self.archetypes[old_archetype.0].extract_row(location.row);
        let pos = extracted
            .values
            .iter()
            .position(|(t, _, _)| *t == type_id)
            .expect("archetype signature said this TypeId is present");
        let (_, boxed, _tick) = extracted.values.swap_remove(pos);
        // Every value in `extracted.values` was, by construction, put
        // there by `World::insert::<U>` for whichever `U` the paired
        // `TypeId` names -- so a `TypeId` match here guarantees the
        // downcast below succeeds; see `ColumnOps::push_any`'s matching
        // invariant.
        let removed = *boxed
            .downcast::<T>()
            .expect("column TypeId matched but concrete downcast failed");

        let new_row = {
            let archetype = &mut self.archetypes[new_archetype.0];
            archetype.insert_row(extracted.entity, extracted.values);
            archetype.last_row_index()
        };
        self.set_location(
            entity,
            EntityLocation {
                archetype: new_archetype,
                row: new_row,
            },
        );
        if let Some(moved) = extracted.moved_into_row {
            self.set_location(
                moved,
                EntityLocation {
                    archetype: old_archetype,
                    row: location.row,
                },
            );
        }

        Some(removed)
    }
}
