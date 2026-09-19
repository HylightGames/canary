// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Offscreen readback pixel assertions for the ECS-to-render bridge.
//!
//! These tests prove **real pixels from real ECS state**: entities are spawned
//! into a [`World`](canary_ecs::World) with `Transform` + `GlobalTransform` +
//! [`Renderable`](canary_render_ecs::Renderable), a `Schedule` running
//! propagation-then-bake produces the [`BakedFrame`](canary_render_ecs::BakedFrame)
//! resource, [`draw_baked_frame`](canary_render_ecs::draw_baked_frame) issues it
//! to a real Vulkan device, and the assertions read back real RGBA8 pixels.
//! Nothing here trusts the bake math on its own authority — the pure-math side
//! is already pinned by unit tests in `src/extract.rs`; these tests close the
//! loop through rasterization.
//!
//! # Why `#[ignore]`
//!
//! Every test here needs a real Vulkan ICD (a software one is enough), which
//! is not guaranteed in every environment `cargo test` runs in. This is the
//! same gating as `canary-render-vulkan`'s `hello_triangle` and
//! `canary-platform`'s `winit_backend_window`: the normal suite must stay
//! green everywhere, and the GPU suite runs explicitly via
//! `cargo test -p canary-render-ecs -- --ignored` wherever an ICD exists.
//!
//! # What CI runs, and what a pass proves
//!
//! The GPU job installs `mesa-vulkan-drivers` (llvmpipe/lavapipe software
//! rasterization — no physical GPU required) and runs the `-- --ignored`
//! invocation above. A pass on llvmpipe proves the full CPU-side chain
//! (extract → bake → buffer upload → one-draw record → readback) is correct:
//! llvmpipe is a conformant Vulkan rasterizer, so wrong vertex layout, wrong
//! stride, flipped math, or stale-frame uploads all show up as wrong pixels.
//! What llvmpipe does *not* prove is real-GPU performance or
//! driver-specific edge behavior (tile-based quirks, exact float rounding at
//! triangle edges) — which is why every assertion below uses
//! dominant-channel/`> 10`-style thresholds inherited from `hello_triangle`
//! rather than exact `u8` equality across the rasterized pipeline. The single
//! exception is the clear color: cleared-but-never-drawn pixels never pass
//! through interpolation, so exact `[0, 0, 0, 255]` equality is sound there
//! (and is exactly what `hello_triangle` asserts for its corner pixel).
//!
//! # Scene geometry (shared by all tests)
//!
//! The bridge bakes with a square aspect, `CAMERA_DISTANCE 3.2` and
//! `FOCAL_LENGTH 2.2`, so an object-space `x` at world `z = 0` lands at NDC
//! `x * 2.2 / 3.2 = x * 0.6875`. A quad of half-size `0.25` centered at world
//! `x = -1.0` spans NDC `[-0.86, -0.52]` — pixels `9..31` on the shared
//! 128-wide target, sampled at `x = 20` — while the mirror quad at `+1.0`
//! spans pixels `97..119`, sampled at `x = 108`. Both sit on the vertical
//! center row (`y = 64`, symmetric about `y = 0`), so the samples are robust
//! to any top/bottom row-order convention in the readback path. The `±9px`
//! margin from each quad's edges keeps the samples clear of rasterizer edge
//! rules on any conformant implementation.

use canary_assets::{load_mesh, AssetHandle, AssetStore, Mesh};
use canary_ecs::World;
use canary_render::{ColorTargetDescriptor, PipelineDescriptor, RenderDevice};
use canary_render_ecs::register_render_bake;
use canary_render_ecs::{
    draw_baked_frame, register_mesh_render_bake, render_vertex_attributes, render_vertex_stride,
    BakedFrame, MeshRenderable, Renderable, RENDER_WGSL,
};
use canary_render_vulkan::VulkanDevice;
use canary_scheduler::Schedule;
use canary_transform::{register_transform_propagation, GlobalTransform, Transform};

/// Shared offscreen target size: square (so the default square-aspect bake is
/// exact) and large enough that the `±1.0`-offset quads land well interior.
const WIDTH: u32 = 128;
/// Shared offscreen target height; equals [`WIDTH`] (square target).
const HEIGHT: u32 = 128;

