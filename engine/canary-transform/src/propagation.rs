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

use canary_ecs::{Entity, Tick, World};
use canary_scheduler::{Schedule, SystemAccess};

use crate::{Children, GlobalTransform, Parent, Transform};

/// Declares the data access of [`propagate_transforms`]: reads `Transform`,
/// `Parent`, and `GlobalTransform` (read-before-write), writes
/// `GlobalTransform`.
pub fn transform_propagation_access() -> SystemAccess {
    SystemAccess::new()
        .reads::<Transform>()
        .reads::<Parent>()
        .writes::<GlobalTransform>()
}

/// Last propagation run that recomputed: the tick it ran at, the
/// membership counts its output was composed from, and whether that view
/// is settled. Stored as an ECS resource so the baseline lives and dies
/// with the `World` it describes. Private: the skip is an internal fast
/// path, not a second API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PropagationBaseline {
    /// [`World::change_tick`] when the baseline was stored.
    last_tick: Tick,
    /// [`World::entity_count`] at that tick.
    entity_count: usize,
    /// Live `Transform` components at that tick.
    transform_count: usize,
    /// Live `Parent` components at that tick.
    parent_count: usize,
    /// Live `GlobalTransform` components at that tick.
    global_count: usize,
    /// Whether the next run may skip. False after a recompute: a write
    /// stamped at the recompute's own tick but landing *after* the probe
    /// (a system writing `Transform` later in the same tick than this one
    /// ran) compares equal to — not greater than — this tick, so the next
    /// tick always recomputes once more (the follow-up pass) before
    /// quiescing. True once that pass has run.
    settled: bool,
}

/// Whether the next [`propagate_transforms`] run must recompute.
///
/// True when no baseline exists yet (first run always recomputes), when
/// this run shares its tick with the baseline (same-tick writes are
/// invisible to the `>` probe below, so a same-tick rerun never skips),
/// when the previous run recomputed and its follow-up pass is still due,
/// when membership changed (entity or relevant component counts differ —
/// the only signal a raw [`World::despawn`] or a component `remove`
/// leaves behind), or when any input component was written after the
/// baseline tick. False only when a full recompute is provably a no-op —
/// and a skipped run stores nothing, so the baseline keeps pointing at
/// the last recompute: a write landing after a skip still compares
/// greater than that older tick and is caught on the next run.
fn propagation_is_dirty(world: &World) -> bool {
    let Some(baseline) = world.resource::<PropagationBaseline>().copied() else {
        return true;
    };
    if world.change_tick() <= baseline.last_tick {
        return true;
    }
    if !baseline.settled {
        return true;
    }
    if world.entity_count() != baseline.entity_count
        || world.query::<Transform>().count() != baseline.transform_count
        || world.query::<Parent>().count() != baseline.parent_count
        || world.query::<GlobalTransform>().count() != baseline.global_count
    {
        return true;
    }
    let since = baseline.last_tick;
    world
        .query_changed_since::<Transform>(since)
        .next()
        .is_some()
        || world.query_changed_since::<Parent>(since).next().is_some()
        || world
            .query_changed_since::<Children>(since)
            .next()
            .is_some()
        || world
            .query_changed_since::<GlobalTransform>(since)
            .next()
            .is_some()
}

/// Records the current tick and membership counts as the baseline for the
/// next run's dirty check. `settled` is true only for a follow-up pass
/// (see [`PropagationBaseline::settled`]); every other recompute leaves
/// another pass due.
fn store_propagation_baseline(world: &mut World, settled: bool) {
    world.insert_resource(PropagationBaseline {
        last_tick: world.change_tick(),
        entity_count: world.entity_count(),
        transform_count: world.query::<Transform>().count(),
        parent_count: world.query::<Parent>().count(),
        global_count: world.query::<GlobalTransform>().count(),
        settled,
    });
}

