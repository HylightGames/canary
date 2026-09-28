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
use crate::archetype::Archetype;
use crate::column::{Tick, TypedColumn};
use crate::entity::Entity;
use std::any::TypeId;

impl World {
    /// Shared iteration core for [`World::query`] and
    /// [`World::query_changed_since`]: every `(Entity, &T, Tick)` across
    /// every archetype the type-to-archetype cache says has a `T`
    /// column.
    pub(crate) fn iter_component<T: 'static>(&self) -> impl Iterator<Item = (Entity, &T, Tick)> {
        let type_id = TypeId::of::<T>();
        self.type_to_archetypes
            .get(&type_id)
            .into_iter()
            .flatten()
            .flat_map(move |&archetype_id| {
                let archetype = &self.archetypes[archetype_id.0];
                let column = archetype
                    .column(type_id)
                    .and_then(|c| c.as_any().downcast_ref::<TypedColumn<T>>())
                    .expect("type_to_archetypes cache and archetype columns disagree");
                archetype
                    .entities()
                    .iter()
                    .copied()
                    .zip(column.values().iter())
                    .zip(column.changed_ticks().iter())
                    .map(|((entity, value), &tick)| (entity, value, tick))
            })
    }

    /// Iterates every currently-alive entity that has a component of
    /// type `T`, along with a reference to that component.
    ///
    /// Backed by the archetype query cache -- see the type-level docs
    /// on [`World`] -- so this is a scan over exactly the archetypes
    /// that can possibly contain `T`, each a tightly packed, contiguous
    /// column, rather than a linear scan filtered after the fact.
    pub fn query<T: 'static>(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.iter_component::<T>()
            .map(|(entity, value, _tick)| (entity, value))
    }

    /// Like [`World::query`], but only yields entities whose `T`
    /// component has been written (via [`World::insert`] or
    /// [`World::get_mut`]) at a tick strictly later than `since` --
    /// typically a [`Tick`] captured from an earlier
    /// [`World::change_tick`] call. This is the "first-class query
    /// filter" named in `docs/architecture/core-runtime.md#ecs-architecture`.
    ///
    /// Moving `entity` to a different archetype because some *other*
    /// component was inserted or removed does not, on its own, advance
    /// `T`'s change tick -- see `ColumnOps::push_any` in
    /// `engine/canary-ecs/src/column.rs`, which carries a component's
    /// existing tick across such a move instead of stamping a new one.
    pub fn query_changed_since<T: 'static>(
        &self,
        since: Tick,
    ) -> impl Iterator<Item = (Entity, &T)> {
        self.iter_component::<T>()
            .filter_map(move |(entity, value, tick)| (tick > since).then_some((entity, value)))
    }

    /// Shared downcast helper for [`World::query2`]/[`World::query3`]:
    /// `archetype`'s column of type `T`, downcast from the type-erased
    /// [`crate::column::ColumnOps`] storage. Panics (an internal
    /// invariant violation, not a user-facing error -- see
    /// [`crate::column::ColumnOps::push_any`]'s matching convention) if
    /// `archetype` doesn't actually have a `T` column; every call site
    /// only calls this after confirming presence via
    /// [`crate::archetype::Archetype::has_component`] or the
    /// `type_to_archetypes` cache, so a panic here means those two
    /// disagreed with the archetype's real columns, not a normal
    /// "entity doesn't have this component" case.
    pub(crate) fn typed_column<T: 'static>(archetype: &Archetype) -> &TypedColumn<T> {
        archetype
            .column(TypeId::of::<T>())
            .and_then(|c| c.as_any().downcast_ref::<TypedColumn<T>>())
            .expect("archetype signature and columns must agree on stored types")
    }

    /// Iterates every currently-alive entity that has *both* a
    /// component of type `A` and one of type `B`, yielding
    /// `(Entity, &A, &B)` -- a real archetype-set intersection, not a
    /// union: an entity with only one of the two is never yielded. See
    /// `docs/architecture/execution-model.md#queries` for why this is a
    /// hand-written method rather than a generic `Query<D>` over
    /// tuples, and [`World::query2_mut`] for the "one mutable, one
    /// shared" counterpart.
    pub fn query2<A: 'static, B: 'static>(&self) -> impl Iterator<Item = (Entity, &A, &B)> {
        let type_b = TypeId::of::<B>();
        self.type_to_archetypes
            .get(&TypeId::of::<A>())
            .into_iter()
            .flatten()
            .copied()
            .filter(move |id| self.archetypes[id.0].has_component(type_b))
            .flat_map(move |archetype_id| {
                let archetype = &self.archetypes[archetype_id.0];
                let column_a = Self::typed_column::<A>(archetype);
                let column_b = Self::typed_column::<B>(archetype);
                archetype
                    .entities()
                    .iter()
                    .copied()
                    .zip(column_a.values().iter())
                    .zip(column_b.values().iter())
                    .map(|((entity, a), b)| (entity, a, b))
            })
    }

    /// Like [`World::query2`], for three components at once: entities
    /// with `A`, `B`, *and* `C` all present, yielding
    /// `(Entity, &A, &B, &C)`.
    pub fn query3<A: 'static, B: 'static, C: 'static>(
        &self,
    ) -> impl Iterator<Item = (Entity, &A, &B, &C)> {
        let type_b = TypeId::of::<B>();
        let type_c = TypeId::of::<C>();
        self.type_to_archetypes
            .get(&TypeId::of::<A>())
            .into_iter()
            .flatten()
            .copied()
            .filter(move |id| {
                let archetype = &self.archetypes[id.0];
                archetype.has_component(type_b) && archetype.has_component(type_c)
            })
            .flat_map(move |archetype_id| {
                let archetype = &self.archetypes[archetype_id.0];
                let column_a = Self::typed_column::<A>(archetype);
                let column_b = Self::typed_column::<B>(archetype);
                let column_c = Self::typed_column::<C>(archetype);
                archetype
                    .entities()
                    .iter()
                    .copied()
                    .zip(column_a.values().iter())
                    .zip(column_b.values().iter())
                    .zip(column_c.values().iter())
                    .map(|(((entity, a), b), c)| (entity, a, b, c))
            })
    }

    /// Like [`World::query2`], but yields a mutable reference to `A`
    /// alongside a shared reference to `B` -- the "update `A` based on
    /// `B`" shape (a `Position` updated from a `Velocity`) that's the
    /// canonical reason multi-component queries exist. Marks every
    /// yielded entity's `A` as changed at the current
    /// [`World::change_tick`], the same conservative "assume the caller
    /// writes through it" policy [`World::get_mut`] already uses.
    ///
    /// Note the marking happens at *call* time, not per yielded item:
    /// merely constructing (even dropping unconsumed) this iterator
    /// dirties every matched row, so a no-op call still advances
    /// downstream [`World::query_changed_since`] consumers. This is a
    /// consequence of the eager collection below, not a separate
    /// policy -- factor it into change-detection-sensitive code.
    ///
    /// Eagerly collects into a `Vec` internally (returning its
    /// `IntoIter`) rather than lazily streaming per archetype -- unlike
    /// [`World::query2`]/[`World::query3`], yielding a `&mut A`
    /// alongside a `&B` from the same archetype needs the archetype's
    /// `unsafe` column-pair split (see
    /// `docs/architecture/execution-model.md#queries`, "On `unsafe`"), and doing that split once per archetype inside this
    /// method's own body -- rather than lazily, across an opaque
    /// `Iterator`'s repeated calls into `self.archetypes` at
    /// caller-controlled points -- is what keeps the *rest* of this
    /// method ordinary safe Rust (a real `IterMut` over each archetype,
    /// not repeated indexed access the borrow checker can't verify is
    /// disjoint). One `Vec` allocation per call, proportional to the
    /// matched entity count -- fine for a first correct implementation,
    /// per this crate's existing `Archetype::extract_row` precedent for
    /// naming a similar tradeoff rather than hiding it.
    ///
    /// # Panics
    /// If `A` and `B` are the same type. Yielding `(&mut T, &T)` to the
    /// same column would be a real aliasing violation, so the
    /// archetype's column-pair split rejects it with an assertion
    /// rather than resolving it on the caller's behalf --
    /// a one-character typo (`query2_mut::<Position, Position>`) fails
    /// loudly here instead of compiling into undefined behavior.
    pub fn query2_mut<A: 'static, B: 'static>(
        &mut self,
    ) -> impl Iterator<Item = (Entity, &mut A, &B)> {
        let type_a = TypeId::of::<A>();
        let type_b = TypeId::of::<B>();
        let current_tick = self.current_tick;

        let mut results: Vec<(Entity, &mut A, &B)> = Vec::new();
        for archetype in &mut self.archetypes {
            if !archetype.has_component(type_a) || !archetype.has_component(type_b) {
                continue;
            }
            // `Entity` is `Copy`; cloning the (typically small) row list
            // up front avoids holding an immutable borrow of `archetype`
            // across the `column_pair_mut` call just below, which needs
            // `&mut archetype`.
            let entities: Vec<Entity> = archetype.entities().to_vec();
            let (column_a, column_b) = archetype
                .column_pair_mut(type_a, type_b)
                .expect("has_component confirmed both types are present above");
            let column_a = column_a
                .as_any_mut()
                .downcast_mut::<TypedColumn<A>>()
                .expect("archetype signature and columns must agree on stored types");
            let column_b = column_b
                .as_any()
                .downcast_ref::<TypedColumn<B>>()
                .expect("archetype signature and columns must agree on stored types");
            column_a.mark_all_changed(current_tick);
            for ((entity, a_ref), b_ref) in entities
                .into_iter()
                .zip(column_a.values_mut().iter_mut())
                .zip(column_b.values().iter())
            {
                results.push((entity, a_ref, b_ref));
            }
        }
        results.into_iter()
    }
}
