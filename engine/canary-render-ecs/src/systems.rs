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

use canary_assets::{AssetStore, Mesh, Texture};
use canary_ecs::World;
use canary_scheduler::{Schedule, SystemAccess};
use canary_transform::GlobalTransform;

use crate::{
    bake_scene_to_vertices, bake_scene_to_vertices_into, bake_textured_scene_to_vertices,
    extract_mesh_scene, extract_scene_into, extract_textured_scene, BakeScratch, BakedFrame,
    BakedTexturedFrame, ExtractScratch, MeshRenderable, Renderable, TexturedRenderable,
};

/// Declares the bake system's data access: reads the `GlobalTransform` +
/// `Renderable` components, writes the `BakedFrame`, `ExtractScratch`, and
/// `BakeScratch` resources (the scratch writes are the take/put-back in
/// [`bake_scene_system`]; they keep the declaration honest for the
/// scheduler's conflict rules, and change no staging — a writer is
/// already solo).
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
        .writes_resource::<ExtractScratch>()
        .writes_resource::<BakeScratch>()
}

/// Bakes the current scene snapshot into the [`BakedFrame`](crate::BakedFrame)
/// resource: take the [`ExtractScratch`](crate::ExtractScratch) resource (or
/// a fresh buffer on the first tick) → [`extract_scene_into`] refreshes it
/// in place, reusing last tick's vertex buffers → take the
/// [`BakeScratch`](crate::BakeScratch) and [`BakedFrame`](crate::BakedFrame)
/// resources (or fresh buffers on the first tick) →
/// [`bake_scene_to_vertices_into`] refills the frame's own vertex buffer in
/// place, reusing last tick's pending-triangle and vertex allocations →
/// overwrite all three resources.
///
/// The scratch take/put-back is what makes the per-tick extract allocation
/// free in steady state: the system itself is stateless (`FnMut(&mut World)`
/// with no captured buffers — the scheduler's doctrine, pinned by the
/// sequential-reuse test below), so cross-tick buffers live in the world as
/// a resource, exactly like the [`BakedFrame`](crate::BakedFrame) they feed.
/// [`World::remove_resource`](canary_ecs::World::remove_resource) hands over
/// ownership (no borrow is held across the query), and re-inserting puts the
/// refreshed buffers back for the next tick. Output is value-identical to a
/// fresh extract every tick — any entity add/remove/resize only reallocates
/// the affected slots (see [`extract_scene_into`](crate::extract_scene_into)).
/// The bake side holds the same contract: [`bake_scene_to_vertices_into`]
/// clears and refills, so a shrunken triangle count can never leave stale
/// floats behind, and growth only `reserve`s the delta.
///
/// # Why reusing the frame's own buffer cannot mutate a live frame
///
/// At bake time the system *owns* all three buffers via `remove_resource`:
/// no borrow of the [`BakedFrame`](crate::BakedFrame) is held anywhere else
/// (this system runs in its own solo-write stage — see [`bake_access`] —
/// so no concurrent system can observe the taken resource). Downstream, the
/// only consumer is [`draw_baked_frame`](crate::draw_baked_frame), which runs
/// explicitly after [`Schedule::run`](canary_scheduler::Schedule::run)
/// returns: it copies the frame's floats into a fresh byte buffer
/// synchronously and `submit_and_wait` blocks until the GPU has consumed it.
/// By the time the next tick takes the buffer back, the previous frame's
/// pixels are already submitted — no in-flight draw can observe the refill.
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
/// never survive a tick. The same overwrite holds for the scratch: the
/// refreshed buffers replace last tick's, so a shrunken entity list can
/// never leave stale slots behind (the truncate in
/// [`extract_scene_into`](crate::extract_scene_into) already dropped them).
pub fn bake_scene_system(world: &mut World) {
    let mut extract_scratch = world
        .remove_resource::<ExtractScratch>()
        .map(|scratch| scratch.0)
        .unwrap_or_default();
    extract_scene_into(world, &mut extract_scratch);
    let mut bake_scratch = world.remove_resource::<BakeScratch>().unwrap_or_default();
    let mut vertices = world
        .remove_resource::<BakedFrame>()
        .map(|frame| frame.vertices)
        .unwrap_or_default();
    bake_scene_to_vertices_into(&extract_scratch, &mut vertices, &mut bake_scratch);
    world.insert_resource(ExtractScratch(extract_scratch));
    world.insert_resource(bake_scratch);
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
    let baked = bake_scene_to_vertices(&items);
    match world.resource_mut::<BakedFrame>() {
        Some(frame) => frame.vertices.extend(baked),
        None => world.insert_resource(BakedFrame { vertices: baked }),
    }
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

/// Declares the textured-bake system's data access: reads the
/// `GlobalTransform` + `TexturedRenderable` components and the
/// `AssetStore<Mesh>` + `AssetStore<Texture>` resources, writes the
/// `BakedTexturedFrame` resource.
///
/// Every clause earns its place:
/// - `reads::<GlobalTransform>()` conflicts with propagation's
///   `writes::<GlobalTransform>`, so — like both earlier bakes — this
///   system can never merge into propagation's stage and always sees
///   fresh globals.
/// - `reads_resource::<AssetStore<Mesh>>()` and
///   `reads_resource::<AssetStore<Texture>>()` name the two store reads
///   inside [`extract_textured_scene`](crate::extract_textured_scene):
///   documentation-as-code for the data dependencies, keeping this
///   system ordered after any future writer of either resource.
/// - `writes_resource::<BakedTexturedFrame>()` makes this a solo-write
///   stage of its own (see [`bake_access`]'s staging docs). It writes a
///   *different* resource than the soup/mesh bakes, so no overwrite
///   rivalry exists with them — but writers always run alone and in
///   registration order regardless, and registering after the mesh bake
///   keeps the per-tick chain deterministic: propagation, soup, mesh,
///   textured.
///
/// # Why overwrite instead of the mesh bake's append
///
/// The mesh bake appends because it shares the soup's [`BakedFrame`]
/// and must preserve it; the textured bake owns
/// [`BakedTexturedFrame`](crate::BakedTexturedFrame) outright — no
/// other system writes that type — so unconditional overwrite (the
/// soup bake's semantics) is correct, and a stale textured frame can
/// never survive a tick.
pub fn bake_textured_access() -> SystemAccess {
    SystemAccess::new()
        .reads::<GlobalTransform>()
        .reads::<TexturedRenderable>()
        .reads_resource::<AssetStore<Mesh>>()
        .reads_resource::<AssetStore<Texture>>()
        .writes_resource::<BakedTexturedFrame>()
}

/// Bakes the textured scene into the
/// [`BakedTexturedFrame`](crate::BakedTexturedFrame) resource:
/// [`extract_textured_scene`] →
/// [`bake_textured_scene_to_vertices`](crate::bake_textured_scene_to_vertices)
/// → overwrite the resource.
///
/// Takes `&mut World` and nothing else, for the same
/// GPU-stays-out-of-the-`Schedule` reason
/// [`bake_scene_system`] documents: the device, target, pipeline, and
/// the resolved [`Texture`] all stay in `main()`'s frame scope, and
/// [`draw_textured_frame`](crate::draw_textured_frame) is called
/// explicitly after [`Schedule::run`](canary_scheduler::Schedule::run).
pub fn bake_textured_scene_system(world: &mut World) {
    let items = extract_textured_scene(world);
    let vertices = bake_textured_scene_to_vertices(&items);
    world.insert_resource(BakedTexturedFrame { vertices });
}

/// Registers [`bake_textured_scene_system`] on `schedule` as a write
/// system with [`bake_textured_access`]'s declaration.
///
/// Must be called *after* [`register_mesh_render_bake`]: every writer
/// takes its own solo-write stage in registration order, so this keeps
/// the canonical per-tick chain propagation → soup → mesh → textured.
/// The subsystem constructor owns this order; see
/// `engine/canary-runtime/src/main.rs`.
pub fn register_textured_render_bake(schedule: &mut Schedule) {
    schedule.add_write_system(bake_textured_access(), bake_textured_scene_system);
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
    fn scratch_reuse_bakes_fresh_values_after_entity_churn() {
        // Given: one triangle baked for a tick, so the scratch resource
        // holds live buffers.
        let mut world = World::new();
        let first = world.spawn();
        world
            .insert(
                first,
                Transform::from_translation(glam::Vec3::new(-1.0, 0.0, 0.0)),
            )
            .unwrap();
        world.insert(first, red_triangle()).unwrap();
        world
            .insert(first, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        schedule.run(&mut world);
        assert!(
            world.resource::<ExtractScratch>().is_some(),
            "the first tick must leave the scratch resource behind"
        );

        // When: the entity despawns and two new ones spawn (one with a
        // bigger mesh), then the schedule runs again — the scratch
        // refreshes over stale slots with a shorter entity list.
        world.despawn(first).unwrap();
        for x in [2.0, 4.0] {
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform::from_translation(glam::Vec3::new(x, 0.0, 0.0)),
                )
                .unwrap();
            let mut verts = red_triangle().vertices.clone();
            if x > 3.0 {
                verts.extend_from_slice(&red_triangle().vertices);
            }
            world
                .insert(entity, Renderable::new(verts, [0.0, 1.0, 0.0]))
                .unwrap();
            world
                .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
                .unwrap();
        }
        schedule.run(&mut world);

        // Then: the frame equals a fresh extract+bake of the new world —
        // 3 + 6 vertices through fresh globals, with no ghost of the
        // despawned triangle.
        let frame = world
            .resource::<BakedFrame>()
            .expect("second tick must bake");
        assert_eq!(
            frame.vertices.len(),
            (3 + 6) * 5,
            "two triangles plus a doubled triangle, nothing stale"
        );
        let fresh_items = crate::extract_scene(&world);
        assert_eq!(fresh_items.len(), 2);
        assert_eq!(
            frame.vertices,
            bake_scene_to_vertices(&fresh_items),
            "the reused-scratch frame must equal a fresh extract+bake"
        );
    }

    #[test]
    fn second_tick_reuses_bake_buffers_with_identical_output() {
        // Given: one triangle baked for a tick, so the scratch and frame
        // resources hold live buffers.
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
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        schedule.run(&mut world);
        let first = world
            .resource::<BakedFrame>()
            .expect("first tick must bake")
            .vertices
            .clone();
        let first_ptr = world
            .resource::<BakedFrame>()
            .expect("first tick must bake")
            .vertices
            .as_ptr();

        // When: the schedule runs again with nothing changed (the steady
        // state: same entities, same triangle count).
        schedule.run(&mut world);

        // Then: the frame is byte-identical and lives at the same
        // allocation — the second bake refilled the taken-back buffer in
        // place instead of allocating a fresh one, and no live-frame
        // mutation is observable (the first clone predates the rerun).
        let frame = world
            .resource::<BakedFrame>()
            .expect("second tick must bake");
        assert_eq!(frame.vertices, first);
        assert_eq!(
            frame.vertices.as_ptr(),
            first_ptr,
            "the second tick must reuse the frame's allocation, not reallocate it"
        );
        assert_eq!(
            frame.vertices,
            bake_scene_to_vertices(&crate::extract_scene(&world)),
            "the reused-buffer frame must equal a fresh extract+bake"
        );
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

    /// One file-loaded textured quad through both stores: the shared
    /// textured-bake fixture. Geometry + UVs come from the checked-in
    /// `quad.glb`, the image from `rgba2x2.png` — never hand-rebuilt.
    fn textured_handles(
        world: &mut World,
    ) -> (
        canary_assets::AssetHandle<Mesh>,
        canary_assets::AssetHandle<canary_assets::Texture>,
    ) {
        use canary_assets::{load_texture, Texture};
        let mesh_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/quad.glb");
        let mesh = canary_assets::load_mesh(&mesh_path)
            .expect("quad fixture must load")
            .into_iter()
            .next()
            .expect("quad fixture holds one mesh");
        let texture_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/rgba2x2.png");
        let texture = load_texture(&texture_path).expect("RGBA fixture must load");
        let mesh_handle = world
            .resource_mut::<AssetStore<Mesh>>()
            .expect("textured-bake tests must insert the mesh store first")
            .insert(mesh);
        let texture_handle = world
            .resource_mut::<AssetStore<Texture>>()
            .expect("textured-bake tests must insert the texture store first")
            .insert(texture);
        (mesh_handle, texture_handle)
    }

    #[test]
    fn textured_bake_access_is_not_read_only() {
        // Given/When: the textured bake's access is registered as a
        // *read* system.
        // Then: registration must panic — proving the declaration
        // writes (the `writes_resource::<BakedTexturedFrame>()`
        // clause), which is what forces the textured bake into its own
        // solo-write stage after the mesh bake. Same contract as the
        // two earlier `*_is_not_read_only` tests, one stage later.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut schedule = Schedule::new();
            schedule.add_read_system(bake_textured_access(), |_| {});
        }));
        assert!(
            result.is_err(),
            "bake_textured_access must declare a write, so registering it as a read system must fail"
        );
    }

    #[test]
    fn textured_bake_sees_fresh_global_and_owns_its_frame() {
        // Given: one textured quad moved to x = +1 with a stale
        // identity global, plus both stores.
        let mut world = World::new();
        world.insert_resource(AssetStore::<Mesh>::new());
        world.insert_resource(AssetStore::<canary_assets::Texture>::new());
        let (mesh_handle, texture_handle) = textured_handles(&mut world);
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .unwrap();
        world
            .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        world
            .insert(entity, TexturedRenderable::new(mesh_handle, texture_handle))
            .unwrap();

        // When: propagation, then all three bakes — the subsystem order.
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        register_textured_render_bake(&mut schedule);
        schedule.run(&mut world);

        // Then: the textured frame holds the quad (6 vertices × 4
        // floats), baked through the *fresh* global — and the soup
        // frame is untouched (empty: no soup entities exist).
        let frame = world
            .resource::<BakedTexturedFrame>()
            .expect("textured bake must leave a BakedTexturedFrame resource");
        assert_eq!(
            frame.vertices.len(),
            6 * 4,
            "one textured quad must reach the textured frame"
        );
        let soup = crate::expand_mesh_to_textured_soup(
            world
                .resource::<AssetStore<Mesh>>()
                .expect("store must still exist")
                .get(mesh_handle)
                .expect("handle must still be live"),
        )
        .expect("quad fixture carries UVs");
        let (positions, uvs): (Vec<[f32; 3]>, Vec<[f32; 2]>) = soup.into_iter().unzip();
        let fresh = crate::TexturedRenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                1.0, 0.0, 0.0,
            ))),
            vertices: positions,
            uvs,
        };
        let expected = crate::bake_textured_scene_to_vertices(std::slice::from_ref(&fresh));
        assert_eq!(
            frame.vertices, expected,
            "textured vertices must be baked from the fresh global"
        );
        assert!(
            world
                .resource::<BakedFrame>()
                .expect("soup bake must still insert its own frame")
                .is_empty(),
            "no soup entities means the soup frame stays empty alongside the textured one"
        );
    }

    #[test]
    fn textured_bake_with_no_textured_entities_leaves_an_empty_frame() {
        // Given: a soup-only world run through all four systems.
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
        register_textured_render_bake(&mut schedule);
        schedule.run(&mut world);

        // Then: the textured bake still inserts its resource (overwrite
        // semantics, like the soup bake), empty — while the soup frame
        // holds the triangle.
        let textured = world
            .resource::<BakedTexturedFrame>()
            .expect("textured bake must insert its frame even with no textured entities");
        assert!(
            textured.is_empty(),
            "no textured entities must bake to an empty textured frame"
        );
        assert_eq!(
            world
                .resource::<BakedFrame>()
                .expect("soup frame must still exist")
                .vertex_count(),
            3,
            "the soup triangle must survive alongside the empty textured frame"
        );
    }

    #[test]
    fn full_chain_bakes_soup_mesh_and_textured_with_fresh_globals() {
        // Given: one soup triangle, one mesh quad, one textured quad —
        // each moved off-origin with a stale identity global, plus both
        // stores. Stale globals mean only the canonical registration
        // order can produce fresh output in every frame.
        let mut world = World::new();
        world.insert_resource(AssetStore::<Mesh>::new());
        world.insert_resource(AssetStore::<canary_assets::Texture>::new());
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
        let mesh_handle = red_mesh_handle(&mut world);
        world
            .insert(
                mesh_entity,
                MeshRenderable::new(mesh_handle, [0.0, 0.0, 1.0]),
            )
            .unwrap();
        let (tex_mesh_handle, texture_handle) = textured_handles(&mut world);
        let textured_entity = world.spawn();
        world
            .insert(
                textured_entity,
                Transform::from_translation(glam::Vec3::new(0.0, 1.0, 0.0)),
            )
            .unwrap();
        world
            .insert(
                textured_entity,
                GlobalTransform::from_matrix(glam::Mat4::IDENTITY),
            )
            .unwrap();
        world
            .insert(
                textured_entity,
                TexturedRenderable::new(tex_mesh_handle, texture_handle),
            )
            .unwrap();

        // When: all four write systems in canonical registration order.
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        register_textured_render_bake(&mut schedule);
        schedule.run(&mut world);

        // Then: soup frame holds triangle + mesh quad (soup first), and
        // the textured frame holds its own quad — every vertex baked
        // through a fresh global, proving the solo-write chain ran each
        // stage exactly once, in order.
        let frame = world
            .resource::<BakedFrame>()
            .expect("bakes must leave a BakedFrame resource");
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
        let textured = world
            .resource::<BakedTexturedFrame>()
            .expect("textured bake must leave its own frame");
        assert_eq!(
            textured.vertices.len(),
            6 * 4,
            "one textured quad must reach the textured frame"
        );
        assert_eq!(
            world
                .resource::<BakedFrame>()
                .expect("frame must still exist")
                .vertex_count(),
            9,
            "mesh bake must have appended (not duplicated or dropped) after soup"
        );
    }

    #[test]
    fn mesh_bake_before_soup_bake_loses_the_mesh_output() {
        // Given: one soup triangle and one mesh quad, stale globals, the
        // store — identical setup to the append test.
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

        // When: the mesh bake runs BEFORE the soup bake — the reversed
        // registration order. This must compile (ordering is a
        // constructor discipline, not a type rule) but produce stale
        // behavior: the soup overwrite wipes the mesh append.
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        register_render_bake(&mut schedule);
        schedule.run(&mut world);

        // Then: only the soup triangle survives — proving registration
        // order (not the scheduler, not the types) is the ordering
        // mechanism, and the canonical order is load-bearing.
        let frame = world
            .resource::<BakedFrame>()
            .expect("bakes must leave a BakedFrame resource");
        assert_eq!(
            frame.vertices.len(),
            3 * 5,
            "reversed order must lose the mesh quad to the soup overwrite"
        );
    }

    #[test]
    fn textured_bake_registered_first_bakes_stale_globals() {
        let mut world = World::new();
        world.insert_resource(AssetStore::<Mesh>::new());
        world.insert_resource(AssetStore::<canary_assets::Texture>::new());
        let (mesh_handle, texture_handle) = textured_handles(&mut world);
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(0.0, 1.0, 0.0)),
            )
            .unwrap();
        world
            .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        world
            .insert(entity, TexturedRenderable::new(mesh_handle, texture_handle))
            .unwrap();

        let mut schedule = Schedule::new();
        register_textured_render_bake(&mut schedule);
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        schedule.run(&mut world);

        let frame = world
            .resource::<BakedTexturedFrame>()
            .expect("textured bake must leave its frame even in the wrong order");
        assert_eq!(frame.vertices.len(), 6 * 4);
        let soup = crate::expand_mesh_to_textured_soup(
            world
                .resource::<AssetStore<Mesh>>()
                .expect("store must still exist")
                .get(mesh_handle)
                .expect("handle must still be live"),
        )
        .expect("quad fixture carries UVs");
        let (positions, uvs): (Vec<[f32; 3]>, Vec<[f32; 2]>) = soup.into_iter().unzip();
        let stale = crate::TexturedRenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::IDENTITY),
            vertices: positions.clone(),
            uvs: uvs.clone(),
        };
        let stale_bake = crate::bake_textured_scene_to_vertices(std::slice::from_ref(&stale));
        assert_eq!(
            frame.vertices, stale_bake,
            "a bake-first registration must bake the stale identity global"
        );
        let fresh = crate::TexturedRenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                0.0, 1.0, 0.0,
            ))),
            vertices: positions,
            uvs,
        };
        let fresh_bake = crate::bake_textured_scene_to_vertices(std::slice::from_ref(&fresh));
        assert_ne!(
            frame.vertices, fresh_bake,
            "the bake-first frame must NOT match the fresh-global output"
        );
    }

    #[test]
    fn mesh_bake_without_a_prior_soup_frame_builds_from_empty() {
        let mut world = World::new();
        world.insert_resource(AssetStore::<Mesh>::new());
        assert!(
            world.resource::<BakedFrame>().is_none(),
            "precondition: no soup bake has run, so no frame exists"
        );
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .unwrap();
        world
            .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        let handle = red_mesh_handle(&mut world);
        world
            .insert(entity, MeshRenderable::new(handle, [0.0, 0.0, 1.0]))
            .unwrap();

        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        schedule.run(&mut world);

        let frame = world
            .resource::<BakedFrame>()
            .expect("mesh bake must create the frame from nothing");
        assert_eq!(
            frame.vertices.len(),
            6 * 5,
            "one mesh quad with no soup bake must still reach the frame"
        );
        let mesh_soup = crate::expand_mesh_to_soup(
            world
                .resource::<AssetStore<Mesh>>()
                .expect("store must still exist")
                .get(handle)
                .expect("handle must still be live"),
        );
        let fresh = crate::RenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                1.0, 0.0, 0.0,
            ))),
            vertices: mesh_soup,
            color: [0.0, 0.0, 1.0],
        };
        assert_eq!(
            frame.vertices,
            bake_scene_to_vertices(std::slice::from_ref(&fresh)),
            "the mesh-only frame must be baked from the fresh global"
        );
    }

    #[test]
    fn stale_mesh_handle_through_the_full_schedule_skips_leaving_soup_intact() {
        let mut world = World::new();
        world.insert_resource(AssetStore::<Mesh>::new());
        world.insert_resource(AssetStore::<canary_assets::Texture>::new());
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
        let stale = world
            .resource_mut::<AssetStore<Mesh>>()
            .expect("mesh store must exist")
            .insert(
                canary_assets::load_mesh(
                    &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../canary-assets/tests/fixtures/quad.glb"),
                )
                .expect("quad fixture must load")
                .into_iter()
                .next()
                .expect("quad fixture holds one mesh"),
            );
        world
            .resource_mut::<AssetStore<Mesh>>()
            .expect("mesh store must exist")
            .remove(stale);
        world
            .insert(mesh_entity, MeshRenderable::new(stale, [0.0, 0.0, 1.0]))
            .unwrap();

        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        register_textured_render_bake(&mut schedule);
        schedule.run(&mut world);

        let frame = world
            .resource::<BakedFrame>()
            .expect("bakes must leave a BakedFrame resource");
        assert_eq!(
            frame.vertices.len(),
            3 * 5,
            "the stale mesh entity must skip; only the soup triangle bakes"
        );
    }

    #[test]
    fn second_tick_overwrites_the_first_frame() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(-1.0, 0.0, 0.0)),
            )
            .unwrap();
        world.insert(entity, red_triangle()).unwrap();
        world
            .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        schedule.run(&mut world);
        let first = world
            .resource::<BakedFrame>()
            .expect("first tick must bake")
            .vertices
            .clone();
        assert_eq!(first.len(), 3 * 5);

        *world
            .get_mut::<Transform>(entity)
            .expect("entity must still hold its Transform") =
            Transform::from_translation(glam::Vec3::new(2.0, 0.0, 0.0));
        schedule.run(&mut world);

        let second = world
            .resource::<BakedFrame>()
            .expect("second tick must bake")
            .vertices
            .clone();
        assert_eq!(
            second.len(),
            3 * 5,
            "the second tick must overwrite with one triangle, not append"
        );
        assert_ne!(
            first, second,
            "the second frame must reflect the moved transform, not the stale first bake"
        );
        let fresh = crate::RenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                2.0, 0.0, 0.0,
            ))),
            vertices: red_triangle().vertices.clone(),
            color: red_triangle().color,
        };
        assert_eq!(
            second,
            bake_scene_to_vertices(std::slice::from_ref(&fresh)),
            "the second frame must equal a fresh bake of the moved entity"
        );
    }

    #[test]
    fn third_tick_matches_a_fresh_bake_sequential_reuse_holds_no_state() {
        // Given: one entity baked through the same schedule twice already.
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(-1.0, 0.0, 0.0)),
            )
            .unwrap();
        world.insert(entity, red_triangle()).unwrap();
        world
            .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
            .unwrap();
        let mut schedule = Schedule::new();
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);

        // When: the schedule runs three times with a move before each rerun.
        schedule.run(&mut world);
        *world
            .get_mut::<Transform>(entity)
            .expect("entity must still hold its Transform") =
            Transform::from_translation(glam::Vec3::new(2.0, 0.0, 0.0));
        schedule.run(&mut world);
        *world
            .get_mut::<Transform>(entity)
            .expect("entity must still hold its Transform") =
            Transform::from_translation(glam::Vec3::new(-3.0, 0.0, 0.0));
        schedule.run(&mut world);

        // Then: the third frame equals a fresh bake of the twice-moved
        // entity — no per-run state leaks across sequential runs (the
        // scheduler holds no `static`/`thread_local` state; systems are
        // `FnMut(&mut World)` closures drained fresh each run).
        let third = world
            .resource::<BakedFrame>()
            .expect("third tick must bake")
            .vertices
            .clone();
        assert_eq!(third.len(), 3 * 5);
        let fresh = crate::RenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                -3.0, 0.0, 0.0,
            ))),
            vertices: red_triangle().vertices.clone(),
            color: red_triangle().color,
        };
        assert_eq!(
            third,
            bake_scene_to_vertices(std::slice::from_ref(&fresh)),
            "the third frame must equal a fresh bake, proving sequential reuse is stateless"
        );
    }
}
