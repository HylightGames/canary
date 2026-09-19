// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Schedule wiring for the render bridge: the bake system, its access
//! declaration, and its registration helper.
//!
//! The bake system is the terminal read of the per-tick ECS pipeline:
//! [`register_transform_propagation`](canary_transform::register_transform_propagation)
//! runs first (recomputing `GlobalTransform` from `Transform` + hierarchy),
//! then [`bake_scene_system`] reads the fresh `GlobalTransform`s and snapshots
//! them into the [`BakedFrame`](crate::BakedFrame) resource. The GPU draw call
//! itself ([`draw_baked_frame`](crate::draw_baked_frame)) is deliberately *not*
//! a scheduled system — see [`bake_scene_system`] for why — and runs explicitly
//! after [`Schedule::run`](canary_scheduler::Schedule::run) returns.

use canary_assets::{AssetStore, Mesh};
use canary_ecs::World;
use canary_scheduler::{Schedule, SystemAccess};
use canary_transform::GlobalTransform;

use crate::{
    bake_scene_to_vertices, extract_mesh_scene, extract_scene, BakedFrame, MeshRenderable,
    Renderable,
};

/// Declares the bake system's data access: reads the `GlobalTransform` +
/// `Renderable` components, writes the `BakedFrame` resource.
///
/// The `writes_resource::<BakedFrame>()` clause is load-bearing, not
/// incidental, and exists for exactly one reason: the ordering mechanism this
/// module relies on.
///
/// # How ordering is guaranteed (registration order + solo-write staging)
///
/// [`Schedule`](canary_scheduler::Schedule) batches systems into stages via
/// `compute_stages` (`engine/canary-scheduler/src/schedule.rs`): a system
/// joins the current stage only if the stage is still entirely read-only, the
/// new system is itself read-only, and its access does not conflict with the
/// stage's cumulative access. Otherwise the current stage closes and a new one
/// starts — and, crucially, *every system that writes anything always gets a
/// stage entirely to itself*, even when its writes are provably disjoint from
/// everything around it.
///
/// Propagation (registered first) declares `writes::<GlobalTransform>` and so
/// occupies its own stage. Bake declares `reads::<GlobalTransform>` here, and
/// a write of `GlobalTransform` followed by a read of `GlobalTransform`
/// conflicts by [`SystemAccess`] read/write rules — so bake can never merge
/// into propagation's stage (or any earlier stage): it is forced into a later
/// stage that runs strictly after. Had bake declared only component/resource
/// reads, it would be read-only and could share a stage with other readers and
/// run *before* (or concurrently with) propagation, baking stale transforms.
/// The resource write exists to make bake a writer, and writers run alone, in
/// registration order, after the writers they follow.
///
/// # Why an App-level scheduler is not introduced here
///
/// The plan (`docs/architecture/rendering.md`, "the ECS-to-render
/// bridge") defers the App-level `Schedule` to v0.0.10+: this task wires ordering *within* the
/// ECS-owning subsystem's own `Schedule` only. Introducing a second scheduler
/// at the `App` layer now would split ordering authority across two schedules
/// with no cross-schedule conflict analysis to keep them consistent — a larger
/// redesign than a two-system pipeline needs.
pub fn bake_access() -> SystemAccess {
    SystemAccess::new()
        .reads::<GlobalTransform>()
        .reads::<Renderable>()
        .writes_resource::<BakedFrame>()
}