/// World-space X offset placing a quad's NDC center at `∓0.6875`: pixels 20
/// (left) and 108 (right) on the 128-wide target, `±11px` wide quads.
const QUAD_OFFSET_X: f32 = 1.0;
/// Object-space half-size of the test quads: NDC half-width `0.25 * 0.6875`.
const QUAD_HALF_SIZE: f32 = 0.25;

/// Pixel read back from the center row above the left quad's interior.
const LEFT_SAMPLE: (u32, u32) = (20, 64);
/// Pixel read back from the center row above the right quad's interior.
const RIGHT_SAMPLE: (u32, u32) = (108, 64);
/// Pixel on the center row between the quads: must stay the clear color.
/// Left quad spans `9..31`, right spans `97..119`, so `x = 64` is far clear.
const GAP_SAMPLE: (u32, u32) = (64, 64);

/// Parses, validates, and cross-compiles [`RENDER_WGSL`] to SPIR-V for one
/// shader stage.
///
/// The same real `naga` calls `hello_triangle` makes against its own
/// `WGSL_SOURCE` — the bridge's shader is byte-identical to that source, so
/// this harness compiles the bridge constant through the identical pipeline
/// rather than duplicating the string.
fn compile_stage(stage: naga::ShaderStage) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(RENDER_WGSL).expect("failed to parse WGSL");
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    let info = validator.validate(&module).expect("WGSL failed validation");

    let entry_point = match stage {
        naga::ShaderStage::Vertex => "vs_main",
        naga::ShaderStage::Fragment => "fs_main",
        naga::ShaderStage::Compute => unreachable!("no compute stage in this test's shader"),
    };
    let options = naga::back::spv::Options {
        lang_version: (1, 0),
        ..Default::default()
    };
    let pipeline_options = naga::back::spv::PipelineOptions {
        shader_stage: stage,
        entry_point: entry_point.to_string(),
    };
    let mut buffer = Vec::new();
    naga::back::spv::Writer::new(&options)
        .expect("failed to create SPIR-V writer")
        .write(&module, &info, Some(&pipeline_options), &None, &mut buffer)
        .expect("failed to write SPIR-V");
    buffer
}

/// Reads one RGBA8 pixel out of a tightly-packed readback buffer.
fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * width + x) * 4) as usize;
    [
        pixels[idx],
        pixels[idx + 1],
        pixels[idx + 2],
        pixels[idx + 3],
    ]
}

/// Creates a real Vulkan device, panicking with the ICD hint when none exists.
///
/// Shared helper (not a test): every test needs the same device-or-clean-error
/// behavior, and a missing ICD must read as "install the driver", never as a
/// bake failure.
fn real_device() -> VulkanDevice {
    VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (CI needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    })
}

/// Builds the bridge pipeline against `device`: stride and attributes come
/// from the bridge's own layout functions (not hard-coded here), so a layout
/// regression fails loudly as wrong pixels rather than silently diverging.
fn bridge_pipeline(device: &VulkanDevice) -> <VulkanDevice as RenderDevice>::Pipeline {
    let vertex_spirv = compile_stage(naga::ShaderStage::Vertex);
    let fragment_spirv = compile_stage(naga::ShaderStage::Fragment);
    let vertex_attributes = render_vertex_attributes();
    device.create_pipeline(&PipelineDescriptor {
        label: "render-ecs readback pipeline",
        vertex_shader_spirv: &vertex_spirv,
        vertex_entry_point: "vs_main",
        fragment_shader_spirv: &fragment_spirv,
        fragment_entry_point: "fs_main",
        vertex_stride: render_vertex_stride(),
        vertex_attributes: &vertex_attributes,
    })
}

/// Draws the world's current [`BakedFrame`] resource to `target` and returns
/// the tightly-packed RGBA8 readback.
///
/// One fresh buffer per frame inside [`draw_baked_frame`] (the RHI's
/// write-once model), then `submit_and_wait` + `read_color_target_rgba8` —
/// the per-frame record pattern the bridge owns.
fn draw_and_readback(
    device: &VulkanDevice,
    target: &<VulkanDevice as RenderDevice>::ColorTarget,
    pipeline: &<VulkanDevice as RenderDevice>::Pipeline,
    world: &World,
) -> Vec<u8> {
    let frame = world
        .resource::<BakedFrame>()
        .expect("schedule.run() must have baked a BakedFrame resource");
    draw_baked_frame(device, target, pipeline, frame);
    let pixels = device.read_color_target_rgba8(target);
    assert_eq!(
        pixels.len(),
        (WIDTH * HEIGHT * 4) as usize,
        "readback should be tightly packed RGBA8 with no row padding"
    );
    pixels
}

