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
///
/// External-consumer metadata, not propagation input: the propagation
/// system walks `Parent` links only and never reads `Children`, so this
/// list exists for gameplay/UI/networking queries, not for the engine's
/// own transform pass. Raw [`World::despawn`] on a hierarchy member does
/// not maintain it — detach via [`remove_parent`] (or use a helper that
/// does) before despawning, or stale handles accumulate here. Stale
/// entries are dead-generation handles, never aliases (see
/// [`World::is_alive`]), but they are the caller's to clean up.
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
/// `Some`, `parent`) is not alive, and [`EcsError::HierarchyCycle`]
/// when `parent` is `child` itself or a descendant of `child` (which
/// would close a parent/child loop). Cycles fail here, at write time:
/// propagation degrades a cyclic link to a local-transform fallback
/// rather than looping forever, but silently accepting a caller bug
/// and producing subtly wrong world-space matrices is worse than
/// rejecting it loudly.
///
/// Both paths touch `Children` through `get_mut`, so a reparent also
/// stamps the `Children` change tick — a hierarchy edit reads as a
/// `Children` data change to any `query_changed_since::<Children>`
/// consumer, not just as a `Parent` change.
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
        if new_parent == child || is_descendant_of(world, new_parent, child) {
            return Err(EcsError::HierarchyCycle);
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

/// Whether `candidate` names `ancestor` anywhere in its parent chain.
/// Walks `Parent` links upward; a stale link (despawned or unknown
/// entity) ends the walk rather than erroring, matching propagation's
/// own fallback. The walk is bounded by the number of times it can
/// advance without revisiting an entity, so a pre-existing cycle in the
/// stored data still terminates instead of looping.
fn is_descendant_of(world: &World, candidate: Entity, ancestor: Entity) -> bool {
    let mut current = candidate;
    let mut visited: Vec<Entity> = Vec::new();
    loop {
        if current == ancestor {
            return true;
        }
        if visited.contains(&current) {
            return false;
        }
        visited.push(current);
        match world.get::<Parent>(current) {
            Some(link) => current = link.0,
            None => return false,
        }
    }
}

/// Detaches `child` from its parent, if it has one.
///
/// Equivalent to [`set_parent`] with `None`. Returns
/// [`EcsError::StaleOrUnknownEntity`] when `child` is not alive; having no
/// parent is a successful no-op, not an error.
pub fn remove_parent(world: &mut World, child: Entity) -> Result<(), EcsError> {
    set_parent(world, child, None)
}

/// Despawns `root` and its entire descendant subtree, leaving no stale
/// hierarchy links behind.
///
/// Raw [`World::despawn`] knows nothing about `Parent`/`Children` (a
/// deliberate crate boundary — `canary-ecs` must not name hierarchy
/// types), so despawning hierarchy members directly orphans children's
/// `Parent` links and leaves dead handles in survivors' `Children`
/// lists. This helper is the correct way to remove a hierarchy:
/// post-order, it despawns every descendant before `root` itself, and
/// detaches `root` from a surviving parent's `Children` first — so no
/// live entity retains a link to anything despawned here.
///
/// A forged cycle in stored data terminates rather than recursing
/// forever (an entity already visited on this walk is not
/// re-entered); cycle *creation* remains rejected at write time by
/// [`set_parent`].
///
/// Returns [`EcsError::StaleOrUnknownEntity`] when `root` is not
/// alive; in that case nothing is despawned.
pub fn despawn_subtree(world: &mut World, root: Entity) -> Result<(), EcsError> {
    if !world.is_alive(root) {
        return Err(EcsError::StaleOrUnknownEntity);
    }
    // Detach from a surviving parent first, so no live `Children`
    // list retains `root` after this call.
    if let Some(parent) = world.get::<Parent>(root).map(|link| link.0) {
        if let Some(children) = world.get_mut::<Children>(parent) {
            children.0.retain(|entity| *entity != root);
        }
        world.remove::<Parent>(root);
    }
    // Post-order: descendants first (each handles its own subtree via
    // the same detach+recurse, so a forged cycle terminates on the
    // visited set rather than looping), then `root` itself, whose own
    // `Children` component dies with it.
    let mut visited = vec![root];
    let mut stack: Vec<Entity> = world
        .get::<Children>(root)
        .map(|children| children.0.clone())
        .unwrap_or_default();
    while let Some(entity) = stack.pop() {
        if !world.is_alive(entity) || visited.contains(&entity) {
            continue;
        }
        visited.push(entity);
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.0.iter().copied());
        }
    }
    for entity in visited.iter().skip(1) {
        // `is_alive` was checked when visited; a despawn cannot fail
        // here short of concurrent modification, which this
        // single-threaded walk cannot observe.
        world
            .despawn(*entity)
            .expect("entity visited alive in a single-threaded walk");
    }
    world.despawn(root)
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

    #[test]
    fn set_parent_rejects_parenting_an_entity_to_itself() {
        let mut world = World::new();
        let entity = world.spawn();

        assert_eq!(
            set_parent(&mut world, entity, Some(entity)),
            Err(EcsError::HierarchyCycle),
            "a self-parent would close a one-link cycle"
        );
        assert_eq!(world.get::<Parent>(entity), None);
    }

    #[test]
    fn set_parent_rejects_closing_a_cycle_through_a_descendant() {
        let mut world = World::new();
        let root = world.spawn();
        let child = world.spawn();
        let grandchild = world.spawn();
        set_parent(&mut world, child, Some(root)).unwrap();
        set_parent(&mut world, grandchild, Some(child)).unwrap();

        assert_eq!(
            set_parent(&mut world, root, Some(grandchild)),
            Err(EcsError::HierarchyCycle),
            "parenting a root under its own grandchild must fail"
        );
        assert_eq!(
            set_parent(&mut world, root, Some(child)),
            Err(EcsError::HierarchyCycle),
            "parenting a root under its own child must fail"
        );
        assert_eq!(
            world.get::<Parent>(root),
            None,
            "a rejected attach must leave the existing relationship untouched"
        );
    }

    #[test]
    fn despawn_subtree_removes_descendants_and_detaches_from_surviving_parent() {
        let mut world = World::new();
        let grandparent = world.spawn();
        let root = world.spawn();
        let child = world.spawn();
        let grandchild = world.spawn();
        set_parent(&mut world, root, Some(grandparent)).unwrap();
        set_parent(&mut world, child, Some(root)).unwrap();
        set_parent(&mut world, grandchild, Some(child)).unwrap();

        despawn_subtree(&mut world, root).unwrap();

        assert!(!world.is_alive(root));
        assert!(!world.is_alive(child));
        assert!(!world.is_alive(grandchild));
        assert!(world.is_alive(grandparent));
        assert_eq!(
            world.get::<Children>(grandparent),
            Some(&Children(vec![])),
            "surviving parent must no longer list the despawned root"
        );
    }

    #[test]
    fn despawn_subtree_on_stale_handle_errors_and_despawns_nothing() {
        let mut world = World::new();
        let root = world.spawn();
        let child = world.spawn();
        set_parent(&mut world, child, Some(root)).unwrap();
        world.despawn(root).unwrap();

        assert_eq!(
            despawn_subtree(&mut world, root),
            Err(EcsError::StaleOrUnknownEntity)
        );
        // The earlier raw despawn already orphaned the child; the
        // failed subtree call must not have touched it further.
        assert!(world.is_alive(child));
    }
}
