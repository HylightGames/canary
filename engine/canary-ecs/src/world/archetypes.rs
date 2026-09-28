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
use crate::archetype::{Archetype, ArchetypeId};
use crate::column::ColumnOps;
use std::any::TypeId;
use std::collections::HashMap;

impl World {
    /// Finds the archetype for `signature` (sorted here, so callers
    /// don't have to), creating it if it doesn't exist yet.
    ///
    /// A newly created archetype's columns come from one of two places:
    /// `fresh_column`, if given -- the one genuinely new component type
    /// a [`World::insert`] call already has a concrete value for -- or,
    /// for every other type in `signature`,
    /// [`crate::column::ColumnOps::new_same_type`] on the matching
    /// column already present on `source_archetype`. Every type in
    /// `signature` other than `fresh_column`'s must already exist on
    /// `source_archetype`: callers only ever pass a `signature` that's
    /// `source_archetype`'s own signature plus or minus exactly one
    /// type, which this relies on but does not itself verify -- see the
    /// `expect` below.
    pub(crate) fn get_or_create_archetype(
        &mut self,
        mut signature: Vec<TypeId>,
        source_archetype: ArchetypeId,
        mut fresh_column: Option<(TypeId, Box<dyn ColumnOps>)>,
    ) -> ArchetypeId {
        signature.sort_unstable();

        if let Some(&id) = self.archetype_index.get(&signature) {
            return id;
        }

        let mut columns: HashMap<TypeId, Box<dyn ColumnOps>> =
            HashMap::with_capacity(signature.len());
        for &type_id in &signature {
            let is_fresh = matches!(&fresh_column, Some((t, _)) if *t == type_id);
            let column = if is_fresh {
                fresh_column.take().expect("checked Some above").1
            } else {
                self.archetypes[source_archetype.0]
                    .column(type_id)
                    .expect("non-fresh column type must already exist on the source archetype")
                    .new_same_type()
            };
            columns.insert(type_id, column);
        }

        let id = ArchetypeId(self.archetypes.len());
        self.archetypes
            .push(Archetype::from_parts(signature.clone(), columns));
        self.archetype_index.insert(signature.clone(), id);
        for type_id in signature {
            self.type_to_archetypes.entry(type_id).or_default().push(id);
        }
        id
    }
}