/// Recomputes every [`GlobalTransform`](crate::GlobalTransform) from local
/// [`Transform`](crate::Transform)s and the [`Parent`](crate::Parent) links,
/// skipping the whole pass on quiet ticks where nothing affecting the output
/// changed.
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
///
/// # Quiet-tick skip (what counts as "changed")
///
/// The composed output is a pure function of three things: every live
/// entity's `Transform` value, every live entity's `Parent` link, and
/// which entities are alive at all. A tick where none of those moved
/// recomposes bit-identical globals, so the pass returns early after a
/// cheap change-detection probe instead of rebuilding the snapshot,
/// depth map, and composed matrices. The probe watches:
///
/// - `Transform` values, via `query_changed_since` (covers `insert`
///   overwrites and `get_mut` touches, including the physics sync writes
///   that run earlier in the same tick);
/// - `Parent` links, via `query_changed_since` (covers attaches and
///   reparents; a forged raw `Parent` insert stamps a tick the same way);
/// - `Children` values, via `query_changed_since` (covers detaches and
///   subtree removals: [`set_parent`](crate::set_parent) and
///   [`despawn_subtree`](crate::despawn_subtree) touch `Children` through
///   `get_mut`, and a `Parent` *removal* leaves no tick behind on the
///   entity it left — the surviving parent's `Children` stamp is the
///   only tick that removal produces);
/// - `GlobalTransform` values, via `query_changed_since` (an external
///   manual edit would otherwise survive a skipped tick instead of being
///   repaired back to the composed value);
/// - structural membership, via entity/component counts (a raw
///   [`World::despawn`] of a `Transform`-carrying parent stamps no tick
///   on its surviving children, yet flips them from composed to local
///   fallback — the count change is what catches it, conservatively
///   recomputing even when the despawned entity turns out to be a leaf
///   whose loss changed nothing).
///
/// Two guard rails keep the tick probe exact. A rerun within the same
/// tick as the baseline never skips (writes stamped at the current tick
/// compare equal to, not greater than, a same-tick baseline). And every
/// recompute leaves a follow-up pass due on the next tick, catching a
/// write that lands after the probe within the recompute's own tick —
/// so every change propagates no later than the tick after it lands,
/// matching the unskipped behavior for arbitrary system orderings. No
/// depth state is cached across runs, so hierarchy edits need no
/// invalidation: the first tick after any structural change recomputes
/// fully.
///
/// # Downstream change-tick contract
///
/// A skipped tick writes nothing, so it stamps no `GlobalTransform`
/// change ticks: a `query_changed_since::<GlobalTransform>` consumer
/// observing a quiet tick sees exactly what the pre-skip code produced
/// on a quiet tick (the read-before-write below already avoided dirtying
/// ticks on no-op recomputes). No in-tree consumer relies on per-tick
/// tick advancement: render extraction (`canary-render-ecs`) reads
/// `GlobalTransform` through full `query2` intersections every tick,
/// never through change detection, so a skipped tick starves nothing —
/// it simply re-reads the same correct matrices. Scheduler staging is
/// untouched: this system keeps [`transform_propagation_access`]'s
/// declaration and still runs as a solo-write stage in registration
/// order; only its body returns early.
pub fn propagate_transforms(world: &mut World) {
    let baseline = world.resource::<PropagationBaseline>().copied();
    let followup =
        baseline.is_some_and(|seen| !seen.settled && world.change_tick() > seen.last_tick);
    if !followup && !propagation_is_dirty(world) {
        return;
    }
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

    // Depth of each entity (roots at 0), memoized. A parent link that
    // leaves the snapshot — despawned entity, or one without a `Parent`
    // component of its own — ends the walk; a cycle ends it too, so a
    // malformed hierarchy degrades to local transforms rather than
    // looping forever.
    let mut depths: HashMap<Entity, usize> = HashMap::with_capacity(snapshot.len());
    for (entity, _, _) in &snapshot {
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

    let mut ordered = snapshot;
    ordered.sort_by_key(|(entity, _, _)| depths.get(entity).copied().unwrap_or(0));

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
        // Non-finite globals must fail loudly in dev, not churn change
        // detection forever: `NaN != NaN` would rewrite (and dirty the
        // tick for) this entity on every run. The recomposition above
        // is deterministic, so exact inequality below means "changed".
        debug_assert!(
            global.is_finite(),
            "propagate_transforms composed a non-finite GlobalTransform"
        );
        // Skip, never poison, in every profile (same doctrine as the
        // physics sync-side NaN guard): the `debug_assert` above is
        // compiled out in release, and without this skip a NaN local
        // would write a NaN global that the exact-inequality check
        // below rewrites on every future run — perpetual change-tick
        // churn plus a poisoned matrix downstream. Retaining the last
        // good `GlobalTransform` (or none, if never written) keeps one
        // bad `Transform` from corrupting the whole hierarchy.
        if !global.is_finite() {
            continue;
        }
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
                world
                    .insert(entity, GlobalTransform(global))
                    .expect("entity alive at snapshot time; single-threaded pass");
            }
        }
    }
    store_propagation_baseline(world, followup);
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

    /// Runs propagation to a settled baseline: propagate, advance, and
    /// propagate again (the follow-up pass), so a subsequent quiet tick
    /// probes clean.
    fn propagate_to_settled(world: &mut World) {
        propagate_transforms(world);
        world.advance_tick();
        propagate_transforms(world);
    }

    #[test]
    fn quiet_tick_skips_and_leaves_output_identical() {
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
        propagate_to_settled(&mut world);
        assert!(
            world
                .resource::<PropagationBaseline>()
                .is_some_and(|baseline| baseline.settled),
            "two runs must settle the baseline"
        );
        let before_root = world.get::<GlobalTransform>(root).copied();
        let before_child = world.get::<GlobalTransform>(child).copied();
        let settled_tick = world
            .resource::<PropagationBaseline>()
            .map(|baseline| baseline.last_tick);

        world.advance_tick();
        assert!(
            !propagation_is_dirty(&world),
            "a tick with no writes must probe clean"
        );
        propagate_transforms(&mut world);

        assert_eq!(world.get::<GlobalTransform>(root).copied(), before_root);
        assert_eq!(world.get::<GlobalTransform>(child).copied(), before_child);
        assert_eq!(
            world
                .resource::<PropagationBaseline>()
                .map(|baseline| baseline.last_tick),
            settled_tick,
            "a skipped run stores nothing, keeping the baseline on the last recompute"
        );
        // A second consecutive quiet tick probes clean too.
        world.advance_tick();
        assert!(
            !propagation_is_dirty(&world),
            "consecutive quiet ticks must stay clean"
        );
    }

    #[test]
    fn same_tick_rerun_after_a_write_recomputes() {
        // The pre-skip code recomputed unconditionally, so a write and a
        // rerun sharing one tick (no `advance_tick` between them) must
        // still propagate: same-tick writes are invisible to the `>`
        // probe, which is why same-tick reruns never skip.
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

        world.get_mut::<Transform>(root).unwrap().translation = glam::Vec3::new(10.0, 0.0, 0.0);
        assert!(
            propagation_is_dirty(&world),
            "a same-tick write must probe dirty"
        );
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(15.0, 0.0, 0.0)).length() < 1e-5,
            "child must follow the moved parent"
        );
    }

    #[test]
    fn follow_up_pass_runs_once_after_a_change_then_quiesces() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(root, Transform::from_translation(glam::Vec3::ZERO))
            .unwrap();
        propagate_to_settled(&mut world);

        world.advance_tick();
        world.get_mut::<Transform>(root).unwrap().translation = glam::Vec3::X;
        propagate_transforms(&mut world);
        assert!(
            world
                .resource::<PropagationBaseline>()
                .is_some_and(|baseline| !baseline.settled),
            "a change-driven recompute leaves a follow-up pass due"
        );

        world.advance_tick();
        assert!(
            propagation_is_dirty(&world),
            "the follow-up pass runs even with no new writes"
        );
        propagate_transforms(&mut world);
        assert!(
            world
                .resource::<PropagationBaseline>()
                .is_some_and(|baseline| baseline.settled),
            "the follow-up pass settles the baseline"
        );
        assert!(
            (global_translation(&world, root) - glam::Vec3::X).length() < 1e-5,
            "output stays correct through the follow-up"
        );

        world.advance_tick();
        assert!(
            !propagation_is_dirty(&world),
            "quiet ticks skip once the follow-up has run"
        );
    }

    #[test]
    fn write_after_a_skip_is_caught_on_the_next_tick() {
        // A skip stores no baseline, so a write landing after the skip —
        // stamped at a tick newer than the frozen baseline — probes dirty
        // on the next tick instead of being swallowed.
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
        propagate_to_settled(&mut world);

        world.advance_tick();
        propagate_transforms(&mut world);
        world.get_mut::<Transform>(root).unwrap().translation = glam::Vec3::new(10.0, 0.0, 0.0);

        world.advance_tick();
        assert!(
            propagation_is_dirty(&world),
            "a write after a skip must probe dirty"
        );
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(15.0, 0.0, 0.0)).length() < 1e-5,
            "child must follow the moved parent"
        );
    }

    #[test]
    fn attach_marks_dirty_and_propagates() {
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
        propagate_to_settled(&mut world);
        assert!(
            (global_translation(&world, child) - glam::Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5
        );

        world.advance_tick();
        set_parent(&mut world, child, Some(root)).unwrap();
        assert!(propagation_is_dirty(&world), "an attach must probe dirty");
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(9.0, 0.0, 0.0)).length() < 1e-5,
            "newly attached child must compose onto its parent"
        );
    }

    #[test]
    fn detach_marks_dirty_and_restores_local() {
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
        propagate_to_settled(&mut world);

        world.advance_tick();
        remove_parent(&mut world, child).unwrap();
        // A detach removes `Parent` (leaving no tick on the child itself);
        // the surviving parent's `Children` stamp plus the membership
        // counts are what make this probe dirty.
        assert!(propagation_is_dirty(&world), "a detach must probe dirty");
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5,
            "detached child is a root again"
        );
    }

    #[test]
    fn raw_despawn_of_parent_marks_dirty_and_falls_back_to_local() {
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
        propagate_transforms(&mut world);
        world.advance_tick();
        propagate_transforms(&mut world);
        assert!(
            (global_translation(&world, child) - glam::Vec3::new(101.0, 0.0, 0.0)).length() < 1e-5
        );

        world.advance_tick();
        // Raw despawn: no helper touches the child's ticks, so only the
        // membership fingerprint catches this.
        world.despawn(parent).unwrap();
        assert!(
            propagation_is_dirty(&world),
            "despawning a parent must probe dirty even though no child tick moved"
        );
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5,
            "child of a raw-despawned parent falls back to local"
        );
    }

    #[test]
    fn despawn_subtree_marks_dirty_and_survivors_stay_correct() {
        use crate::despawn_subtree;

        let mut world = World::new();
        let grandparent = world.spawn();
        world
            .insert(grandparent, Transform::from_translation(glam::Vec3::X))
            .unwrap();
        let root = world.spawn();
        world
            .insert(
                root,
                Transform::from_translation(glam::Vec3::new(0.0, 1.0, 0.0)),
            )
            .unwrap();
        let child = world.spawn();
        world
            .insert(
                child,
                Transform::from_translation(glam::Vec3::new(0.0, 0.0, 1.0)),
            )
            .unwrap();
        set_parent(&mut world, root, Some(grandparent)).unwrap();
        set_parent(&mut world, child, Some(root)).unwrap();
        propagate_to_settled(&mut world);

        world.advance_tick();
        despawn_subtree(&mut world, root).unwrap();
        assert!(
            propagation_is_dirty(&world),
            "a subtree removal must probe dirty"
        );
        propagate_transforms(&mut world);

        assert!(world.is_alive(grandparent));
        assert!(
            (global_translation(&world, grandparent) - glam::Vec3::X).length() < 1e-5,
            "the surviving grandparent keeps its global"
        );
    }

    #[test]
    fn spawned_entity_marks_dirty_and_propagates() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(
                root,
                Transform::from_translation(glam::Vec3::new(4.0, 0.0, 0.0)),
            )
            .unwrap();
        propagate_to_settled(&mut world);

        world.advance_tick();
        let late = world.spawn();
        world
            .insert(
                late,
                Transform::from_translation(glam::Vec3::new(1.0, 1.0, 1.0)),
            )
            .unwrap();
        assert!(
            propagation_is_dirty(&world),
            "spawning a Transform entity must probe dirty"
        );
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, late) - glam::Vec3::new(1.0, 1.0, 1.0)).length() < 1e-5,
            "a late-spawned root copies its local transform"
        );
        assert!(
            (global_translation(&world, root) - glam::Vec3::new(4.0, 0.0, 0.0)).length() < 1e-5,
            "pre-existing globals survive a membership change"
        );
    }

    #[test]
    fn direct_parent_removal_marks_dirty_and_restores_local() {
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
        propagate_to_settled(&mut world);

        world.advance_tick();
        // Bypassing `remove_parent`: no `Children` maintenance, no tick on
        // the child — only the `Parent` membership count moves.
        world.remove::<Parent>(child);
        assert!(
            propagation_is_dirty(&world),
            "a direct Parent removal must probe dirty"
        );
        propagate_transforms(&mut world);

        assert!(
            (global_translation(&world, child) - glam::Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5,
            "child without a Parent link is a root again"
        );
    }

    #[test]
    fn manual_global_edit_is_repaired_on_the_next_run() {
        let mut world = World::new();
        let root = world.spawn();
        world
            .insert(
                root,
                Transform::from_translation(glam::Vec3::new(2.0, 0.0, 0.0)),
            )
            .unwrap();
        propagate_to_settled(&mut world);

        world.advance_tick();
        world
            .insert(root, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        assert!(
            propagation_is_dirty(&world),
            "an external GlobalTransform edit must probe dirty"
        );
        propagate_transforms(&mut world);

        assert_mat4_approx_eq(
            world.get::<GlobalTransform>(root).unwrap().matrix(),
            glam::Mat4::from_translation(glam::Vec3::new(2.0, 0.0, 0.0)),
        );
    }

    #[test]
    fn downstream_change_tick_contract_quiet_empty_then_dirty_on_move() {
        // The consumer-visible promise: quiet ticks (skipped or not) stamp
        // no `GlobalTransform` ticks, while a real move still surfaces
        // through `query_changed_since` — and full reads (what render
        // extraction does) see fresh values either way.
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
        propagate_to_settled(&mut world);

        world.advance_tick();
        let baseline = world.change_tick();
        world.advance_tick();
        assert!(
            !propagation_is_dirty(&world),
            "the watched tick must be quiet"
        );
        propagate_transforms(&mut world);

        assert!(
            world
                .query_changed_since::<GlobalTransform>(baseline)
                .next()
                .is_none(),
            "a quiet (skipped) tick must not dirty downstream change detection"
        );
        // Full reads still see the composed values — nothing starved.
        assert!(
            (global_translation(&world, child) - glam::Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5
        );

        world.advance_tick();
        world.get_mut::<Transform>(root).unwrap().translation = glam::Vec3::new(10.0, 0.0, 0.0);
        propagate_transforms(&mut world);

        assert!(
            world
                .query_changed_since::<GlobalTransform>(baseline)
                .next()
                .is_some(),
            "a real move must surface through downstream change detection"
        );
        assert!(
            (global_translation(&world, child) - glam::Vec3::new(13.0, 0.0, 0.0)).length() < 1e-5,
            "full reads see the fresh composed value after the move"
        );
    }

    /// Release-only pin for the non-finite skip above: a NaN local must
    /// retain the last good global (never poison it), and must not
    /// dirty change detection on re-runs. Gated on release because
    /// debug builds intentionally `debug_assert`-panic on the same
    /// input one line above — both behaviors are covered, each where
    /// it applies (this runs in the release-workflow test pass; the
    /// debug panic path is exercised by definition whenever a debug
    /// run feeds non-finite input).
    #[test]
    #[cfg(not(debug_assertions))]
    fn non_finite_global_is_skipped_never_poisoned() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .unwrap();
        propagate_transforms(&mut world);
        let good = world.get::<GlobalTransform>(entity).copied();

        // Poison the local transform, re-propagate twice: the global
        // must still equal the last good matrix (not NaN), and the
        // second run must not dirty the tick (NaN != NaN would).
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(f32::NAN, 0.0, 0.0)),
            )
            .unwrap();
        propagate_transforms(&mut world);
        let retained = world.get::<GlobalTransform>(entity).copied();
        assert_eq!(
            retained, good,
            "a non-finite local must retain the last good global, got {retained:?}"
        );
        assert!(
            retained.is_some_and(|g| g.matrix().is_finite()),
            "retained global must be finite, got {retained:?}"
        );
        world.advance_tick();
        let baseline = world.change_tick();
        world.advance_tick();
        propagate_transforms(&mut world);
        assert!(
            world
                .query_changed_since::<GlobalTransform>(baseline)
                .next()
                .is_none(),
            "a skipped non-finite entity must not dirty change detection"
        );
    }
}
