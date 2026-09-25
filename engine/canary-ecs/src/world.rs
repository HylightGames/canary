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

use crate::archetype::{Archetype, ArchetypeId};
use crate::column::{ColumnOps, Tick, TypedColumn};
use crate::component_identity::CanaryComponent;
use crate::entity::Entity;
use crate::error::EcsError;

/// Per-slot bookkeeping: whether it's currently occupied, the
/// generation to stamp on the next entity that occupies it, and (while
/// occupied) where its row currently lives.
///
/// `generation` is `u64` -- see the type-level docs on
/// [`crate::Entity`] for why.
#[derive(Default)]
struct Slot {
    generation: u64,
    alive: bool,
    location: Option<EntityLocation>,
}

/// A single resource's storage: its value plus the [`Tick`] it was last
/// written at, mirroring what [`crate::column::TypedColumn`] tracks
/// per-row for components -- see
/// `docs/architecture/execution-model.md#resources`.
struct ResourceEntry {
    value: Box<dyn Any + Send + Sync>,
    changed_tick: Tick,
}

/// Where one entity's row currently lives: which archetype, and which
/// row within it. Updated every time an entity's component set changes
/// (moving it to a different archetype) or another entity's
/// `swap_remove`-driven relocation lands on top of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EntityLocation {
    archetype: ArchetypeId,
    row: usize,
}

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
    slots: Vec<Slot>,
    free_indices: Vec<u32>,
    archetypes: Vec<Archetype>,
    /// Canonical `signature -> ArchetypeId` lookup. Keys are always
    /// sorted (see [`World::get_or_create_archetype`]), so two
    /// archetypes with the same component types, inserted in any order,
    /// always resolve to the same entry.
    archetype_index: HashMap<Vec<TypeId>, ArchetypeId>,
    /// The query cache: every archetype that contains a column of a
    /// given `TypeId`. Grows as new archetypes are created; archetypes
    /// are never removed once created (an emptied archetype is simply
    /// an archetype with zero rows, ready to be reused), so this never
    /// needs invalidating, only appending to.
    type_to_archetypes: HashMap<TypeId, Vec<ArchetypeId>>,
    /// The archetype for "no components" -- every entity passes through
    /// it at [`World::spawn`], before its first [`World::insert`].
    empty_archetype: ArchetypeId,
    current_tick: Tick,
    /// ADR 0010's proposed registry: stable schema identity -> the
    /// host's `TypeId` for that type, populated by
    /// [`World::register_component`].
    schema_registry: HashMap<&'static str, TypeId>,
    /// Globally-unique, engine-owned state addressed by type rather
    /// than by entity -- see
    /// `docs/architecture/execution-model.md#resources` (`v0.0.7`).
    /// A single-slot analogue of a component column: one value, one
    /// [`Tick`], per type, rather than a `Vec` of them.
    resources: HashMap<TypeId, ResourceEntry>,
    /// Cached count of currently-alive entities, so
    /// [`World::entity_count`] is O(1) instead of scanning every slot
    /// ever created (including long-dead ones) on every call.
    alive_count: usize,
}

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

    fn location_of(&self, entity: Entity) -> Option<EntityLocation> {
        let slot = self.slots.get(entity.index as usize)?;
        if slot.alive && slot.generation == entity.generation {
            slot.location
        } else {
            None
        }
    }

    fn set_location(&mut self, entity: Entity, location: EntityLocation) {
        self.slots[entity.index as usize].location = Some(location);
    }

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

    /// Shared iteration core for [`World::query`] and
    /// [`World::query_changed_since`]: every `(Entity, &T, Tick)` across
    /// every archetype the type-to-archetype cache says has a `T`
    /// column.
    fn iter_component<T: 'static>(&self) -> impl Iterator<Item = (Entity, &T, Tick)> {
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
    fn typed_column<T: 'static>(archetype: &Archetype) -> &TypedColumn<T> {
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

    /// The tick that will be stamped on the *next* write ([`World::insert`]
    /// moving an entity into a component for the first time, an
    /// [`World::insert`] overwrite, or a [`World::get_mut`] access).
    /// Callers doing change detection typically capture this value, do
    /// some work across one or more [`World::advance_tick`] boundaries,
    /// and later call [`World::query_changed_since`] with it.
    pub fn change_tick(&self) -> Tick {
        self.current_tick
    }

    /// Advances the world's tick. No automatic advancement exists:
    /// `canary-scheduler` (which runs real systems, including hierarchy
    /// propagation) deliberately does not own the tick, so callers doing
    /// their own change-detection bookkeeping call this at whatever
    /// granularity ("once per frame", "once per system") suits them.
    /// See `docs/architecture/core-runtime.md#threading--the-job-system`.
    pub fn advance_tick(&mut self) {
        self.current_tick.increment();
    }

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

    /// Registers `T`'s stable [`CanaryComponent::SCHEMA_ID`] against its
    /// host-internal `TypeId`, so the identity can later be resolved
    /// back via [`World::type_id_for_schema`] -- the "registry mapping
    /// [the stable] identity to the host's `TypeId` at runtime for the
    /// fast path" described in ADR 0010 (see the type-level docs on
    /// [`World`]). Idempotent when called more than once for the same
    /// `T`.
    ///
    /// A component only needs this if it might cross the plugin,
    /// replication, or marketplace boundary; purely internal components
    /// can skip it and just use `T: Send + Sync + 'static` with
    /// [`World::insert`]/[`World::get`]/[`World::query`] as normal.
    ///
    /// Returns [`EcsError::DuplicateSchemaId`] if a *different* type is
    /// already registered under `T::SCHEMA_ID` -- two component types
    /// racing for the same stable identity is exactly the
    /// cross-language collision this registry exists to catch early.
    pub fn register_component<T: CanaryComponent>(&mut self) -> Result<(), EcsError> {
        let type_id = TypeId::of::<T>();
        match self.schema_registry.get(T::SCHEMA_ID) {
            Some(&existing) if existing != type_id => {
                Err(EcsError::DuplicateSchemaId(T::SCHEMA_ID))
            }
            Some(_) => Ok(()),
            None => {
                self.schema_registry.insert(T::SCHEMA_ID, type_id);
                Ok(())
            }
        }
    }

    /// Resolves a stable [`CanaryComponent::SCHEMA_ID`] back to the
    /// host-internal `TypeId` it was [`World::register_component`]ed
    /// under, if any. This is the boundary a Tier A (WASM) plugin
    /// loader, a replication decoder, or a marketplace tool crosses
    /// through in the target design -- see the type-level docs on
    /// [`World`] and ADR 0010. As of `v0.0.3`, this is exactly what the
    /// real Tier A loader uses: `engine/canary-plugin-api/src/tier_a.rs`'s
    /// `HostState::get`/`set` resolve a WASM guest's `schema-id` string
    /// through this method before touching ECS storage. Replication and
    /// marketplace tooling remain future consumers of the same seam.
    pub fn type_id_for_schema(&self, schema_id: &str) -> Option<TypeId> {
        self.schema_registry.get(schema_id).copied()
    }

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
    fn get_or_create_archetype(
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

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Position {
        x: f32,
        y: f32,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Velocity {
        dx: f32,
        dy: f32,
    }

    /// `World` must be usable from a future work-stealing job system (see
    /// `docs/architecture/core-runtime.md#threading--the-job-system`) --
    /// this is a compile-time guard against ever regressing that, not a
    /// runtime behavior check. See
    /// `docs/reviews/2026-08-senior-architecture-review.md`, Finding 2.2.
    #[test]
    fn world_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<World>();
    }

    #[test]
    fn components_round_trip_through_insert_get_and_remove() {
        let mut world = World::new();
        let entity = world.spawn();

        assert!(world.get::<Position>(entity).is_none());

        world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
        assert_eq!(
            world.get::<Position>(entity),
            Some(&Position { x: 1.0, y: 2.0 })
        );

        world.get_mut::<Position>(entity).unwrap().x = 5.0;
        assert_eq!(
            world.get::<Position>(entity),
            Some(&Position { x: 5.0, y: 2.0 })
        );

        let removed = world.remove::<Position>(entity);
        assert_eq!(removed, Some(Position { x: 5.0, y: 2.0 }));
        assert!(world.get::<Position>(entity).is_none());
    }

    #[test]
    fn query_only_returns_alive_entities_with_the_component() {
        let mut world = World::new();

        let with_position = world.spawn();
        world
            .insert(with_position, Position { x: 0.0, y: 0.0 })
            .unwrap();

        let without_position = world.spawn();
        world
            .insert(without_position, Velocity { dx: 1.0, dy: 1.0 })
            .unwrap();

        let despawned_with_position = world.spawn();
        world
            .insert(despawned_with_position, Position { x: 9.0, y: 9.0 })
            .unwrap();
        world.despawn(despawned_with_position).unwrap();

        let found: Vec<Entity> = world.query::<Position>().map(|(e, _)| e).collect();
        assert_eq!(found, vec![with_position]);
    }

    #[test]
    fn despawn_removes_all_components_and_frees_the_slot() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();

        world.despawn(entity).unwrap();

        assert!(!world.is_alive(entity));
        assert_eq!(world.get::<Position>(entity), None);
        assert_eq!(
            world.despawn(entity),
            Err(EcsError::StaleOrUnknownEntity),
            "despawning an already-despawned entity should error, not panic"
        );
    }

    #[test]
    fn a_recycled_slot_does_not_alias_the_old_handle() {
        let mut world = World::new();
        let first = world.spawn();
        world.despawn(first).unwrap();

        let second = world.spawn();

        // The slot index may well be reused...
        assert_eq!(first.index(), second.index());
        // ...but the generation must differ, so `first` is never mistaken
        // for `second`.
        assert_ne!(first.generation(), second.generation());
        assert!(!world.is_alive(first));
        assert!(world.is_alive(second));
    }

    // -- Archetype transitions -------------------------------------------

    #[test]
    fn insert_into_a_new_archetype_preserves_other_components() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
        world.insert(entity, Velocity { dx: 3.0, dy: 4.0 }).unwrap();

        assert_eq!(
            world.get::<Position>(entity),
            Some(&Position { x: 1.0, y: 2.0 })
        );
        assert_eq!(
            world.get::<Velocity>(entity),
            Some(&Velocity { dx: 3.0, dy: 4.0 })
        );
    }

    #[test]
    fn inserting_an_already_present_component_type_overwrites_in_place() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();
        world.insert(entity, Position { x: 9.0, y: 9.0 }).unwrap();

        assert_eq!(
            world.get::<Position>(entity),
            Some(&Position { x: 9.0, y: 9.0 })
        );
    }

    #[test]
    fn remove_moves_entity_to_a_smaller_archetype_and_preserves_remaining_components() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
        world.insert(entity, Velocity { dx: 3.0, dy: 4.0 }).unwrap();

        let removed = world.remove::<Velocity>(entity);

        assert_eq!(removed, Some(Velocity { dx: 3.0, dy: 4.0 }));
        assert_eq!(
            world.get::<Position>(entity),
            Some(&Position { x: 1.0, y: 2.0 })
        );
        assert_eq!(world.get::<Velocity>(entity), None);
    }

    #[test]
    fn removing_an_absent_component_type_returns_none_and_does_not_move_the_entity() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();

        assert_eq!(world.remove::<Velocity>(entity), None);
        assert_eq!(
            world.get::<Position>(entity),
            Some(&Position { x: 1.0, y: 1.0 })
        );
    }

    #[test]
    fn removing_from_a_stale_entity_returns_none_like_an_absent_component() {
        // Pins the documented `remove` contract: idempotent removal
        // answers "nothing to take" identically for dead handles and
        // missing components. Liveness checks belong to `is_alive`,
        // not to this return value.
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();
        world.despawn(entity).unwrap();

        assert!(!world.is_alive(entity));
        assert_eq!(world.remove::<Position>(entity), None);
    }

    #[test]
    fn query_spans_multiple_archetypes() {
        let mut world = World::new();

        let just_position = world.spawn();
        world
            .insert(just_position, Position { x: 1.0, y: 1.0 })
            .unwrap();

        let both = world.spawn();
        world.insert(both, Position { x: 2.0, y: 2.0 }).unwrap();
        world.insert(both, Velocity { dx: 0.0, dy: 0.0 }).unwrap();

        let just_velocity = world.spawn();
        world
            .insert(just_velocity, Velocity { dx: 9.0, dy: 9.0 })
            .unwrap();

        let mut found: Vec<Entity> = world.query::<Position>().map(|(e, _)| e).collect();
        found.sort_by_key(|e| e.index());
        let mut expected = vec![just_position, both];
        expected.sort_by_key(|e| e.index());
        assert_eq!(found, expected);
    }

    #[test]
    fn query2_only_returns_entities_with_both_components() {
        let mut world = World::new();

        let just_position = world.spawn();
        world
            .insert(just_position, Position { x: 1.0, y: 1.0 })
            .unwrap();

        let both = world.spawn();
        world.insert(both, Position { x: 2.0, y: 2.0 }).unwrap();
        world.insert(both, Velocity { dx: 3.0, dy: 3.0 }).unwrap();

        let just_velocity = world.spawn();
        world
            .insert(just_velocity, Velocity { dx: 9.0, dy: 9.0 })
            .unwrap();

        let found: Vec<(Entity, Position, Velocity)> = world
            .query2::<Position, Velocity>()
            .map(|(e, p, v)| (e, *p, *v))
            .collect();

        assert_eq!(
            found,
            vec![(
                both,
                Position { x: 2.0, y: 2.0 },
                Velocity { dx: 3.0, dy: 3.0 }
            )]
        );
    }

    #[test]
    fn query2_is_order_independent_in_the_type_parameters() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position { x: 1.0, y: 2.0 }).unwrap();
        world.insert(e, Velocity { dx: 3.0, dy: 4.0 }).unwrap();

        let a: Vec<Entity> = world
            .query2::<Position, Velocity>()
            .map(|(e, _, _)| e)
            .collect();
        let b: Vec<Entity> = world
            .query2::<Velocity, Position>()
            .map(|(e, _, _)| e)
            .collect();
        assert_eq!(a, vec![e]);
        assert_eq!(b, vec![e]);
    }

    #[test]
    fn query3_is_order_independent_in_the_type_parameters() {
        // `query2` pins this; `query3` intersects three archetype sets
        // and deserves the same guarantee rather than inheriting it by
        // assumption.
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position { x: 1.0, y: 2.0 }).unwrap();
        world.insert(e, Velocity { dx: 3.0, dy: 4.0 }).unwrap();
        world.insert(e, Health { hp: 100.0 }).unwrap();

        let a: Vec<Entity> = world
            .query3::<Position, Velocity, Health>()
            .map(|(e, _, _, _)| e)
            .collect();
        let b: Vec<Entity> = world
            .query3::<Health, Velocity, Position>()
            .map(|(e, _, _, _)| e)
            .collect();
        assert_eq!(a, vec![e]);
        assert_eq!(b, vec![e]);
    }

    #[test]
    fn entity_count_tracks_spawns_and_despawns() {
        let mut world = World::new();
        assert_eq!(world.entity_count(), 0);
        let a = world.spawn();
        let b = world.spawn();
        assert_eq!(world.entity_count(), 2);
        world.despawn(a).unwrap();
        assert_eq!(world.entity_count(), 1);
        world.despawn(b).unwrap();
        assert_eq!(world.entity_count(), 0);
        // Recycled slots must not double-count: despawning freed the
        // slot, respawning reuses it, and the count tracks live
        // entities, not slots ever created.
        let _ = world.spawn();
        assert_eq!(world.entity_count(), 1);
    }

    #[test]
    #[should_panic(expected = "set_erased: internal invariant violated")]
    fn set_erased_panics_on_a_concrete_type_mismatch() {
        // The `# Panics` contract on `World::set_erased`: a value whose
        // concrete type doesn't match `type_id` is a host-side bug and
        // fails loudly rather than corrupting the column.
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();
        world.set_erased(
            entity,
            TypeId::of::<Position>(),
            Box::new(Velocity { dx: 1.0, dy: 1.0 }),
        );
    }

    #[test]
    fn merely_calling_query2_mut_dirties_matched_rows() {
        // `query2_mut` stamps ticks eagerly at call time (it collects
        // first), even if the caller never consumes the iterator — a
        // conservative contract this test pins so no later refactor can
        // silently make it lazy without updating the docs.
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();
        world.insert(entity, Velocity { dx: 1.0, dy: 1.0 }).unwrap();
        world.advance_tick();
        let baseline = world.change_tick();
        world.advance_tick();

        drop(world.query2_mut::<Position, Velocity>());

        let changed: Vec<Entity> = world
            .query_changed_since::<Position>(baseline)
            .map(|(e, _)| e)
            .collect();
        assert_eq!(
            changed,
            vec![entity],
            "a query2_mut call must dirty ticks even when unconsumed"
        );
    }

    #[test]
    fn query3_requires_all_three_components() {
        let mut world = World::new();

        let all_three = world.spawn();
        world
            .insert(all_three, Position { x: 1.0, y: 1.0 })
            .unwrap();
        world
            .insert(all_three, Velocity { dx: 2.0, dy: 2.0 })
            .unwrap();
        world.insert(all_three, Health { hp: 100.0 }).unwrap();

        let missing_health = world.spawn();
        world
            .insert(missing_health, Position { x: 9.0, y: 9.0 })
            .unwrap();
        world
            .insert(missing_health, Velocity { dx: 9.0, dy: 9.0 })
            .unwrap();

        let found: Vec<Entity> = world
            .query3::<Position, Velocity, Health>()
            .map(|(e, _, _, _)| e)
            .collect();
        assert_eq!(found, vec![all_three]);
    }

    #[test]
    fn query2_mut_writes_through_and_leaves_the_shared_component_untouched() {
        let mut world = World::new();

        let both = world.spawn();
        world.insert(both, Position { x: 0.0, y: 0.0 }).unwrap();
        world.insert(both, Velocity { dx: 1.0, dy: 2.0 }).unwrap();

        let just_position = world.spawn();
        world
            .insert(just_position, Position { x: 5.0, y: 5.0 })
            .unwrap();

        for (_, position, velocity) in world.query2_mut::<Position, Velocity>() {
            position.x += velocity.dx;
            position.y += velocity.dy;
        }

        assert_eq!(
            world.get::<Position>(both),
            Some(&Position { x: 1.0, y: 2.0 })
        );
        assert_eq!(
            world.get::<Velocity>(both),
            Some(&Velocity { dx: 1.0, dy: 2.0 })
        );
        // Untouched: not yielded by query2_mut (missing Velocity), so
        // must be completely unaffected by the loop above.
        assert_eq!(
            world.get::<Position>(just_position),
            Some(&Position { x: 5.0, y: 5.0 })
        );
    }

    #[test]
    fn query2_mut_marks_only_the_mutable_component_as_changed() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position { x: 0.0, y: 0.0 }).unwrap();
        world.insert(e, Velocity { dx: 1.0, dy: 1.0 }).unwrap();

        let since = world.change_tick();
        world.advance_tick();

        for (_, position, _velocity) in world.query2_mut::<Position, Velocity>() {
            position.x += 1.0;
        }

        let position_changed = world
            .query_changed_since::<Position>(since)
            .any(|(entity, _)| entity == e);
        let velocity_changed = world
            .query_changed_since::<Velocity>(since)
            .any(|(entity, _)| entity == e);
        assert!(position_changed);
        assert!(!velocity_changed);
    }

    #[test]
    fn query2_mut_spans_multiple_archetypes() {
        let mut world = World::new();

        let e1 = world.spawn();
        world.insert(e1, Position { x: 0.0, y: 0.0 }).unwrap();
        world.insert(e1, Velocity { dx: 1.0, dy: 0.0 }).unwrap();

        // A different archetype (Position + Velocity + Health), still
        // matched by a query2::<Position, Velocity> -- extra components
        // beyond the two queried for must not exclude an entity.
        let e2 = world.spawn();
        world.insert(e2, Position { x: 0.0, y: 0.0 }).unwrap();
        world.insert(e2, Velocity { dx: 2.0, dy: 0.0 }).unwrap();
        world.insert(e2, Health { hp: 50.0 }).unwrap();

        for (_, position, velocity) in world.query2_mut::<Position, Velocity>() {
            position.x += velocity.dx;
        }

        assert_eq!(
            world.get::<Position>(e1),
            Some(&Position { x: 1.0, y: 0.0 })
        );
        assert_eq!(
            world.get::<Position>(e2),
            Some(&Position { x: 2.0, y: 0.0 })
        );
    }

    #[test]
    #[should_panic(expected = "must name different component types")]
    fn query2_mut_with_the_same_type_twice_panics_instead_of_aliasing() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position { x: 0.0, y: 0.0 }).unwrap();

        let _ = world.query2_mut::<Position, Position>().count();
    }

    /// The trickiest part of a `swap_remove`-based archetype move: when
    /// the row leaving isn't the archetype's last row, some *other*
    /// entity gets relocated as a side effect, and its recorded
    /// location must be fixed up -- or a later lookup for it either
    /// panics (stale row index) or silently returns the wrong data.
    #[test]
    fn archetype_transition_fixes_up_the_swapped_entitys_location() {
        let mut world = World::new();

        let e0 = world.spawn();
        world.insert(e0, Position { x: 0.0, y: 0.0 }).unwrap();
        let e1 = world.spawn();
        world.insert(e1, Position { x: 1.0, y: 0.0 }).unwrap();
        let e2 = world.spawn();
        world.insert(e2, Position { x: 2.0, y: 0.0 }).unwrap();
        // e0, e1, e2 now all share one [Position] archetype, in that row order.

        // Moving e0 (not the archetype's last row) out forces a
        // swap-remove there: e2, the archetype's last row, slides into
        // e0's old slot.
        world.insert(e0, Velocity { dx: 9.0, dy: 9.0 }).unwrap();

        assert_eq!(
            world.get::<Position>(e0),
            Some(&Position { x: 0.0, y: 0.0 })
        );
        assert_eq!(
            world.get::<Velocity>(e0),
            Some(&Velocity { dx: 9.0, dy: 9.0 })
        );
        // e1 was never the row that got swapped -- its own recorded
        // location shouldn't have needed to change at all.
        assert_eq!(
            world.get::<Position>(e1),
            Some(&Position { x: 1.0, y: 0.0 })
        );
        // e2 got relocated; if that relocation wasn't recorded, this
        // reads either stale/wrong data or a row that no longer holds it.
        assert_eq!(
            world.get::<Position>(e2),
            Some(&Position { x: 2.0, y: 0.0 })
        );
    }

    #[test]
    fn despawn_fixes_up_the_swapped_entitys_location() {
        let mut world = World::new();

        let e0 = world.spawn();
        world.insert(e0, Position { x: 0.0, y: 0.0 }).unwrap();
        let e1 = world.spawn();
        world.insert(e1, Position { x: 1.0, y: 0.0 }).unwrap();
        let e2 = world.spawn();
        world.insert(e2, Position { x: 2.0, y: 0.0 }).unwrap();

        world.despawn(e0).unwrap();

        assert!(!world.is_alive(e0));
        assert_eq!(
            world.get::<Position>(e1),
            Some(&Position { x: 1.0, y: 0.0 })
        );
        assert_eq!(
            world.get::<Position>(e2),
            Some(&Position { x: 2.0, y: 0.0 })
        );
    }

    // -- Change detection --------------------------------------------------

    #[test]
    fn get_mut_marks_the_component_as_changed() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 0.0, y: 0.0 }).unwrap();

        let before_mutation = world.change_tick();
        world.advance_tick();
        world.get_mut::<Position>(entity).unwrap().x = 5.0;
        let after_mutation = world.change_tick();

        let changed_since_before: Vec<Entity> = world
            .query_changed_since::<Position>(before_mutation)
            .map(|(e, _)| e)
            .collect();
        assert_eq!(changed_since_before, vec![entity]);

        let changed_since_after: Vec<Entity> = world
            .query_changed_since::<Position>(after_mutation)
            .map(|(e, _)| e)
            .collect();
        assert!(changed_since_after.is_empty());
    }

    #[test]
    fn moving_to_a_new_archetype_preserves_an_untouched_components_change_tick() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 0.0, y: 0.0 }).unwrap();
        let position_written_at = world.change_tick();

        world.advance_tick();
        // Inserting Velocity moves `entity` to a new archetype; Position's
        // column data has to be carried over through that move, and its
        // change tick must come with it, unmodified.
        world.insert(entity, Velocity { dx: 1.0, dy: 1.0 }).unwrap();

        let changed: Vec<Entity> = world
            .query_changed_since::<Position>(position_written_at)
            .map(|(e, _)| e)
            .collect();
        assert!(
            changed.is_empty(),
            "moving archetypes because of an unrelated Velocity insert must not mark Position as freshly changed"
        );
    }

    #[test]
    fn from_raw_parts_round_trips_a_real_entity_and_safely_rejects_a_mismatched_one() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();

        let reconstructed = Entity::from_raw_parts(entity.index(), entity.generation());
        assert_eq!(reconstructed, entity);
        assert!(world.is_alive(reconstructed));
        assert_eq!(
            world.get::<Position>(reconstructed),
            Some(&Position { x: 1.0, y: 1.0 })
        );

        // A forged handle for the same slot but the wrong generation
        // must be safely rejected, not treated as an unchecked
        // precondition -- see Entity::from_raw_parts's doc comment.
        let forged = Entity::from_raw_parts(entity.index(), entity.generation().wrapping_add(1));
        assert!(!world.is_alive(forged));
        assert_eq!(world.get::<Position>(forged), None);
    }

    // -- Type-erased access (the Tier A "host adapter") --------------------

    #[test]
    fn get_erased_and_set_erased_round_trip_through_a_real_type_id() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
        let type_id = TypeId::of::<Position>();

        let read_back = world
            .get_erased(entity, type_id)
            .and_then(|value| value.downcast_ref::<Position>())
            .copied();
        assert_eq!(read_back, Some(Position { x: 1.0, y: 2.0 }));

        let overwrote = world.set_erased(entity, type_id, Box::new(Position { x: 9.0, y: 9.0 }));
        assert!(overwrote);
        assert_eq!(
            world.get::<Position>(entity),
            Some(&Position { x: 9.0, y: 9.0 }),
            "set_erased should be visible through the ordinary typed get too"
        );
    }

    #[test]
    fn get_erased_returns_none_for_a_component_type_the_entity_lacks() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();

        assert!(world.get_erased(entity, TypeId::of::<Velocity>()).is_none());
    }

    #[test]
    fn set_erased_returns_false_and_changes_nothing_for_a_component_type_the_entity_lacks() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 1.0, y: 1.0 }).unwrap();

        let changed = world.set_erased(
            entity,
            TypeId::of::<Velocity>(),
            Box::new(Velocity { dx: 1.0, dy: 1.0 }),
        );

        assert!(
            !changed,
            "set_erased must not insert a new component type -- see World::set_erased's docs"
        );
        assert!(world.get::<Velocity>(entity).is_none());
        assert_eq!(
            world.get::<Position>(entity),
            Some(&Position { x: 1.0, y: 1.0 }),
            "the failed set_erased attempt must not have disturbed Position either"
        );
    }

    #[test]
    fn has_component_erased_matches_real_component_presence() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 0.0, y: 0.0 }).unwrap();

        assert!(world.has_component_erased(entity, TypeId::of::<Position>()));
        assert!(!world.has_component_erased(entity, TypeId::of::<Velocity>()));
    }

    #[test]
    fn set_erased_marks_the_component_as_changed() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Position { x: 0.0, y: 0.0 }).unwrap();

        let baseline = world.change_tick();
        world.advance_tick();
        world.set_erased(
            entity,
            TypeId::of::<Position>(),
            Box::new(Position { x: 5.0, y: 5.0 }),
        );

        let changed: Vec<Entity> = world
            .query_changed_since::<Position>(baseline)
            .map(|(e, _)| e)
            .collect();
        assert_eq!(changed, vec![entity]);
    }

    // -- Component identity (ADR 0010 first cut) ---------------------------

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Health {
        hp: f32,
    }

    impl CanaryComponent for Health {
        const SCHEMA_ID: &'static str = "canary-ecs-tests:health@1";
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Shield {
        absorption: f32,
    }

    impl CanaryComponent for Shield {
        // Deliberately colliding with `Health`'s schema id.
        const SCHEMA_ID: &'static str = "canary-ecs-tests:health@1";
    }

    #[test]
    fn register_component_resolves_schema_id_to_type_id() {
        let mut world = World::new();
        world.register_component::<Health>().unwrap();

        assert_eq!(
            world.type_id_for_schema("canary-ecs-tests:health@1"),
            Some(TypeId::of::<Health>())
        );
        assert_eq!(world.type_id_for_schema("no-such-schema"), None);
    }

    #[test]
    fn registering_a_different_type_under_the_same_schema_id_errors() {
        let mut world = World::new();
        world.register_component::<Health>().unwrap();

        assert_eq!(
            world.register_component::<Shield>(),
            Err(EcsError::DuplicateSchemaId("canary-ecs-tests:health@1"))
        );
    }

    #[test]
    fn registering_the_same_type_twice_is_idempotent() {
        let mut world = World::new();
        world.register_component::<Health>().unwrap();
        assert_eq!(world.register_component::<Health>(), Ok(()));
    }

    proptest::proptest! {
        #[test]
        fn despawned_entities_never_alias_a_later_spawn(spawn_count in 1usize..64) {
            let mut world = World::new();
            let entities: Vec<Entity> = (0..spawn_count).map(|_| world.spawn()).collect();

            // Despawn every entity, then spawn the same number again, and
            // check that none of the original handles are ever reported
            // alive again, even though every slot index gets recycled.
            for &e in &entities {
                world.despawn(e).unwrap();
            }
            let respawned: Vec<Entity> = (0..spawn_count).map(|_| world.spawn()).collect();

            for &old in &entities {
                proptest::prop_assert!(!world.is_alive(old));
            }
            for &new in &respawned {
                proptest::prop_assert!(world.is_alive(new));
            }
        }
    }

    /// One randomized operation in a scripted insert/remove/despawn
    /// sequence, targeting an entity by index into the test's own
    /// `entities` vec (out-of-range indices are simply no-ops -- see
    /// `op_strategy`'s small ranges, which make in-range indices common).
    #[derive(Debug, Clone)]
    enum Op {
        Spawn,
        InsertPosition(usize),
        InsertVelocity(usize),
        RemovePosition(usize),
        RemoveVelocity(usize),
        Despawn(usize),
    }

    fn op_strategy() -> impl proptest::strategy::Strategy<Value = Op> {
        use proptest::prelude::*;
        prop_oneof![
            Just(Op::Spawn),
            (0usize..8).prop_map(Op::InsertPosition),
            (0usize..8).prop_map(Op::InsertVelocity),
            (0usize..8).prop_map(Op::RemovePosition),
            (0usize..8).prop_map(Op::RemoveVelocity),
            (0usize..8).prop_map(Op::Despawn),
        ]
    }

    proptest::proptest! {
        /// The core round-trip invariant named in
        /// `docs/development/coding-standards.md` ("components round-trip
        /// through insert/query"), exercised over arbitrary sequences of
        /// insert/remove/despawn across multiple entities and component
        /// types -- the combination most likely to expose an archetype
        /// bookkeeping bug (a bad row index after a `swap_remove`, a
        /// value/tick pair that didn't travel together) that a handful of
        /// hand-picked example tests could miss.
        #[test]
        fn components_round_trip_through_arbitrary_insert_remove_sequences(
            ops in proptest::collection::vec(op_strategy(), 1..60)
        ) {
            let mut world = World::new();
            let mut entities: Vec<Entity> = Vec::new();
            let mut expected_position: Vec<bool> = Vec::new();
            let mut expected_velocity: Vec<bool> = Vec::new();

            for op in ops {
                match op {
                    Op::Spawn => {
                        entities.push(world.spawn());
                        expected_position.push(false);
                        expected_velocity.push(false);
                    }
                    Op::InsertPosition(i) => {
                        if let Some(&e) = entities.get(i) {
                            if world.insert(e, Position { x: i as f32, y: 0.0 }).is_ok() {
                                expected_position[i] = true;
                            }
                        }
                    }
                    Op::InsertVelocity(i) => {
                        if let Some(&e) = entities.get(i) {
                            if world.insert(e, Velocity { dx: i as f32, dy: 0.0 }).is_ok() {
                                expected_velocity[i] = true;
                            }
                        }
                    }
                    Op::RemovePosition(i) => {
                        if let Some(&e) = entities.get(i) {
                            world.remove::<Position>(e);
                            expected_position[i] = false;
                        }
                    }
                    Op::RemoveVelocity(i) => {
                        if let Some(&e) = entities.get(i) {
                            world.remove::<Velocity>(e);
                            expected_velocity[i] = false;
                        }
                    }
                    Op::Despawn(i) => {
                        if let Some(&e) = entities.get(i) {
                            let _ = world.despawn(e);
                        }
                    }
                }
            }

            for (i, &e) in entities.iter().enumerate() {
                if world.is_alive(e) {
                    proptest::prop_assert_eq!(world.get::<Position>(e).is_some(), expected_position[i]);
                    proptest::prop_assert_eq!(world.get::<Velocity>(e).is_some(), expected_velocity[i]);
                } else {
                    proptest::prop_assert!(world.get::<Position>(e).is_none());
                    proptest::prop_assert!(world.get::<Velocity>(e).is_none());
                }
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct FrameCount(u64);

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct GravityConstant(f32);

    #[test]
    fn resources_round_trip_through_insert_get_and_remove() {
        let mut world = World::new();

        assert!(world.resource::<FrameCount>().is_none());
        assert!(!world.contains_resource::<FrameCount>());

        world.insert_resource(FrameCount(0));
        assert!(world.contains_resource::<FrameCount>());
        assert_eq!(world.resource::<FrameCount>(), Some(&FrameCount(0)));

        world.resource_mut::<FrameCount>().unwrap().0 = 5;
        assert_eq!(world.resource::<FrameCount>(), Some(&FrameCount(5)));

        assert_eq!(world.remove_resource::<FrameCount>(), Some(FrameCount(5)));
        assert!(world.resource::<FrameCount>().is_none());
    }

    #[test]
    fn inserting_a_resource_of_an_already_present_type_overwrites() {
        let mut world = World::new();
        world.insert_resource(GravityConstant(9.8));
        world.insert_resource(GravityConstant(3.7));
        assert_eq!(
            world.resource::<GravityConstant>(),
            Some(&GravityConstant(3.7))
        );
    }

    #[test]
    fn distinct_resource_types_do_not_interfere() {
        let mut world = World::new();
        world.insert_resource(FrameCount(1));
        world.insert_resource(GravityConstant(9.8));

        assert_eq!(world.resource::<FrameCount>(), Some(&FrameCount(1)));
        assert_eq!(
            world.resource::<GravityConstant>(),
            Some(&GravityConstant(9.8))
        );

        world.remove_resource::<FrameCount>();
        assert!(world.resource::<FrameCount>().is_none());
        assert_eq!(
            world.resource::<GravityConstant>(),
            Some(&GravityConstant(9.8))
        );
    }

    #[test]
    fn resource_mut_marks_the_resource_as_changed() {
        let mut world = World::new();
        world.insert_resource(FrameCount(0));

        let since = world.change_tick();
        assert!(!world.resource_changed_since::<FrameCount>(since));

        world.advance_tick();
        world.resource_mut::<FrameCount>().unwrap().0 += 1;
        assert!(world.resource_changed_since::<FrameCount>(since));
    }

    #[test]
    fn resource_changed_since_is_false_for_a_resource_that_was_never_inserted() {
        let world = World::new();
        assert!(!world.resource_changed_since::<FrameCount>(Tick::default()));
    }
}
