// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! ECS parent/child hierarchy components and the helpers that keep them in sync.

use canary_ecs::{EcsError, Entity, World};

/// Points at this entity's parent in the hierarchy, if it has one.
///
/// Entities without a `Parent` are roots. A `Parent` naming a despawned or
/// unknown entity is treated as absent by propagation (the child falls back
/// to its local transform) rather than as an error at read time; use
/// [`set_parent`]/[`remove_parent`] to keep the relationship well-formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Parent(pub Entity);

/// The entities that name this entity as their [`Parent`].
///
/// Kept in sync with `Parent` by [`set_parent`]/[`remove_parent`] — never
/// edited directly when those helpers are available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Children(pub Vec<Entity>);

/// Attaches `child` under `parent` (or detaches it, when `None`), keeping
/// [`Parent`] and [`Children`] in sync with each other.
///
/// Detaching removes `Parent` from `child` and drops `child` from the old
/// parent's `Children` (leaving an empty list in place, not removing the
/// component). Attaching inserts `Parent(parent)` on `child`
/// and pushes `child` onto `parent`'s `Children`, creating that component
/// when missing.
///
/// Returns [`EcsError::StaleOrUnknownEntity`] when `child` (or, when
/// `Some`, `parent`) is not alive.
pub fn set_parent(
    world: &mut World,
    child: Entity,
    parent: Option<Entity>,
) -> Result<(), EcsError> {
    if !world.is_alive(child) {
        return Err(EcsError::StaleOrUnknownEntity);
    }
    if let Some(new_parent) = parent {
        if !world.is_alive(new_parent) {
            return Err(EcsError::StaleOrUnknownEntity);
        }
    }

    let old_parent = world.get::<Parent>(child).map(|p| p.0);

    // No-op when the requested relationship already holds.
    if old_parent == parent {
        return Ok(());
    }

    // Detach from the previous parent first, so a reparent moves the child
    // rather than duplicating it across two `Children` lists.
    if let Some(old) = old_parent {
        if let Some(children) = world.get_mut::<Children>(old) {
            children.0.retain(|e| *e != child);
        }
        world.remove::<Parent>(child);
    }

    let Some(new_parent) = parent else {
        return Ok(());
    };

    world.insert(child, Parent(new_parent))?;
    match world.get_mut::<Children>(new_parent) {
        Some(children) => {
            if !children.0.contains(&child) {
                children.0.push(child);
            }
        }
        None => {
            world.insert(new_parent, Children(vec![child]))?;
        }
    }

    Ok(())
}

/// Detaches `child` from its parent, if it has one.
///
/// Equivalent to [`set_parent`] with `None`. Returns
/// [`EcsError::StaleOrUnknownEntity`] when `child` is not alive; having no
/// parent is a successful no-op, not an error.
pub fn remove_parent(world: &mut World, child: Entity) -> Result<(), EcsError> {
    set_parent(world, child, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_parent_inserts_parent_and_registers_the_child() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        set_parent(&mut world, child, Some(parent)).unwrap();

        assert_eq!(world.get::<Parent>(child), Some(&Parent(parent)));
        assert_eq!(
            world.get::<Children>(parent),
            Some(&Children(vec![child])),
            "parent must list the newly attached child"
        );
    }

    #[test]
    fn set_parent_to_none_removes_parent_and_unregisters_the_child() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        set_parent(&mut world, child, Some(parent)).unwrap();

        remove_parent(&mut world, child).unwrap();

        assert_eq!(world.get::<Parent>(child), None);
        assert_eq!(
            world.get::<Children>(parent),
            Some(&Children(vec![])),
            "detached child must be dropped from the parent's list"
        );
    }

    #[test]
    fn reparenting_moves_the_child_between_children_lists() {
        let mut world = World::new();
        let first = world.spawn();
        let second = world.spawn();
        let child = world.spawn();
        set_parent(&mut world, child, Some(first)).unwrap();

        set_parent(&mut world, child, Some(second)).unwrap();

        assert_eq!(world.get::<Parent>(child), Some(&Parent(second)));
        assert_eq!(
            world.get::<Children>(first),
            Some(&Children(vec![])),
            "old parent must no longer list the moved child"
        );
        assert_eq!(world.get::<Children>(second), Some(&Children(vec![child])));
    }

    #[test]
    fn setting_the_same_parent_twice_does_not_duplicate_the_child() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        set_parent(&mut world, child, Some(parent)).unwrap();
        set_parent(&mut world, child, Some(parent)).unwrap();

        assert_eq!(
            world.get::<Children>(parent),
            Some(&Children(vec![child])),
            "idempotent re-attach must not push the child twice"
        );
    }

    #[test]
    fn remove_parent_without_a_parent_is_a_successful_no_op() {
        let mut world = World::new();
        let child = world.spawn();

        remove_parent(&mut world, child).unwrap();

        assert_eq!(world.get::<Parent>(child), None);
    }

    #[test]
    fn set_parent_rejects_stale_child_and_parent_handles() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        world.despawn(child).unwrap();

        assert_eq!(
            set_parent(&mut world, child, Some(parent)),
            Err(EcsError::StaleOrUnknownEntity),
            "attaching a despawned child must error, not panic"
        );

        let other = world.spawn();
        world.despawn(parent).unwrap();
        assert_eq!(
            set_parent(&mut world, other, Some(parent)),
            Err(EcsError::StaleOrUnknownEntity),
            "attaching under a despawned parent must error, not panic"
        );
        assert_eq!(
            remove_parent(&mut world, child),
            Err(EcsError::StaleOrUnknownEntity),
            "detaching a despawned child must error, not panic"
        );
    }

    #[test]
    fn detaching_clears_a_stale_parent_link_left_by_despawn() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        set_parent(&mut world, child, Some(parent)).unwrap();
        world.despawn(parent).unwrap();

        // The child's `Parent` still names the despawned entity; detaching
        // must clean that up without touching the dead parent's storage.
        remove_parent(&mut world, child).unwrap();

        assert_eq!(world.get::<Parent>(child), None);
    }
}