/// Bakes the current scene snapshot into the [`BakedFrame`](crate::BakedFrame)
/// resource: [`extract_scene`] → [`bake_scene_to_vertices`] → overwrite the
/// resource.
///
/// Runs as a scheduled write system (see [`register_render_bake`]); takes
/// `&mut World` and nothing else. That signature is a doctrine point, not an
/// accident:
///
/// # Why the GPU stays out of the `Schedule`
///
/// A scheduled system's body may capture device state only through a
/// `FnMut(&World)` / `FnMut(&mut World)` closure, so holding a
/// `&VulkanDevice` (or any `RenderDevice`) inside the closure ties the
/// device's borrow to the schedule's `'static` registration lifetime —
/// lifetime-fragile by construction, since the device is owned by `main()`'s
/// frame scope, not by the ECS world. Beyond lifetimes, it would violate the
/// "Extract, don't query" rule (`docs/architecture/rendering.md`: the render
/// graph consumes a read-only snapshot of ECS-visible render state prepared by
/// an explicit extract step, rather than passes reaching back into the live
/// `World` mid-frame). The [`BakedFrame`](crate::BakedFrame) resource *is*
/// that snapshot: plain floats, no device handles, no lifetimes. The device,
/// target, and pipeline therefore stay in `main()` (or the owning binary),
/// and [`draw_baked_frame`](crate::draw_baked_frame) is called explicitly
/// after [`Schedule::run`](canary_scheduler::Schedule::run) — the schedule
/// never sees the GPU.
///
/// # Overwrite semantics
///
/// [`World::insert_resource`] replaces any existing value of the same type
/// (there is at most one resource per type per world), so calling this system
/// every tick unconditionally overwrites last frame's bake — no
/// first-insert-vs-update branching is needed here, and a stale frame can
/// never survive a tick.
pub fn bake_scene_system(world: &mut World) {
    let items = extract_scene(world);
    let vertices = bake_scene_to_vertices(&items);
    world.insert_resource(BakedFrame { vertices });
}

/// Registers [`bake_scene_system`] on `schedule` as a write system with
/// [`bake_access`]'s declaration.
///
/// Must be called *after*
/// [`register_transform_propagation`](canary_transform::register_transform_propagation):
/// registration order is the first half of the ordering mechanism (solo-write
/// staging is the second — see [`bake_access`]), so registering bake first
/// would bake stale `GlobalTransform`s. The subsystem constructor owns this
/// order; see `engine/canary-runtime/src/main.rs`.
pub fn register_render_bake(schedule: &mut Schedule) {
    schedule.add_write_system(bake_access(), bake_scene_system);
}

/// Declares the mesh-bake system's data access: reads the `GlobalTransform` +
/// `MeshRenderable` components and the `AssetStore<Mesh>` resource, writes
/// the `BakedFrame` resource.
///
/// Every clause earns its place:
/// - `reads::<GlobalTransform>()` conflicts with propagation's
///   `writes::<GlobalTransform>`, so — like the soup bake — this system can
///   never merge into propagation's stage and always sees fresh globals.
/// - `reads_resource::<AssetStore<Mesh>>()` names the store read inside
///   [`extract_mesh_scene`](crate::extract_mesh_scene): the declaration is
///   documentation-as-code for the data dependency, and keeps this system
///   ordered after any future writer of that resource by the scheduler's
///   conflict rules.
/// - `writes_resource::<BakedFrame>()` makes this a solo-write stage of its
///   own (see [`bake_access`]'s staging docs), registered *after* the soup
///   bake — so it runs strictly after it, reads the soup-baked frame, and
///   appends. Registering it before the soup bake would let the soup
///   overwrite the mesh vertices instead.
///   The subsystem constructor owns this order; see
///   `engine/canary-runtime/src/main.rs`.
///
/// # Why append instead of merging the two bakes
///
/// One system extracting both soups would allow a single global
/// painter-sort across every triangle; two systems cannot (the mesh bake
/// sees only soup-baked floats, not the soup's per-triangle depths).
/// Mesh triangles are therefore sorted among themselves and drawn after
/// the soup — a documented limit, exact for non-overlapping scenes, and
/// the honest shape given an RHI with no depth buffer. A global merge
/// (or real depth) is deferred RHI work, not something to fake here.
/// Zero RHI churn either way: this system only appends floats to the same
/// [`BakedFrame`](crate::BakedFrame) the existing draw call uploads.
pub fn bake_mesh_access() -> SystemAccess {
    SystemAccess::new()
        .reads::<GlobalTransform>()
        .reads::<MeshRenderable>()
        .reads_resource::<AssetStore<Mesh>>()
        .writes_resource::<BakedFrame>()
}