/// Two triangles forming a `2s`-sided quad centered on the origin in the
/// `z = 0` plane.
fn quad_vertices(s: f32) -> Vec<[f32; 3]> {
    vec![
        [-s, -s, 0.0],
        [s, -s, 0.0],
        [s, s, 0.0],
        [-s, -s, 0.0],
        [s, s, 0.0],
        [-s, s, 0.0],
    ]
}

/// Spawns one flat-colored quad at world `(x, 0, 0)` with a deliberately stale
/// identity [`GlobalTransform`]: the schedule's propagation pass (registered
/// before bake) is what makes the global fresh, so every test exercises the
/// real per-tick chain rather than a hand-filled global.
fn spawn_quad(world: &mut World, x: f32, color: [f32; 3]) -> canary_ecs::Entity {
    let entity = world.spawn();
    world
        .insert(
            entity,
            Transform::from_translation(glam::Vec3::new(x, 0.0, 0.0)),
        )
        .expect("fresh entity accepts Transform");
    world
        .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
        .expect("fresh entity accepts GlobalTransform");
    world
        .insert(
            entity,
            Renderable::new(quad_vertices(QUAD_HALF_SIZE), color),
        )
        .expect("fresh entity accepts Renderable");
    entity
}

/// Spawns one file-loaded mesh quad at world `(x, 0, 0)`, flat-colored.
///
/// The `quad.glb` fixture is a half-size-`0.5` quad, twice the
/// [`QUAD_HALF_SIZE`] the soup helper builds — so the entity's `Transform`
/// carries a `0.5` scale, making the mesh cover exactly the same pixels a
/// hand-fed [`quad_vertices`] quad would at the same offset. Stale identity
/// [`GlobalTransform`] again, so the schedule's propagation is what makes
/// it fresh.
fn spawn_mesh_quad(
    world: &mut World,
    handle: AssetHandle<Mesh>,
    x: f32,
    color: [f32; 3],
) -> canary_ecs::Entity {
    let entity = world.spawn();
    let mut transform = Transform::from_translation(glam::Vec3::new(x, 0.0, 0.0));
    transform.scale = glam::Vec3::splat(0.5);
    world
        .insert(entity, transform)
        .expect("fresh entity accepts Transform");
    world
        .insert(entity, GlobalTransform::from_matrix(glam::Mat4::IDENTITY))
        .expect("fresh entity accepts GlobalTransform");
    world
        .insert(entity, MeshRenderable::new(handle, color))
        .expect("fresh entity accepts MeshRenderable");
    entity
}

/// Loads the checked-in `quad.glb` fixture into a fresh [`AssetStore`],
/// returning the store plus the live handle for its single mesh.
///
/// The path is manifest-relative (this test's crate dir), never CWD-
/// relative: `cargo test` may run from anywhere, and a missing fixture
/// must read as a loader failure, never as an empty scene that still
/// clears green.
fn mesh_store_with_quad() -> (AssetStore<Mesh>, AssetHandle<Mesh>) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../canary-assets/tests/fixtures/quad.glb");
    let mesh = load_mesh(&path)
        .expect("quad fixture must load")
        .into_iter()
        .next()
        .expect("quad fixture holds one mesh");
    let mut store = AssetStore::new();
    let handle = store.insert(mesh);
    (store, handle)
}

/// Registers the full per-tick chain: propagation, then soup bake, then
/// mesh bake. Registration order is the ordering mechanism (each writer
/// takes its own solo-write stage), so this helper — not each test's
/// inline schedule — is the single source of truth for system order.
fn full_render_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    register_transform_propagation(&mut schedule);
    register_render_bake(&mut schedule);
    register_mesh_render_bake(&mut schedule);
    schedule
}
/// Asserts `pixel` is dominantly `channel` (value `> 150`) with the other two
/// channels quiet (`< 80`).
///
/// Flat per-entity colors mean interior pixels should read back near `255` on
/// exactly one channel; the thresholds leave wide room for conformant
/// rasterizer rounding while still rejecting the clear color, a wrong entity's
/// color, or a half-blended edge — the `hello_triangle` dominant-channel
/// style, tightened slightly because flat quads have no interpolation to be
/// generous about.
fn assert_dominant_channel(pixel: [u8; 4], channel: usize, context: &str) {
    assert_eq!(
        pixel[3], 255,
        "{context}: alpha should be opaque, got {pixel:?}"
    );
    for (i, name) in ["red", "green", "blue"].iter().enumerate() {
        if i == channel {
            assert!(
                pixel[i] > 150,
                "{context}: expected dominant {name}, got {pixel:?}"
            );
        } else {
            assert!(
                pixel[i] < 80,
                "{context}: expected quiet {name}, got {pixel:?}"
            );
        }
    }
}

