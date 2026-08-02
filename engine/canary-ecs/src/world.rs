use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::entity::Entity;
use crate::error::EcsError;

/// Per-slot bookkeeping: whether it's currently occupied, and the
/// generation to stamp on the next entity that occupies it.
#[derive(Default)]
struct Slot {
    generation: u32,
    alive: bool,
}

/// The ECS World: owns entities and their components.
///
/// **v0.0.1-pre1 placeholder**: components are stored per-type in a
/// `HashMap` keyed by entity slot index, *not* in archetype tables, and
/// [`World::query`] does a linear scan rather than using a cached
/// archetype match. See
/// `docs/architecture/core-runtime.md#ecs-architecture` for the target
/// design and why this scope was chosen for the first buildable
/// milestone.
#[derive(Default)]
pub struct World {
    slots: Vec<Slot>,
    free_indices: Vec<u32>,
    components: HashMap<TypeId, HashMap<u32, Box<dyn Any>>>,
}

impl World {
    /// Creates an empty `World`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawns a new entity with no components and returns its handle.
    ///
    /// Reuses a despawned slot's index when one is available, bumping
    /// that slot's generation so old handles into it remain detectably
    /// stale (see [`World::is_alive`]).
    pub fn spawn(&mut self) -> Entity {
        if let Some(index) = self.free_indices.pop() {
            let slot = &mut self.slots[index as usize];
            slot.alive = true;
            Entity {
                index,
                generation: slot.generation,
            }
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 0,
                alive: true,
            });
            Entity {
                index,
                generation: 0,
            }
        }
    }

    /// Despawns an entity, removing all of its components and marking its
    /// slot free for reuse by a future [`World::spawn`]. The slot's
    /// generation is bumped first, so `entity` (and any copy of it) is
    /// reported as not alive by [`World::is_alive`] from this point on,
    /// even after the slot is recycled.
    ///
    /// Returns [`EcsError::StaleOrUnknownEntity`] if `entity` was already
    /// not alive.
    pub fn despawn(&mut self, entity: Entity) -> Result<(), EcsError> {
        self.check_alive(entity)?;

        let slot = &mut self.slots[entity.index as usize];
        slot.alive = false;
        slot.generation = slot.generation.wrapping_add(1);
        self.free_indices.push(entity.index);

        for storage in self.components.values_mut() {
            storage.remove(&entity.index);
        }

        Ok(())
    }

    /// Whether `entity` refers to a currently-alive entity in this world
    /// (i.e. was spawned and has not since been despawned).
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.slots
            .get(entity.index as usize)
            .is_some_and(|slot| slot.alive && slot.generation == entity.generation)
    }

    /// The number of currently-alive entities.
    pub fn entity_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.alive).count()
    }

    fn check_alive(&self, entity: Entity) -> Result<(), EcsError> {
        if self.is_alive(entity) {
            Ok(())
        } else {
            Err(EcsError::StaleOrUnknownEntity)
        }
    }

    /// Inserts (or replaces) a component of type `T` on `entity`.
    ///
    /// Returns [`EcsError::StaleOrUnknownEntity`] if `entity` is not
    /// alive.
    pub fn insert<T: 'static>(&mut self, entity: Entity, component: T) -> Result<(), EcsError> {
        self.check_alive(entity)?;
        self.components
            .entry(TypeId::of::<T>())
            .or_default()
            .insert(entity.index, Box::new(component));
        Ok(())
    }

    /// Returns a reference to `entity`'s component of type `T`, if it is
    /// alive and has one.
    pub fn get<T: 'static>(&self, entity: Entity) -> Option<&T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.components
            .get(&TypeId::of::<T>())?
            .get(&entity.index)?
            .downcast_ref::<T>()
    }

    /// Returns a mutable reference to `entity`'s component of type `T`, if
    /// it is alive and has one.
    pub fn get_mut<T: 'static>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.components
            .get_mut(&TypeId::of::<T>())?
            .get_mut(&entity.index)?
            .downcast_mut::<T>()
    }

    /// Removes and returns `entity`'s component of type `T`, if it is
    /// alive and has one.
    pub fn remove<T: 'static>(&mut self, entity: Entity) -> Option<T> {
        if !self.is_alive(entity) {
            return None;
        }
        let boxed = self
            .components
            .get_mut(&TypeId::of::<T>())?
            .remove(&entity.index)?;
        // The map only ever stores `T` under `TypeId::of::<T>()`, so this
        // downcast cannot fail in practice.
        boxed.downcast::<T>().ok().map(|b| *b)
    }

    /// Iterates every currently-alive entity that has a component of type
    /// `T`, along with a reference to that component.
    ///
    /// v0.0.1-pre1: a linear scan over `T`'s storage, filtered by
    /// liveness — not a cached archetype query. See
    /// `docs/architecture/core-runtime.md#ecs-architecture`.
    pub fn query<T: 'static>(&self) -> impl Iterator<Item = (Entity, &T)> {
        let slots = &self.slots;
        self.components
            .get(&TypeId::of::<T>())
            .into_iter()
            .flat_map(|storage| storage.iter())
            .filter_map(move |(&index, any)| {
                let slot = slots.get(index as usize)?;
                if !slot.alive {
                    return None;
                }
                let component = any.downcast_ref::<T>()?;
                Some((
                    Entity {
                        index,
                        generation: slot.generation,
                    },
                    component,
                ))
            })
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

    #[test]
    fn components_round_trip_through_insert_get_and_remove() {
        let mut world = World::new();
        let entity = world.spawn();

        assert!(world.get::<Position>(entity).is_none());

        world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
        assert_eq!(world.get::<Position>(entity), Some(&Position { x: 1.0, y: 2.0 }));

        world.get_mut::<Position>(entity).unwrap().x = 5.0;
        assert_eq!(world.get::<Position>(entity), Some(&Position { x: 5.0, y: 2.0 }));

        let removed = world.remove::<Position>(entity);
        assert_eq!(removed, Some(Position { x: 5.0, y: 2.0 }));
        assert!(world.get::<Position>(entity).is_none());
    }

    #[test]
    fn query_only_returns_alive_entities_with_the_component() {
        let mut world = World::new();

        let with_position = world.spawn();
        world.insert(with_position, Position { x: 0.0, y: 0.0 }).unwrap();

        let without_position = world.spawn();
        world.insert(without_position, Velocity { dx: 1.0, dy: 1.0 }).unwrap();

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
}
