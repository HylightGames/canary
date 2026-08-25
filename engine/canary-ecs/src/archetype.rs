// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::column::{ColumnOps, Tick};
use crate::entity::Entity;

/// Identifies one [`Archetype`] within a [`crate::World`]'s archetype
/// table, by index. Not exposed publicly -- see
/// [`crate::World`]'s public API, which only ever hands out [`Entity`]
/// handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ArchetypeId(pub(crate) usize);

/// An archetype table: every entity in `entities` has exactly the set of
/// component types named by `signature`, and `columns` stores those
/// components contiguously, row-parallel to `entities`. This is the
/// "entities with the same set of component types are stored
/// contiguously" design from
/// `docs/architecture/core-runtime.md#ecs-architecture`.
pub(crate) struct Archetype {
    /// Sorted (by `TypeId`'s `Ord` impl) so two archetypes with the same
    /// component types always compare equal regardless of the order
    /// those types were inserted in -- this is what lets
    /// [`crate::World`] key its archetype lookup on this `Vec`
    /// directly.
    signature: Vec<TypeId>,
    entities: Vec<Entity>,
    columns: HashMap<TypeId, Box<dyn ColumnOps>>,
}

/// The result of [`Archetype::extract_row`]: everything that used to
/// live at the extracted row, plus the bookkeeping needed to fix up
/// whichever entity the underlying `swap_remove` relocated.
pub(crate) struct ExtractedRow {
    pub(crate) entity: Entity,
    /// One `(component TypeId, value, changed tick)` triple per column
    /// this archetype had -- consumed by [`Archetype::insert_row`],
    /// typically after adding or removing one entry to reflect a
    /// [`crate::World::insert`] or [`crate::World::remove`] transition.
    pub(crate) values: Vec<(TypeId, Box<dyn Any + Send + Sync>, Tick)>,
    /// The entity that `Vec::swap_remove` moved into the vacated row (a
    /// well-formed row's-eye view of "the archetype's previous last
    /// entity"), if the extracted row wasn't already last.
    pub(crate) moved_into_row: Option<Entity>,
}

impl Archetype {
    /// Builds an archetype from a signature and an already-constructed,
    /// still-empty set of columns -- one per `TypeId` in `signature`.
    /// See [`crate::World::get_or_create_archetype`] for how those
    /// columns get built without a `TypeId -> constructor` registry.
    pub(crate) fn from_parts(
        signature: Vec<TypeId>,
        columns: HashMap<TypeId, Box<dyn ColumnOps>>,
    ) -> Self {
        debug_assert_eq!(
            signature.len(),
            columns.len(),
            "archetype signature and columns must name exactly the same TypeIds"
        );
        Self {
            signature,
            entities: Vec::new(),
            columns,
        }
    }

    pub(crate) fn signature(&self) -> &[TypeId] {
        &self.signature
    }

    pub(crate) fn has_component(&self, type_id: TypeId) -> bool {
        self.signature.contains(&type_id)
    }

    pub(crate) fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub(crate) fn column(&self, type_id: TypeId) -> Option<&dyn ColumnOps> {
        self.columns.get(&type_id).map(|c| c.as_ref())
    }

    pub(crate) fn column_mut(&mut self, type_id: TypeId) -> Option<&mut (dyn ColumnOps + '_)> {
        match self.columns.get_mut(&type_id) {
            Some(column) => Some(column.as_mut()),
            None => None,
        }
    }

    /// The row index a value just appended by [`Archetype::insert_row`]
    /// now occupies.
    pub(crate) fn last_row_index(&self) -> usize {
        self.entities.len() - 1
    }

    /// Removes row `row` entirely -- the occupying entity and every
    /// column's value and [`Tick`] at that row -- compacting each
    /// underlying `Vec` via `swap_remove` so the archetype stays dense
    /// (no gaps). See [`ExtractedRow`] for what the caller gets back
    /// and is responsible for doing with it.
    ///
    /// **Performance note, named rather than left implicit**: this
    /// allocates a `Vec` for the returned values plus one `Box` per
    /// column (via [`crate::column::ColumnOps::swap_remove_to_any`]),
    /// so every [`crate::World::insert`]/[`crate::World::remove`]/
    /// [`crate::World::despawn`] call that changes an entity's
    /// archetype costs at least a few heap allocations, proportional to
    /// that entity's component count. Fine for a first correct
    /// implementation and for entities with the small component counts
    /// typical of real usage; a spawn/despawn-heavy workload with large
    /// component counts is where this would show up first if it ever
    /// needs revisiting. Not a bottleneck this codebase has actually
    /// measured -- named as a known tradeoff, not a confirmed problem.
    pub(crate) fn extract_row(&mut self, row: usize) -> ExtractedRow {
        let entity = self.entities.swap_remove(row);
        let moved_into_row = self.entities.get(row).copied();
        let values = self
            .columns
            .iter_mut()
            .map(|(&type_id, column)| {
                let (value, tick) = column.swap_remove_to_any(row);
                (type_id, value, tick)
            })
            .collect();
        ExtractedRow {
            entity,
            values,
            moved_into_row,
        }
    }

    /// Appends a new row built from `entity` and `values`. Every
    /// `TypeId` in `self.signature` must have exactly one matching
    /// entry in `values` -- panics otherwise, per the same
    /// internal-invariant convention as [`crate::column::ColumnOps::push_any`].
    /// Extra entries (e.g. the type a [`crate::World::remove`] call
    /// just pulled out) must already be removed by the caller before
    /// this is called.
    pub(crate) fn insert_row(
        &mut self,
        entity: Entity,
        mut values: Vec<(TypeId, Box<dyn Any + Send + Sync>, Tick)>,
    ) {
        self.entities.push(entity);
        for &type_id in &self.signature {
            let pos = values.iter().position(|(t, _, _)| *t == type_id).expect(
                "insert_row: values missing an entry for a column in this archetype's signature",
            );
            let (_, value, tick) = values.swap_remove(pos);
            self.columns
                .get_mut(&type_id)
                .expect("signature and columns must agree on which TypeIds this archetype has")
                .push_any(value, tick);
        }
        debug_assert!(
            values.is_empty(),
            "insert_row: values contained an entry for a column not in this archetype's signature"
        );
    }
}