/// Two quads at non-overlapping NDC render in their own entity colors.
///
/// **What this proves:** the full multi-entity path — `extract_scene` picks up
/// *both* entities (not just the first), `bake_scene_to_vertices` projects
/// each through its *own* `GlobalTransform` (a shared or dropped transform
/// would stack the quads), the per-entity flat color survives the upload
/// (swapped colors fail the per-side dominant-channel asserts), and one draw
/// over the single baked buffer rasterizes both. The gap pixel staying clear
/// additionally proves the quads did not smear across the target (wrong
/// aspect, wrong stride, or a broken projection would move or stretch them
/// into the middle).
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn two_entities_render_distinct_colors() {
    // Given: two quads at non-overlapping world X, red left and blue right,
    // baked through the real propagation-then-bake schedule.
    let mut world = World::new();
    spawn_quad(&mut world, -QUAD_OFFSET_X, [1.0, 0.0, 0.0]);
    spawn_quad(&mut world, QUAD_OFFSET_X, [0.0, 0.0, 1.0]);
    let mut schedule = Schedule::new();
    register_transform_propagation(&mut schedule);
    register_render_bake(&mut schedule);
    schedule.run(&mut world);

    // When: the baked frame is drawn once to a fresh 128x128 target.
    let device = real_device();
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let pipeline = bridge_pipeline(&device);
    let pixels = draw_and_readback(&device, &target, &pipeline, &world);

    // Then: each quad's interior shows its own entity color, the quads differ
    // from each other, and the gap between them is the clear color.
    let left = pixel_at(&pixels, WIDTH, LEFT_SAMPLE.0, LEFT_SAMPLE.1);
    assert_dominant_channel(left, 0, "left quad interior should be entity red");
    let right = pixel_at(&pixels, WIDTH, RIGHT_SAMPLE.0, RIGHT_SAMPLE.1);
    assert_dominant_channel(right, 2, "right quad interior should be entity blue");
    assert_ne!(
        left, right,
        "the two entities must render distinct colors, got {left:?} and {right:?}"
    );
    // Interior robustness: neighbors of each sample agree (proves quad
    // coverage, not a one-pixel sliver a broken projection could fake).
    for (x, y) in [(16, 64), (24, 64), (20, 60), (20, 68)] {
        assert_dominant_channel(
            pixel_at(&pixels, WIDTH, x, y),
            0,
            "left quad neighbor ({x}, {y}) should stay red",
        );
    }
    for (x, y) in [(104, 64), (112, 64), (108, 60), (108, 68)] {
        assert_dominant_channel(
            pixel_at(&pixels, WIDTH, x, y),
            2,
            "right quad neighbor ({x}, {y}) should stay blue",
        );
    }
    assert_eq!(
        pixel_at(&pixels, WIDTH, GAP_SAMPLE.0, GAP_SAMPLE.1),
        [0, 0, 0, 255],
        "the gap between the quads should be the clear color"
    );
}

