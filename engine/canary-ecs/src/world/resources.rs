// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use super::storage::ResourceEntry;
use super::World;
use crate::column::Tick;
use std::any::TypeId;

impl World {
    /// Inserts (or replaces) the resource of type `T`, stamping the
    /// current [`World::change_tick`] -- see
    /// `docs/architecture/execution-model.md#resources`. Unlike
    /// [`World::insert`] for components, there's no entity to attach
    /// this to and no archetype transition: a resource is global to the
    /// `World`, at most one value per type.
    pub fn insert_resource<T: Send + Sync + 'static>(&mut self, resource: T) {
        self.resources.insert(
            TypeId::of::<T>(),
            ResourceEntry {
                value: Box::new(resource),
                changed_tick: self.current_tick,
            },
        );
    }

    /// Returns a reference to the resource of type `T`, if one has been
    /// inserted.
    pub fn resource<T: 'static>(&self) -> Option<&T> {
        self.resources
            .get(&TypeId::of::<T>())?
            .value
            .downcast_ref::<T>()
    }

    /// Returns a mutable reference to the resource of type `T`, if one
    /// has been inserted. Marks it as changed at the current
    /// [`World::change_tick`] unconditionally -- the same conservative
    /// convention as [`World::get_mut`] for components, since a caller
    /// receiving `&mut T` is assumed to write through it.
    pub fn resource_mut<T: 'static>(&mut self) -> Option<&mut T> {
        let current_tick = self.current_tick;
        let entry = self.resources.get_mut(&TypeId::of::<T>())?;
        entry.changed_tick = current_tick;
        entry.value.downcast_mut::<T>()
    }

    /// Removes and returns the resource of type `T`, if one has been
    /// inserted.
    pub fn remove_resource<T: 'static>(&mut self) -> Option<T> {
        let entry = self.resources.remove(&TypeId::of::<T>())?;
        // `resources` is keyed by `TypeId::of::<T>()` and every entry is
        // only ever constructed by `insert_resource::<T>` for that same
        // `T` (see that method), so this downcast cannot fail -- an
        // internal invariant, not a caller-facing error, per the same
        // convention as `ColumnOps::push_any`.
        Some(
            *entry
                .value
                .downcast::<T>()
                .expect("resources map key and stored value type must agree"),
        )
    }

    /// Whether a resource of type `T` has been inserted.
    pub fn contains_resource<T: 'static>(&self) -> bool {
        self.resources.contains_key(&TypeId::of::<T>())
    }

    /// Whether the resource of type `T` has been written (via
    /// [`World::insert_resource`] or [`World::resource_mut`]) at a tick
    /// strictly later than `since` -- the resource analogue of
    /// [`World::query_changed_since`]. Returns `false`, not an error or
    /// panic, if no resource of type `T` has been inserted at all,
    /// matching this crate's existing "missing is `None`/`false`, not a
    /// distinct error case" convention (see [`World::has_component_erased`]).
    pub fn resource_changed_since<T: 'static>(&self, since: Tick) -> bool {
        self.resources
            .get(&TypeId::of::<T>())
            .is_some_and(|entry| entry.changed_tick > since)
    }
}
