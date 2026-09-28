// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Unit and property tests for [`World`] — kept in one module mirroring the pre-split `#[cfg(test)] mod tests`.

use super::*;
use std::any::TypeId;

use crate::column::Tick;
use crate::component_identity::CanaryComponent;
use crate::entity::Entity;
use crate::error::EcsError;

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

#[test]
fn insert_on_a_stale_entity_returns_stale_or_unknown() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Position { x: 0.0, y: 0.0 }).unwrap();
    world.despawn(entity).unwrap();
    assert!(
        matches!(
            world.insert(entity, Position { x: 1.0, y: 1.0 }),
            Err(EcsError::StaleOrUnknownEntity)
        ),
        "inserting into a despawned slot must report the stale handle, not silently drop"
    );
}

#[test]
fn erased_access_on_a_dead_entity_reports_absence() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Position { x: 0.0, y: 0.0 }).unwrap();
    world.despawn(entity).unwrap();
    let type_id = TypeId::of::<Position>();
    assert!(world.get_erased(entity, type_id).is_none());
    assert!(!world.set_erased(entity, type_id, Box::new(Position { x: 0.0, y: 0.0 })));
    assert!(!world.has_component_erased(entity, type_id));
}

#[test]
fn archetype_identity_is_independent_of_insertion_order() {
    let mut world = World::new();
    let first = world.spawn();
    world.insert(first, Position { x: 0.0, y: 0.0 }).unwrap();
    world.insert(first, Velocity { dx: 1.0, dy: 1.0 }).unwrap();
    let second = world.spawn();
    world.insert(second, Velocity { dx: 2.0, dy: 2.0 }).unwrap();
    world.insert(second, Position { x: 3.0, y: 3.0 }).unwrap();

    let mut both_orders: Vec<Entity> = world
        .query2::<Position, Velocity>()
        .map(|(e, _, _)| e)
        .collect();
    both_orders.sort_by_key(|e| e.index());
    let mut swapped: Vec<Entity> = world
        .query2::<Velocity, Position>()
        .map(|(e, _, _)| e)
        .collect();
    swapped.sort_by_key(|e| e.index());
    assert_eq!(both_orders, swapped);
    assert_eq!(both_orders.len(), 2);
}

#[test]
fn remove_move_preserves_remaining_change_ticks() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Position { x: 0.0, y: 0.0 }).unwrap();
    world.insert(entity, Velocity { dx: 1.0, dy: 1.0 }).unwrap();
    let written_at = world.change_tick();

    world.advance_tick();
    // Removing Velocity moves `entity` to a smaller archetype; the
    // surviving Position column keeps its own change tick, so a
    // remove-move must not mark Position as freshly changed.
    world.remove::<Velocity>(entity).unwrap();

    let changed: Vec<Entity> = world
        .query_changed_since::<Position>(written_at)
        .map(|(e, _)| e)
        .collect();
    assert!(
            changed.is_empty(),
            "moving archetypes because of an unrelated Velocity remove must not mark Position as freshly changed"
        );
}
