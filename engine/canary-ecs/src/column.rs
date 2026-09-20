// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::any::Any;

/// A monotonically increasing point in a [`crate::World`]'s history, used
/// to detect whether a component has been written since some earlier
/// point in time -- see [`crate::World::change_tick`],
/// [`crate::World::advance_tick`], and [`crate::World::query_changed_since`].
/// This is the "first-class query filter" named in
/// `docs/architecture/core-runtime.md#ecs-architecture`.
///
/// `Tick(0)` is never observed as a "changed at" value in practice --
/// [`crate::World::advance_tick`] is called before the first write in
/// any correctly-written system loop -- but nothing here enforces that;
/// a component written before the first [`crate::World::advance_tick`]
/// call is simply never reported as "changed since `Tick(0)`" by
/// [`crate::World::query_changed_since`], which is the conservative,
/// safe direction for that edge case to fail in.
///
/// `Tick` is a `u64`, not a `u32`, on purpose -- the same reasoning as
/// [`crate::Entity::generation`] (see that type's docs), just found
/// later: a plain `PartialOrd`/`Ord`-derived `tick > since` comparison
/// (see [`crate::World::query_changed_since`]) is only correct if `Tick`
/// never wraps around during a `World`'s lifetime. At `u32`, a
/// long-lived server calling [`crate::World::advance_tick`] once per
/// frame at 60Hz wraps in about 2.3 years of continuous uptime -- past
/// that point, a stale `since` value captured before the wrap would
/// compare *greater* than a genuinely more recent tick, silently hiding
/// real changes from [`crate::World::query_changed_since`] rather than
/// erroring. Found during the September 2026 external architecture
/// review triage (`docs/reviews/triage/2026-09-review-triage.md`, review #1
/// item 3); `u64` doesn't eliminate the theoretical possibility, but --
/// like `Entity::generation` -- moves it from "plausible over a real
/// long-lived server's uptime" to "not reachable by any realistic
/// amount of runtime."
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Tick(u64);

impl Tick {
    /// Advances to the next tick. The inner representation stays
    /// private even within the crate -- [`crate::World::advance_tick`]
    /// goes through this method rather than reaching into a `pub(crate)`
    /// field, so `Tick` only ever changes by exactly one defined
    /// operation.
    pub(crate) fn increment(&mut self) {
        // `wrapping_add`, matching `Entity::generation`'s own recycle
        // path (`World::despawn`): `u64` makes a wrap unreachable in
        // practice (see this type's docs), but release and debug builds
        // must agree on what happens if one ever occurs -- a `+=`
        // here would panic in debug while silently wrapping in
        // release, which is exactly the kind of configuration-
        // dependent behavior this crate avoids elsewhere.
        self.0 = self.0.wrapping_add(1);
    }
}

/// A single column's worth of type-erased storage operations, so
/// [`crate::archetype::Archetype`] can hold columns of different
/// concrete component types side by side in one map and move rows
/// between archetypes generically.
///
/// This exists so `World` never needs a `TypeId -> constructor`
/// registry: a new archetype's columns are always built either by
/// asking an existing column of the same type for
/// [`ColumnOps::new_same_type`], or -- for the one genuinely new type in
/// a [`crate::World::insert`] call -- constructed directly by the
/// caller, which already has the concrete type in hand. Every component
/// type, built-in or third-party, goes through exactly this path; see
/// the "no privileged built-ins" architectural value in
/// `docs/vision/design-philosophy.md`.
pub(crate) trait ColumnOps: Send + Sync {
    /// An empty column of the same concrete element type as `self`.
    fn new_same_type(&self) -> Box<dyn ColumnOps>;

    /// Removes row `row` (via `Vec::swap_remove`, so the archetype's
    /// last row moves into its place -- callers are responsible for
    /// updating whichever entity that was), returning the value and its
    /// recorded [`Tick`], both type-erased, for reinsertion into a
    /// destination column via [`ColumnOps::push_any`].
    fn swap_remove_to_any(&mut self, row: usize) -> (Box<dyn Any + Send + Sync>, Tick);

    /// Appends a type-erased value and the [`Tick`] it was previously
    /// recorded under, preserving that tick rather than stamping a new
    /// one -- moving to a different archetype because an *unrelated*
    /// component was added or removed must not look like a write to
    /// this one. Panics if `value`'s concrete type doesn't match this
    /// column's element type: every call site constructs `value` from a
    /// column [`ColumnOps::new_same_type`] agrees with, so a mismatch
    /// here is an internal invariant violation, not a user-facing error
    /// -- see the panic-vs-`Result` convention in
    /// `docs/development/coding-standards.md`.
    fn push_any(&mut self, value: Box<dyn Any + Send + Sync>, tick: Tick);