/// Moving an entity's `Transform` moves its pixels on the next redraw.
///
/// **What this proves (the end-to-end chain):** `Transform` → propagation →
/// fresh `GlobalTransform` → bake → draw. After the move and a second
/// `schedule.run()`, the old interior pixel must return to the exact clear
/// color (the stale frame was *replaced*, not painted over — `bake`
/// overwrites the `BakedFrame` resource and the redraw re-clears the target)
/// and the mirrored pixel must show the entity color (propagation actually
/// ran — had bake read the stale pre-move global, the quad would still raster
/// at the old position and *both* asserts would fail in opposite directions).
/// This is the single test that fails if propagation and bake ever run in the
/// wrong order: bake-first would bake the stale global and the new pixel
/// would stay clear.
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn moved_entity_redraws_moved() {
    // Given: one red quad on the left, baked and drawn to a shared target.
    let mut world = World::new();
    let entity = spawn_quad(&mut world, -QUAD_OFFSET_X, [1.0, 0.0, 0.0]);
    let mut schedule = Schedule::new();
    register_transform_propagation(&mut schedule);
    register_render_bake(&mut schedule);
    schedule.run(&mut world);

    let device = real_device();
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let pipeline = bridge_pipeline(&device);
    let first = draw_and_readback(&device, &target, &pipeline, &world);
    assert_dominant_channel(
        pixel_at(&first, WIDTH, LEFT_SAMPLE.0, LEFT_SAMPLE.1),
        0,
        "pre-move: left interior should be entity red",
    );

    // When: the Transform moves to the mirror position, the schedule re-runs,
    // and the SAME target is redrawn (shared target, so stale pixels would
    // survive a missing clear).
    world
        .get_mut::<Transform>(entity)
        .expect("quad entity still holds its Transform")
        .translation
        .x = QUAD_OFFSET_X;
    schedule.run(&mut world);
    let second = draw_and_readback(&device, &target, &pipeline, &world);

    // Then: the old interior is back to the clear color and the new interior
    // shows the entity color.
    assert_eq!(
        pixel_at(&second, WIDTH, LEFT_SAMPLE.0, LEFT_SAMPLE.1),
        [0, 0, 0, 255],
        "post-move: the old interior must return to the clear color"
    );
    assert_dominant_channel(
        pixel_at(&second, WIDTH, RIGHT_SAMPLE.0, RIGHT_SAMPLE.1),
        0,
        "post-move: the new interior should be entity red",
    );
}

/// An empty scene draws nothing and leaves the clear color everywhere.
///
/// **What this proves:** the degenerate end of the draw path — an empty bake
/// (`BakedFrame::is_empty`) still begins, ends, and submits the render pass
/// (so the target clears) without creating a zero-size vertex buffer or
/// issuing a zero-vertex draw, either of which would risk driver-defined
/// behavior. Guards the regression where "nothing to draw" skips the submit
/// and leaves stale contents (or uninitialized memory) in the target.
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn empty_scene_clears_to_clear_color() {
    // Given: a world with no renderable entities, run through the real
    // schedule (bake must still insert the resource, empty).
    let mut world = World::new();
    let mut schedule = Schedule::new();
    register_transform_propagation(&mut schedule);
    register_render_bake(&mut schedule);
    schedule.run(&mut world);
    assert!(
        world
            .resource::<BakedFrame>()
            .expect("bake must insert the BakedFrame resource even for an empty scene")
            .is_empty(),
        "an empty scene must bake to an empty frame"
    );

    // When: the empty frame is drawn.
    let device = real_device();
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let pipeline = bridge_pipeline(&device);
    let pixels = draw_and_readback(&device, &target, &pipeline, &world);

    // Then: every sampled pixel — corners, edges, center, and a coarse grid —
    // is exactly the clear color.
    for (x, y) in [
        (0, 0),
        (WIDTH - 1, 0),
        (0, HEIGHT - 1),
        (WIDTH - 1, HEIGHT - 1),
        (WIDTH / 2, HEIGHT / 2),
        LEFT_SAMPLE,
        RIGHT_SAMPLE,
        GAP_SAMPLE,
    ] {
        assert_eq!(
            pixel_at(&pixels, WIDTH, x, y),
            [0, 0, 0, 255],
            "empty scene: pixel ({x}, {y}) should be the clear color"
        );
    }
}

