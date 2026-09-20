// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Hierarchy propagation: recomputing [`GlobalTransform`](crate::GlobalTransform)
//! from local [`Transform`](crate::Transform) and [`Parent`](crate::Parent).

use std::collections::HashMap;

use canary_ecs::{Entity, World};
use canary_scheduler::{Schedule, SystemAccess};

use crate::{GlobalTransform, Parent, Transform};

/// Declares the data access of [`propagate_transforms`]: reads `Transform`
/// and `Parent`, writes `GlobalTransform`.
pub fn transform_propagation_access() -> SystemAccess {
    SystemAccess::new()
        .reads::<Transform>()
        .reads::<Parent>()
        .writes::<GlobalTransform>()
}

/// Recomputes every [`GlobalTransform`](crate::GlobalTransform) from local
/// [`Transform`](crate::Transform)s and the [`Parent`](crate::Parent) links.
///
/// Roots (entities with no `Parent`, or whose `Parent` names a despawned,
/// unknown, or `Transform`-less entity) copy their local matrix; children
/// compose as `parent_global * local`, visited parent-before-child via depth
/// ordering computed fresh each run. Entities with a `Transform` but no
/// `GlobalTransform` get one inserted; entities with a `GlobalTransform`
/// but no `Transform` keep their last value (propagation only writes where
/// a local `Transform` exists).
///
/// Snapshot-then-write: hierarchy data is collected through read queries
/// first and `GlobalTransform` is written afterwards via
/// `get_mut`/`insert`, since `World::query2_mut` only covers
/// one-mutable-one-shared shapes and cannot express this pass.
pub fn propagate_transforms(world: &mut World) {
    // Snapshot: every local matrix plus its parent link, if any.
    let snapshot: Vec<(Entity, glam::Mat4, Option<Entity>)> = world
        .query::<Transform>()
        .map(|(entity, transform)| {
            let parent = world.get::<Parent>(entity).map(|link| link.0);
            (entity, transform.to_matrix(), parent)
        })
        .collect();

    let parent_of: HashMap<Entity, Entity> = snapshot
        .iter()
        .filter_map(|(entity, _, parent)| parent.map(|link| (*entity, link)))
        .collect();

    let mut ordered = snapshot;

    // Fast-path: if no entities have parents, depth is 0 for all and ordering is unchanged.
    if !parent_of.is_empty() {
        // Depth of each entity (roots at 0), memoized. A parent link that
        // leaves the snapshot — despawned entity, or one without a `Parent`
        // component of its own — ends the walk; a cycle ends it too, so a
        // malformed hierarchy degrades to local transforms rather than
        // looping forever.
        let mut depths: HashMap<Entity, usize> = HashMap::with_capacity(ordered.len());
        for (entity, _, parent) in &ordered {
            if parent.is_none() {
                depths.insert(*entity, 0);
                continue;
            }
            let mut chain: Vec<Entity> = Vec::new();
            let mut current = *entity;
            loop {
                if let Some(&known) = depths.get(&current) {
                    let base = known;
                    for (i, member) in chain.iter().enumerate() {
                        depths.insert(*member, base + chain.len() - i);
                    }
                    break;
                }
                if chain.contains(&current) {
                    // Cycle: number the walked members by their distance from
                    // the repeated link so ordering still terminates.
                    for (i, member) in chain.iter().enumerate() {
                        depths.entry(*member).or_insert(chain.len() - i);
                    }
                    depths.entry(current).or_insert(0);
                    break;
                }
                chain.push(current);
                match parent_of.get(&current) {
                    Some(next) => current = *next,
                    None => {
                        for (i, member) in chain.iter().enumerate() {
                            depths.insert(*member, chain.len() - 1 - i);
                        }
                        break;
                    }
                }
            }
        }

        ordered.sort_by_key(|(entity, _, _)| depths.get(entity).copied().unwrap_or(0));
    }

    // Compose parent-before-child: `depths` ordering guarantees a parent's
    // global is already in `composed` when its child is visited — unless
    // the parent has no `Transform` (or is gone), in which case the child
    // falls back to its local matrix.
    let mut composed: HashMap<Entity, glam::Mat4> = HashMap::with_capacity(ordered.len());
    for (entity, local, parent) in &ordered {
        let global = parent
            .and_then(|link| composed.get(&link).copied())
            .map_or(*local, |parent_global| parent_global * *local);
        composed.insert(*entity, global);
    }

    for (entity, global) in composed {
        // Read-then-maybe-write, not blind `get_mut`: `get_mut` stamps
        // the current tick unconditionally (a caller holding `&mut T`
        // is conservatively assumed to write through it), so an
        // unconditional write would mark every `GlobalTransform`
        // changed on every run -- even with zero edits -- and any
        // downstream `query_changed_since::<GlobalTransform>`
        // consumer would re-run every tick. The recomposition above is
        // deterministic in its inputs, so exact inequality here means
        // "something actually changed," not a float-comparison
        // shortcut.
        let needs_write = world
            .get::<GlobalTransform>(entity)
            .is_none_or(|slot| *slot != GlobalTransform(global));
        if !needs_write {
            continue;
        }
        match world.get_mut::<GlobalTransform>(entity) {
            Some(slot) => {
                *slot = GlobalTransform(global);
            }
            None => {
                // `entity` was alive at snapshot time and nothing despawns
                // mid-pass, so this cannot fail in practice; a stale
                // handle here would mean a concurrent modification this
                // single-threaded pass cannot observe.
                let _ = world.insert(entity, GlobalTransform(global));
            }
        }
    }
}