    /// A type-erased reference to row `row`'s value, for a caller that
    /// only has a runtime `TypeId` (e.g. resolved via
    /// [`crate::World::type_id_for_schema`]), not a concrete type at
    /// the call site -- the read half of the "host adapter" ADR 0010
    /// named but didn't itself build; see
    /// `docs/roadmap/v0.0.3-roadmap.md`'s component-data-ABI scope item.
    fn get_erased(&self, row: usize) -> &(dyn Any + Send + Sync);

    /// Overwrites row `row` in place with a type-erased value, stamping
    /// `tick` as its new changed-tick -- the write half of the same
    /// "host adapter". Does not change the column's length (no insert,
    /// no archetype transition); see
    /// [`crate::World::set_erased`] for why that's a deliberate first-cut
    /// boundary. Panics on a type mismatch, per the same convention as
    /// [`ColumnOps::push_any`].
    fn set_erased(&mut self, row: usize, value: Box<dyn Any + Send + Sync>, tick: Tick);

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// The concrete, contiguous storage backing one component type within
/// one archetype: a plain `Vec<T>` plus a row-aligned `Vec<Tick>`
/// recording when each row was last written. Densely packed and
/// index-parallel to [`crate::archetype::Archetype::entities`] -- this
/// is what makes iterating "every entity with `T`" a linear scan over
/// tightly packed memory rather than a chase through scattered
/// allocations, per `docs/architecture/core-runtime.md#ecs-architecture`.
pub(crate) struct TypedColumn<T> {
    values: Vec<T>,
    changed_ticks: Vec<Tick>,
}

impl<T: 'static> TypedColumn<T> {
    pub(crate) fn new() -> Self {
        Self {
            values: Vec::new(),
            changed_ticks: Vec::new(),
        }
    }

    pub(crate) fn values(&self) -> &[T] {
        &self.values
    }

    /// A mutable slice over every row's value -- used by
    /// [`crate::World::query2_mut`] to yield disjoint `&mut T` per row
    /// via a genuine `IterMut` rather than repeated indexed
    /// [`TypedColumn::get_mut`] calls, which the borrow checker can't
    /// verify are non-overlapping across loop iterations the way a real
    /// slice iterator can. See
    /// `docs/architecture/execution-model.md#queries` for why this
    /// column-level split is where the actual complexity lives, not
    /// here.
    pub(crate) fn values_mut(&mut self) -> &mut [T] {
        &mut self.values
    }

    pub(crate) fn changed_ticks(&self) -> &[Tick] {
        &self.changed_ticks
    }

    /// Overwrites row `row` in place (same archetype, no row move --
    /// used by [`crate::World::insert`] when the entity already has a
    /// component of this type) and stamps `tick` as its new
    /// changed-tick.
    pub(crate) fn set(&mut self, row: usize, value: T, tick: Tick) {
        self.values[row] = value;
        self.changed_ticks[row] = tick;
    }

    pub(crate) fn get_mut(&mut self, row: usize) -> Option<&mut T> {
        self.values.get_mut(row)
    }

    /// Stamps row `row`'s changed-tick without touching its value --
    /// used by [`crate::World::get_mut`], which hands out a `&mut T`
    /// and must conservatively assume the caller writes through it.
    pub(crate) fn mark_changed(&mut self, row: usize, tick: Tick) {
        self.changed_ticks[row] = tick;
    }

    /// Stamps *every* row's changed-tick at once -- the whole-column
    /// analogue of [`TypedColumn::mark_changed`], used by
    /// [`crate::World::query2_mut`], which hands out `&mut T` for every
    /// row it yields and, like [`crate::World::get_mut`], conservatively
    /// assumes each one is written through.
    pub(crate) fn mark_all_changed(&mut self, tick: Tick) {
        self.changed_ticks.fill(tick);
    }
}

impl<T: Send + Sync + 'static> ColumnOps for TypedColumn<T> {
    fn new_same_type(&self) -> Box<dyn ColumnOps> {
        Box::new(TypedColumn::<T>::new())
    }

    fn swap_remove_to_any(&mut self, row: usize) -> (Box<dyn Any + Send + Sync>, Tick) {
        let tick = self.changed_ticks.swap_remove(row);
        let value = self.values.swap_remove(row);
        (Box::new(value), tick)
    }

    fn push_any(&mut self, value: Box<dyn Any + Send + Sync>, tick: Tick) {
        let value = *value
            .downcast::<T>()
            .expect("push_any: internal archetype invariant violated (column/value type mismatch)");
        self.values.push(value);
        self.changed_ticks.push(tick);
    }

    fn get_erased(&self, row: usize) -> &(dyn Any + Send + Sync) {
        &self.values[row]
    }

    fn set_erased(&mut self, row: usize, value: Box<dyn Any + Send + Sync>, tick: Tick) {
        let value = *value
            .downcast::<T>()
            .expect("set_erased: internal invariant violated (column/value type mismatch)");
        self.set(row, value, tick);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