/// Appends the file-loaded mesh scene to the [`BakedFrame`](crate::BakedFrame)
/// resource: [`extract_mesh_scene`] → [`bake_scene_to_vertices`] → extend.
///
/// Read-modify-write over the soup bake's output: starts from the current
/// frame's vertices (or an empty frame when no soup bake ran — a
/// mesh-only world is legitimate), extends with the mesh bake, and
/// overwrites the resource. Takes `&mut World` and nothing else, for the
/// same GPU-stays-out-of-the-`Schedule` reason
/// [`bake_scene_system`] documents.
///
/// A mesh-empty tick (no store, no live handles) returns early without
/// touching the resource: there is nothing to append, and rewriting an
/// identical frame would only churn the resource version for no pixels.
pub fn bake_mesh_scene_system(world: &mut World) {
    let items = extract_mesh_scene(world);
    if items.is_empty() {
        return;
    }
    let mut vertices = world
        .resource::<BakedFrame>()
        .map(|frame| frame.vertices.clone())
        .unwrap_or_default();
    vertices.extend(bake_scene_to_vertices(&items));
    world.insert_resource(BakedFrame { vertices });
}

/// Registers [`bake_mesh_scene_system`] on `schedule` as a write system with
/// [`bake_mesh_access`]'s declaration.
///
/// Must be called *after* [`register_render_bake`]: both systems write the
/// `BakedFrame` resource, so each occupies its own solo-write stage in
/// registration order — soup first, mesh appended after. Registering mesh
/// first would let the soup bake overwrite the mesh vertices. The subsystem
/// constructor owns this order; see `engine/canary-runtime/src/main.rs`.
pub fn register_mesh_render_bake(schedule: &mut Schedule) {
    schedule.add_write_system(bake_mesh_access(), bake_mesh_scene_system);
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_ecs::World;
    use canary_scheduler::Schedule;
    use canary_transform::{register_transform_propagation, Transform};

    /// A single in-front-of-camera triangle: object-space origin shape, flat
    /// red. Kept at the origin so that the global matrix's translation fully
    /// determines the baked output.
    fn red_triangle() -> Renderable {
        Renderable::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [1.0, 0.0, 0.0],
        )
    }

    #[test]
    fn bake_runs_after_propagation_sees_fresh_global() {
        // Given: an entity whose local Transform moved, but whose cached
        // GlobalTransform is still stale (as it is on any tick before
        // propagation runs).
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .unwrap();
        world.insert(entity, red_triangle()).unwrap();
        world
            .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();

        // When: propagation runs first, then bake — the subsystem order.
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        schedule.run(&mut world);

        // Then: the baked frame matches a bake of the *fresh* global, not
        // the stale identity that was cached before the tick.
        let frame = world
            .resource::<BakedFrame>()
            .expect("bake must insert the BakedFrame resource");
        let fresh = crate::RenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                1.0, 0.0, 0.0,
            ))),
            vertices: red_triangle().vertices.clone(),
            color: red_triangle().color,
        };
        let expected = bake_scene_to_vertices(std::slice::from_ref(&fresh));
        assert_eq!(
            frame.vertices.len(),
            expected.len(),
            "baked frame must reflect the propagated transform"
        );
        for (actual, want) in frame.vertices.iter().zip(expected.iter()) {
            assert!(
                (actual - want).abs() < 1e-5,
                "baked vertex {actual} differs from fresh-global bake {want}"
            );
        }
    }

    #[test]
    fn bake_access_is_not_read_only() {
        // Given/When: bake's access is registered as a *read* system.
        // Then: registration must panic — proving the declaration writes
        // (the `writes_resource::<BakedFrame>()` clause), which is what
        // forces bake into its own ordered stage after propagation instead
        // of merging into a shared read stage. (`SystemAccess::is_read_only`
        // is crate-private to `canary-scheduler`, so the panic on
        // `add_read_system`'s eager read-only check is the observable
        // contract from this crate.)
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut schedule = Schedule::new();
            schedule.add_read_system(bake_access(), |_| {});
        }));
        assert!(
            result.is_err(),
            "bake_access must declare a write, so registering it as a read system must fail"
        );
    }

    /// One file-loaded quad through the store: the shared mesh-bake
    /// fixture. Geometry comes from the checked-in `quad.glb` (never
    /// hand-rebuilt), tinted flat red; translation is applied by the
    /// caller via `Transform`.
    fn red_mesh_handle(world: &mut World) -> canary_assets::AssetHandle<canary_assets::Mesh> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/quad.glb");
        let mesh = canary_assets::load_mesh(&path)
            .expect("quad fixture must load")
            .into_iter()
            .next()
            .expect("quad fixture holds one mesh");
        world
            .resource_mut::<AssetStore<Mesh>>()
            .expect("mesh-bake tests must insert the AssetStore resource first")
            .insert(mesh)
    }

    #[test]
    fn mesh_bake_access_is_not_read_only() {
        // Given/When: the mesh bake's access is registered as a *read* system.
        // Then: registration must panic — proving the declaration writes
        // (the `writes_resource::<BakedFrame>()` clause), which is what
        // forces the mesh bake into its own solo-write stage after the soup
        // bake instead of merging into a shared read stage. Same contract
        // as `bake_access_is_not_read_only`, one stage later.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut schedule = Schedule::new();
            schedule.add_read_system(bake_mesh_access(), |_| {});
        }));
        assert!(
            result.is_err(),
            "bake_mesh_access must declare a write, so registering it as a read system must fail"
        );
    }

    #[test]
    fn mesh_bake_appends_after_soup_bake_with_fresh_global() {
        // Given: one soup triangle moved to x = -1 and one mesh quad moved
        // to x = +1, both with stale identity globals, plus the store.
        let mut world = World::new();
        world.insert_resource(AssetStore::<Mesh>::new());
        let soup_entity = world.spawn();
        world
            .insert(
                soup_entity,
                Transform::from_translation(glam::Vec3::new(-1.0, 0.0, 0.0)),
            )
            .unwrap();
        world.insert(soup_entity, red_triangle()).unwrap();
        world
            .insert(
                soup_entity,
                GlobalTransform::from_matrix(glam::Mat4::IDENTITY),
            )
            .unwrap();
        let mesh_entity = world.spawn();
        world
            .insert(
                mesh_entity,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .unwrap();
        world
            .insert(
                mesh_entity,
                GlobalTransform::from_matrix(glam::Mat4::IDENTITY),
            )
            .unwrap();
        let handle = red_mesh_handle(&mut world);
        world
            .insert(mesh_entity, MeshRenderable::new(handle, [0.0, 0.0, 1.0]))
            .unwrap();

        // When: propagation, then soup bake, then mesh bake — the
        // subsystem order.
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        schedule.run(&mut world);

        // Then: the frame holds the soup triangle (3 vertices) plus the
        // mesh quad (6 vertices), each baked through its *fresh* global.
        let frame = world
            .resource::<BakedFrame>()
            .expect("mesh bake must leave a BakedFrame resource");
        assert_eq!(
            frame.vertices.len(),
            (3 + 6) * 5,
            "soup triangle plus mesh quad must both reach the frame"
        );
        let soup_fresh = crate::RenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                -1.0, 0.0, 0.0,
            ))),
            vertices: red_triangle().vertices.clone(),
            color: red_triangle().color,
        };
        let expected_soup = bake_scene_to_vertices(std::slice::from_ref(&soup_fresh));
        assert_eq!(
            &frame.vertices[..expected_soup.len()],
            expected_soup.as_slice(),
            "soup vertices must come first, baked from the fresh global"
        );
        let mesh_soup = crate::expand_mesh_to_soup(
            world
                .resource::<AssetStore<Mesh>>()
                .expect("store must still exist")
                .get(handle)
                .expect("handle must still be live"),
        );
        let mesh_fresh = crate::RenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                1.0, 0.0, 0.0,
            ))),
            vertices: mesh_soup,
            color: [0.0, 0.0, 1.0],
        };
        let expected_mesh = bake_scene_to_vertices(std::slice::from_ref(&mesh_fresh));
        assert_eq!(
            &frame.vertices[expected_soup.len()..],
            expected_mesh.as_slice(),
            "mesh vertices must append after the soup, baked from the fresh global"
        );
    }

    #[test]
    fn mesh_bake_with_no_meshes_leaves_the_soup_frame_untouched() {
        // Given: a soup-only world run through all three systems.
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::from_translation(glam::Vec3::ZERO))
            .unwrap();
        world.insert(entity, red_triangle()).unwrap();
        world
            .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        schedule.run(&mut world);
        let before = world
            .resource::<BakedFrame>()
            .expect("soup bake must insert the frame")
            .vertices
            .clone();

        // When: the mesh system runs again directly (still no meshes).
        bake_mesh_scene_system(&mut world);

        // Then: the frame is byte-identical — the early return must not
        // rewrite (or clear) the soup's output.
        assert_eq!(
            world
                .resource::<BakedFrame>()
                .expect("frame must still exist")
                .vertices,
            before,
            "a mesh-empty tick must leave the soup frame untouched"
        );
    }
}