/// Registers [`propagate_transforms`] on `schedule` as a write system with
/// [`transform_propagation_access`]'s declaration.
pub fn register_transform_propagation(schedule: &mut Schedule) {
    schedule.add_write_system(transform_propagation_access(), propagate_transforms);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{remove_parent, set_parent};

    /// Element-wise approximate matrix equality (`glam` math is `f32`).
    fn assert_mat4_approx_eq(a: glam::Mat4, b: glam::Mat4) {
        for (x, y) in a.to_cols_array().iter().zip(b.to_cols_array().iter()) {
            assert!(
                (x - y).abs() < 1e-5,
                "matrices differ: {a:?} vs {b:?} (elements {x} vs {y})"
            );
        }
    }

    /// Translation component of an entity's `GlobalTransform`.
    fn global_translation(world: &World, entity: Entity) -> glam::Vec3 {
        world
            .get::<GlobalTransform>(entity)
            .expect("entity should have a GlobalTransform after propagation")
            .matrix()
            .to_scale_rotation_translation()
            .2
    }

    #[test]
    fn multi_level_chain_composes_translations_down_the_hierarchy() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(
                root,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(0.0, 2.0, 0.0)),
            )
            .unwrap();
        let grandchild = world.spawn();
        world
            .insert(
                grandchild,
                Transform::from_translation(glam::Vec3::new(0.0, 0.0, 3.0)),
            )
            .unwrap();
        set_parent(&mut world, child, Some(root)).unwrap();
        set_parent(&mut world, grandchild, Some(child)).unwrap();

        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, root) - glam::Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5
        );
        assert!(
            (global_translation(&world, child) - glam::Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5,
            "child composes onto its parent's global"
        );
        assert!(
            (global_translation(&world, grandchild) - glam::Vec3::new(1.0, 2.0, 3.0)).length()
                < 1e-5,
            "grandchild composes through the whole chain"
        );
    }

    #[test]
    fn moving_a_parent_updates_descendants_on_the_next_run() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(root, Transform::from_translation(glam::Vec3::ZERO))
            .unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(5.0, 0.0, 0.0)),
            )
            .unwrap();
        set_parent(&mut world, child, Some(root)).unwrap();
        propagate_transforms(&mut world);
        assert!(
            (global_translation(&world, child) - glam::Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5
        );

        world.get_mut::<Transform>(root).unwrap().translation = glam::Vec3::new(10.0, 0.0, 0.0);
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(15.0, 0.0, 0.0)).length() < 1e-5,
            "child must follow the moved parent"
        );
    }

    #[test]
    fn child_rotation_and_scale_compose_onto_the_parent() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(
                root,
                Transform {
                    translation: glam::Vec3::ZERO,
                    rotation: glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                    scale: glam::Vec3::ONE,
                },
            )
            .unwrap();
        let child = world.spawn();
        world
            .insert(child, Transform::from_translation(glam::Vec3::X))
            .unwrap();
        set_parent(&mut world, child, Some(root)).unwrap();

        propagate_transforms(&mut world);

        let expected = world.get::<Transform>(root).unwrap().to_matrix()
            * glam::Mat4::from_translation(glam::Vec3::X);
        assert_mat4_approx_eq(
            world.get::<GlobalTransform>(child).unwrap().matrix(),
            expected,
        );
        // A 90-degree Z rotation turns local +X into world +Y.
        assert!((global_translation(&world, child) - glam::Vec3::Y).length() < 1e-5);
    }

    #[test]
    fn propagation_inserts_missing_global_transforms() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(3.0, 0.0, 0.0)),
            )
            .unwrap();
        assert_eq!(world.get::<GlobalTransform>(entity), None);

        propagate_transforms(&mut world);

        assert_mat4_approx_eq(
            world.get::<GlobalTransform>(entity).unwrap().matrix(),
            glam::Mat4::from_translation(glam::Vec3::new(3.0, 0.0, 0.0)),
        );
    }

    #[test]
    fn global_transform_without_a_local_transform_is_left_alone() {
        let mut world = World::new();
        let entity = world.spawn();
        let untouched = glam::Mat4::from_translation(glam::Vec3::new(9.0, 9.0, 9.0));
        world
            .insert(entity, GlobalTransform::from_matrix(untouched))
            .unwrap();

        propagate_transforms(&mut world);

        assert_mat4_approx_eq(
            world.get::<GlobalTransform>(entity).unwrap().matrix(),
            untouched,
        );
    }

    #[test]
    fn child_of_a_despawned_parent_falls_back_to_its_local_transform() {
        let mut world = World::new();
        let parent = world.spawn();
        world
            .insert(
                parent,
                Transform::from_translation(glam::Vec3::new(100.0, 0.0, 0.0)),
            )
            .unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .unwrap();
        set_parent(&mut world, child, Some(parent)).unwrap();
        world.despawn(parent).unwrap();

        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5,
            "stale parent link must not panic and must not shift the child"
        );
    }

    #[test]
    fn child_of_a_transform_less_but_alive_parent_falls_back_to_local() {
        // Only the *despawned*-parent case was covered before; a parent
        // that is alive but carries no `Transform` takes the same
        // fallback path (`composed` has no entry for it) and deserves
        // its own pin.
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(2.0, 0.0, 0.0)),
            )
            .unwrap();
        set_parent(&mut world, child, Some(parent)).unwrap();

        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5,
            "a Transform-less parent must not shift the child"
        );
    }

    #[test]
    fn a_forged_parent_cycle_degrades_to_local_fallback_instead_of_hanging() {
        // `set_parent` rejects every cycle at write time, so the cycle
        // fallback in `propagate_transforms` is reachable only through
        // raw `Parent` inserts bypassing that guard — forge one here to
        // prove the pass terminates with deterministic local-fallback
        // globals rather than looping forever. Exactly one member keeps
        // its pure local transform (its parent's global isn't composed
        // yet when it is visited); the other composes onto it. Which is
        // which follows snapshot order, so the test accepts either
        // arrangement rather than over-pinning it.
        let mut world = World::new();
        let local_a = glam::Vec3::new(10.0, 0.0, 0.0);
        let local_b = glam::Vec3::new(1.0, 0.0, 0.0);
        let a = world.spawn();
        let b = world.spawn();
        world
            .insert(a, Transform::from_translation(local_a))
            .unwrap();
        world
            .insert(b, Transform::from_translation(local_b))
            .unwrap();
        world.insert(a, Parent(b)).unwrap();
        world.insert(b, Parent(a)).unwrap();

        propagate_transforms(&mut world);

        let global_a = global_translation(&world, a);
        let global_b = global_translation(&world, b);
        assert!(
            global_a.is_finite() && global_b.is_finite(),
            "cycle fallback must not produce inf/NaN: a={global_a:?} b={global_b:?}"
        );
        let a_is_local = (global_a - local_a).length() < 1e-5;
        let b_is_local = (global_b - local_b).length() < 1e-5;
        assert!(
            a_is_local != b_is_local,
            "exactly one cycle member must fall back to local; got a={global_a:?} b={global_b:?}"
        );
        let composed = local_a + local_b;
        if a_is_local {
            assert!(
                (global_b - composed).length() < 1e-5,
                "b must compose onto a's global; got {global_b:?}"
            );
        } else {
            assert!(
                (global_a - composed).length() < 1e-5,
                "a must compose onto b's global; got {global_a:?}"
            );
        }
    }

    #[test]
    fn detaching_a_child_restores_its_local_transform_as_global() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(
                root,
                Transform::from_translation(glam::Vec3::new(8.0, 0.0, 0.0)),
            )
            .unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .unwrap();
        set_parent(&mut world, child, Some(root)).unwrap();
        propagate_transforms(&mut world);
        assert!(
            (global_translation(&world, child) - glam::Vec3::new(9.0, 0.0, 0.0)).length() < 1e-5
        );

        remove_parent(&mut world, child).unwrap();
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5,
            "detached child is a root again"
        );
    }

    #[test]
    fn schedule_registration_runs_propagation_as_a_write_system() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(
                root,
                Transform::from_translation(glam::Vec3::new(2.0, 0.0, 0.0)),
            )
            .unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(3.0, 0.0, 0.0)),
            )
            .unwrap();
        set_parent(&mut world, child, Some(root)).unwrap();

        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        schedule.run(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5,
            "registered system must propagate through Schedule::run"
        );
    }

    #[test]
    fn rerun_without_edits_marks_no_global_transform_changed() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(
                root,
                Transform::from_translation(glam::Vec3::new(2.0, 0.0, 0.0)),
            )
            .unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(3.0, 0.0, 0.0)),
            )
            .unwrap();
        set_parent(&mut world, child, Some(root)).unwrap();

        propagate_transforms(&mut world);
        // Two ticks pass with zero edits in between: the baseline is
        // captured *before* the final tick advance, so any write the
        // second propagation performs stamps a tick strictly newer
        // than the baseline and shows up in the query. Without the
        // read-before-write above, this fails (every global stamped
        // at the newest tick); with it, nothing is stamped at all.
        world.advance_tick();
        let baseline = world.change_tick();
        world.advance_tick();
        propagate_transforms(&mut world);

        assert!(
            world
                .query_changed_since::<GlobalTransform>(baseline)
                .next()
                .is_none(),
            "a no-op re-propagation must not dirty change detection"
        );
    }
}