/// A file-loaded quad renders the same pixels as its hand-fed equivalent.
///
/// **What this proves (file-bytes-to-pixels for geometry):** the left quad's
/// vertices come from `quad.glb` on disk — parsed by `load_mesh`, owned by
/// the [`AssetStore`], referenced by handle, expanded index→soup at the
/// bridge — while the right quad is the hand-fed [`quad_vertices`] soup the
/// existing tests already prove. Both are the same size on screen (the mesh
/// entity's `0.5` scale compensates the fixture's half-size-`0.5` quad),
/// the same color, and mirrored positions — so their interiors must read
/// back identically: dominant red each, and exactly equal to each other
/// (flat color, symmetric coverage — any bridge-side corruption of the
/// file path, from index mis-expansion to a dropped triangle, breaks the
/// equality). The gap staying clear proves neither smeared. Zero RHI
/// churn: the mesh floats ride the same [`BakedFrame`], shader, layout,
/// and one-draw record as the soup.
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn file_loaded_mesh_matches_hand_fed_soup() {
    // Given: a mesh quad (red, left) and a soup quad (red, right) through
    // the full propagation → soup-bake → mesh-bake schedule.
    let mut world = World::new();
    let (store, handle) = mesh_store_with_quad();
    world.insert_resource(store);
    spawn_mesh_quad(&mut world, handle, -QUAD_OFFSET_X, [1.0, 0.0, 0.0]);
    spawn_quad(&mut world, QUAD_OFFSET_X, [1.0, 0.0, 0.0]);
    let mut schedule = full_render_schedule();
    schedule.run(&mut world);

    // When: the combined frame is drawn once to a fresh 128x128 target.
    let device = real_device();
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let pipeline = bridge_pipeline(&device);
    let pixels = draw_and_readback(&device, &target, &pipeline, &world);

    // Then: file path and hand-fed path render identically.
    let left = pixel_at(&pixels, WIDTH, LEFT_SAMPLE.0, LEFT_SAMPLE.1);
    assert_dominant_channel(left, 0, "file-loaded quad interior should be entity red");
    let right = pixel_at(&pixels, WIDTH, RIGHT_SAMPLE.0, RIGHT_SAMPLE.1);
    assert_dominant_channel(right, 0, "hand-fed quad interior should be entity red");
    assert_eq!(
        left, right,
        "mirrored same-size same-color quads must read back exactly equal, got {left:?} vs {right:?}"
    );
    for (x, y) in [(16, 64), (24, 64), (20, 60), (20, 68)] {
        assert_dominant_channel(
            pixel_at(&pixels, WIDTH, x, y),
            0,
            "file-loaded neighbor ({x}, {y}) should stay red",
        );
    }
    assert_eq!(
        pixel_at(&pixels, WIDTH, GAP_SAMPLE.0, GAP_SAMPLE.1),
        [0, 0, 0, 255],
        "the gap between the file-loaded and hand-fed quads should be the clear color"
    );
}

/// Moving a mesh entity's `Transform` moves its file-loaded pixels.
///
/// **What this proves:** the end-to-end chain for file geometry —
/// `Transform` → propagation → fresh `GlobalTransform` → mesh extract
/// (handle resolve + index→soup) → mesh bake (append) → draw. After the
/// move and a second `schedule.run()`, the old interior must return to
/// the exact clear color (the frame was replaced and the target
/// re-cleared, not painted over) and the mirrored pixel must show the
/// entity color (propagation actually ran before the mesh bake — had the
/// mesh bake read the stale pre-move global, the quad would still raster
/// left and *both* asserts would fail in opposite directions).
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn moved_mesh_redraws_moved() {
    // Given: one file-loaded red quad on the left, baked and drawn to a
    // shared target.
    let mut world = World::new();
    let (store, handle) = mesh_store_with_quad();
    world.insert_resource(store);
    let entity = spawn_mesh_quad(&mut world, handle, -QUAD_OFFSET_X, [1.0, 0.0, 0.0]);
    let mut schedule = full_render_schedule();
    schedule.run(&mut world);

    let device = real_device();
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let pipeline = bridge_pipeline(&device);
    let first = draw_and_readback(&device, &target, &pipeline, &world);
    assert_dominant_channel(
        pixel_at(&first, WIDTH, LEFT_SAMPLE.0, LEFT_SAMPLE.1),
        0,
        "pre-move: file-loaded left interior should be entity red",
    );

    // When: the Transform moves to the mirror position, the full schedule
    // re-runs, and the SAME target is redrawn (shared target, so stale
    // pixels would survive a missing clear).
    world
        .get_mut::<Transform>(entity)
        .expect("mesh entity still holds its Transform")
        .translation
        .x = QUAD_OFFSET_X;
    schedule.run(&mut world);
    let second = draw_and_readback(&device, &target, &pipeline, &world);

    // Then: the old interior is back to the clear color and the new
    // interior shows the entity color.
    assert_eq!(
        pixel_at(&second, WIDTH, LEFT_SAMPLE.0, LEFT_SAMPLE.1),
        [0, 0, 0, 255],
        "post-move: the old file-loaded interior must return to the clear color"
    );
    assert_dominant_channel(
        pixel_at(&second, WIDTH, RIGHT_SAMPLE.0, RIGHT_SAMPLE.1),
        0,
        "post-move: the new file-loaded interior should be entity red",
    );
}
