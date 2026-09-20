// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The v0.0.11 2D game-proof scene: a static ground, a falling dynamic
//! box, and a scripted kinematic platform, drawn as z-pinned flat quads
//! through the UNCHANGED soup bake (DECISION-3: fixed camera constants,
//! zero RHI churn).
//!
//! # What this proves
//!
//! The headless tests prove the simulation: the ground never moves, the
//! box falls under gravity and comes to rest ON the ground (collision,
//! no tunneling), and the kinematic platform follows its pose script
//! exactly. The `#[ignore]`-gated pixel tests close the loop through
//! rasterization: pre-physics vs post-physics frames differ (the negative
//! control — an unstepped pipeline draws the box at its spawn pose, so
//! these tests fail on it by construction), the ground pixels stay
//! stable, and the moved bodies' redraw re-proves the full chain
//! physics → propagation → extract → bake → draw.
//!
//! # Scene layout (world units, z-pinned plane)
//!
//! - Ground: `Fixed` + `Cuboid([2.0, 0.25])` at `(0, -1.2)`, green quad
//!   scaled `(8, 1, 1)` over the half-size-`0.25` soup quad so the pixels
//!   match the collider (`2.0 x 0.25` half-extents).
//! - Falling box: `Dynamic` + `Cuboid([0.25, 0.25])` at `(0, 1.0)`, red.
//!   The collider is load-bearing: mass comes from colliders, so a
//!   colliderless dynamic body would hang motionless and this test would
//!   fail (see the `PhysicsBackend` trait docs).
//! - Platform: `KinematicPosition` + `Cuboid([0.25, 0.25])` scripted from
//!   `x = 0.5` to `x = 1.0` at `y = 0.5`, blue. Scripted, not input
//!   driven: no input system exists yet (explicitly deferred), and
//!   `KinematicPosition` bodies are driven FROM their `Transform` each
//!   tick — so the "script" is the test writing `Transform` poses, the
//!   same seam future input code will write through.
//!
//! # Pixel geometry
//!
//! The bridge bakes with `CAMERA_DISTANCE 3.2`, `FOCAL_LENGTH 2.2`, so a
//! world unit at `z = 0` spans `2.2 / 3.2 * 64 = 44` pixels on the
//! 128-wide target: `px = 64 + v * 44` per axis (world `+y` reads toward
//! higher row indices through this backend's viewport mapping —
//! calibrated live against llvmpipe, not derived). A half-size-`0.25`
//! quad spans `±11px` around its center sample.
//!
//! # Why `#[ignore]` on the pixel tests only
//!
//! Same gating as `render_ecs_readback`: pixels need a real Vulkan ICD
//! (software llvmpipe is enough). The headless tests run everywhere with
//! the normal suite; the pixel tests run via
//! `cargo test -p canary-render-ecs -- --ignored`.

use canary_ecs::{Entity, World};
use canary_physics::{register_physics_step, Collider, FrameDelta, RigidBody};
use canary_render::{ColorTargetDescriptor, PipelineDescriptor, RenderDevice};
use canary_render_ecs::{
    draw_baked_frame, register_render_bake, render_vertex_attributes, render_vertex_stride,
    BakedFrame, Renderable, RENDER_WGSL,
};
use canary_render_vulkan::VulkanDevice;
use canary_scheduler::Schedule;
use canary_transform::{register_transform_propagation, GlobalTransform, Transform};

/// Shared offscreen target: square, matching the bridge's square-aspect bake.
const WIDTH: u32 = 128;
/// Shared offscreen target height (square target).
const HEIGHT: u32 = 128;

/// Pixels per world unit at `z = 0`: `2.2 / 3.2 * 64`.
const PX_PER_UNIT: f32 = 44.0;

/// Object-space half-size of the soup quads (the `render_ecs_readback` value).
const QUAD_HALF_SIZE: f32 = 0.25;

/// Ground center pose.
const GROUND_POS: [f32; 2] = [0.0, -1.2];
/// Ground collider half-extents (also the quad's world half-size).
const GROUND_HALF: [f32; 2] = [2.0, 0.25];
/// Falling-box spawn pose.
const BOX_START: [f32; 2] = [0.0, 1.0];
/// Box/platform collider half-extents (match the unscaled soup quad).
const BODY_HALF: [f32; 2] = [0.25, 0.25];
/// Scripted platform height (clear of the box's fall line at `x = 0`).
const PLATFORM_Y: f32 = 0.5;
/// Platform script endpoints: the lane `0.65 → 1.1` keeps a clear gap
/// from the box's fall line (box right edge `0.25`, platform left edge
/// `0.40` at start) so the falling box never grazes the platform, while
/// both script poses stay fully on the 128-wide target.
const PLATFORM_START_X: f32 = 0.65;
/// Platform script endpoints.
const PLATFORM_END_X: f32 = 1.1;

/// Full 2 s run at `FIXED_DT`: the box lands (~0.6 s of free fall for
/// 1.7 m) and settles long before the run ends.
const RUN_TICKS: usize = 120;
/// Ticks at the start pose before the platform teleports to its end pose.
const SCRIPT_TICKS: usize = 60;

/// Maps a world `(x, y)` at `z = 0` to a target pixel.
fn world_to_pixel(x: f32, y: f32) -> (u32, u32) {
    let px = (64.0 + x * PX_PER_UNIT).round() as u32;
    let py = (64.0 + y * PX_PER_UNIT).round() as u32;
    (px, py)
}

/// Two triangles forming a `2s`-sided quad in the `z = 0` plane (the
/// `render_ecs_readback` shape).
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

/// Spawns one physics body rendered as a soup quad: physics components
/// plus a deliberately stale identity [`GlobalTransform`] so the
/// schedule's propagation pass is what makes the global fresh, plus a
/// [`Renderable`] the unchanged soup bake picks up.
fn spawn_game_body(
    world: &mut World,
    body: RigidBody,
    collider: Collider,
    x: f32,
    y: f32,
    scale: [f32; 3],
    color: [f32; 3],
) -> Entity {
    let entity = world.spawn();
    world
        .insert(entity, body)
        .expect("fresh entity accepts RigidBody");
    world
        .insert(entity, collider)
        .expect("fresh entity accepts Collider");
    let mut transform = Transform::from_translation(glam::Vec3::new(x, y, 0.0));
    transform.scale = glam::Vec3::from_array(scale);
    world
        .insert(entity, transform)
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

/// The three game-proof bodies. Colors are distinct per body: green
/// ground, red box, blue platform.
struct GameScene {
    /// Static ground entity.
    ground: Entity,
    /// Falling dynamic box entity.
    falling: Entity,
    /// Scripted kinematic platform entity.
    platform: Entity,
}

/// Builds the game-proof scene: ground + falling box + platform at its
/// script start pose.
fn spawn_game_scene(world: &mut World) -> GameScene {
    let ground = spawn_game_body(
        world,
        RigidBody::fixed(),
        Collider::cuboid(GROUND_HALF),
        GROUND_POS[0],
        GROUND_POS[1],
        [8.0, 1.0, 1.0],
        [0.0, 1.0, 0.0],
    );
    let falling = spawn_game_body(
        world,
        RigidBody::dynamic(),
        Collider::cuboid(BODY_HALF),
        BOX_START[0],
        BOX_START[1],
        [1.0, 1.0, 1.0],
        [1.0, 0.0, 0.0],
    );
    let platform = spawn_game_body(
        world,
        RigidBody::kinematic_position(),
        Collider::cuboid(BODY_HALF),
        PLATFORM_START_X,
        PLATFORM_Y,
        [1.0, 1.0, 1.0],
        [0.0, 0.0, 1.0],
    );
    GameScene {
        ground,
        falling,
        platform,
    }
}

/// The canonical per-tick chain: physics step FIRST (it writes
/// `Transform`), then propagation (reads it), then the soup bake.
/// Registration order is the ordering mechanism — the same order
/// `EcsSubsystem` uses.
fn game_schedule() -> Schedule {
    let mut schedule = Schedule::new();
    register_physics_step(&mut schedule);
    register_transform_propagation(&mut schedule);
    register_render_bake(&mut schedule);
    schedule
}

/// Runs one tick with exactly one fixed step: mirrors
/// `EcsSubsystem::tick` (insert the frame delta, run the schedule).
fn tick_once(world: &mut World, schedule: &mut Schedule) {
    world.insert_resource(FrameDelta::default());
    schedule.run(world);
}

/// Runs the full scripted game: `SCRIPT_TICKS` ticks with the platform
/// at its start pose, teleport the platform to its end pose (the
/// script — no input system exists yet), then run to `RUN_TICKS`.
fn run_game(world: &mut World, schedule: &mut Schedule, scene: &GameScene) {
    for _ in 0..SCRIPT_TICKS {
        tick_once(world, schedule);
    }
    world
        .get_mut::<Transform>(scene.platform)
        .expect("platform entity still holds its Transform")
        .translation
        .x = PLATFORM_END_X;
    for _ in SCRIPT_TICKS..RUN_TICKS {
        tick_once(world, schedule);
    }
}

/// Current translation of an entity's `Transform`.
fn translation_of(world: &World, entity: Entity) -> glam::Vec3 {
    world
        .get::<Transform>(entity)
        .expect("entity must still carry Transform")
        .translation
}

// --- Headless game-proof tests (run everywhere, no ICD needed) ---

/// The static ground never moves, no matter how long the run.
#[test]
fn ground_stays_put_for_the_whole_run() {
    // Given: the game-proof scene through the canonical schedule.
    let mut world = World::new();
    let scene = spawn_game_scene(&mut world);
    let mut schedule = game_schedule();

    // When: the full scripted run.
    run_game(&mut world, &mut schedule, &scene);

    // Then: the ground pose is bit-identical to its spawn pose.
    let at = translation_of(&world, scene.ground);
    assert_eq!(
        [at.x, at.y, at.z],
        [GROUND_POS[0], GROUND_POS[1], 0.0],
        "a Fixed body must never move: {at:?}"
    );
}

/// The dynamic box falls under gravity and comes to rest ON the
/// ground: below its spawn, at the contact height, never tunneled
/// through.
#[test]
fn falling_box_lands_and_rests_on_the_ground() {
    // Given: the game-proof scene through the canonical schedule.
    let mut world = World::new();
    let scene = spawn_game_scene(&mut world);
    let mut schedule = game_schedule();

    // When: the full scripted run (2 simulated seconds).
    run_game(&mut world, &mut schedule, &scene);

    // Then: the box fell (below spawn), rests at the contact height
    // (ground top `-0.95` plus box half-height `0.25` = `-0.70`, with
    // solver slop), and never tunneled past the ground slab.
    let at = translation_of(&world, scene.falling);
    assert!(
        at.y < BOX_START[1] - 1.0,
        "two seconds of gravity must drop the box well below spawn: {at:?}"
    );
    assert!(
        (-0.80..=-0.60).contains(&at.y),
        "the box must rest ON the ground (contact at y = -0.70): {at:?}"
    );
    assert!(
        at.x.abs() < 0.05,
        "a centered drop onto flat ground must not wander sideways: {at:?}"
    );
}

/// The scripted kinematic platform follows its pose script: start pose
/// for the first half, end pose after the teleport.
#[test]
fn scripted_platform_follows_its_pose_script() {
    // Given: the game-proof scene through the canonical schedule.
    let mut world = World::new();
    let scene = spawn_game_scene(&mut world);
    let mut schedule = game_schedule();

    // When: half the run (platform still at its script start pose).
    for _ in 0..SCRIPT_TICKS {
        tick_once(&mut world, &mut schedule);
    }
    let mid = translation_of(&world, scene.platform);
    assert!(
        (mid.x - PLATFORM_START_X).abs() < 1e-3,
        "the kinematic platform must hold its scripted start pose: {mid:?}"
    );

    // When: the teleport plus the rest of the run.
    world
        .get_mut::<Transform>(scene.platform)
        .expect("platform entity still holds its Transform")
        .translation
        .x = PLATFORM_END_X;
    for _ in SCRIPT_TICKS..RUN_TICKS {
        tick_once(&mut world, &mut schedule);
    }

    // Then: the platform sits at its scripted end pose, height untouched.
    let at = translation_of(&world, scene.platform);
    assert!(
        (at.x - PLATFORM_END_X).abs() < 1e-3,
        "the kinematic platform must follow the script to its end pose: {at:?}"
    );
    assert!(
        (at.y - PLATFORM_Y).abs() < 1e-3,
        "the script never touches height: {at:?}"
    );
}

/// Negative control, headless half: with zero frame time the pipeline
/// steps nothing and no body moves — the pose the pixels would draw is
/// the spawn pose, which is exactly what the pixel negative control
/// below asserts against.
#[test]
fn zero_frame_time_steps_nothing_and_moves_nothing() {
    // Given: the game-proof scene through the canonical schedule.
    let mut world = World::new();
    let scene = spawn_game_scene(&mut world);
    let mut schedule = game_schedule();

    // When: a full run's worth of ticks with zero frame time each.
    for _ in 0..RUN_TICKS {
        world.insert_resource(FrameDelta::new(std::time::Duration::ZERO));
        schedule.run(&mut world);
    }

    // Then: every body is still exactly at its spawn pose.
    let falling = translation_of(&world, scene.falling);
    assert_eq!(
        [falling.x, falling.y],
        BOX_START,
        "zero steps must leave the box at spawn: {falling:?}"
    );
    let platform = translation_of(&world, scene.platform);
    assert_eq!(
        [platform.x, platform.y],
        [PLATFORM_START_X, PLATFORM_Y],
        "zero steps must leave the platform at its script start: {platform:?}"
    );
}

// --- Pixel proof (needs a real Vulkan ICD; #[ignore]-gated) ---

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
fn real_device() -> VulkanDevice {
    VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (CI needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    })
}

/// Builds the soup bridge pipeline against `device`: stride and
/// attributes come from the bridge's own layout functions, so a layout
/// regression fails as wrong pixels rather than diverging silently.
fn bridge_pipeline(device: &VulkanDevice) -> <VulkanDevice as RenderDevice>::Pipeline {
    let compile_stage = |stage: naga::ShaderStage| {
        let module = naga::front::wgsl::parse_str(RENDER_WGSL).expect("failed to parse WGSL");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        let info = validator.validate(&module).expect("WGSL failed validation");
        let entry_point = match stage {
            naga::ShaderStage::Vertex => "vs_main",
            naga::ShaderStage::Fragment => "fs_main",
            // Wildcard, not an exhaustive variant list: naga grows `ShaderStage`
            // over majors (mesh/task, ray-tracing stages); this harness only
            // ever compiles vertex + fragment entry points.
            _ => unreachable!("only vertex/fragment stages in this test's shader"),
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
    };
    let vertex_spirv = compile_stage(naga::ShaderStage::Vertex);
    let fragment_spirv = compile_stage(naga::ShaderStage::Fragment);
    let vertex_attributes = render_vertex_attributes();
    device.create_pipeline(&PipelineDescriptor {
        label: "physics game-proof readback pipeline",
        vertex_shader_spirv: &vertex_spirv,
        vertex_entry_point: "vs_main",
        fragment_shader_spirv: &fragment_spirv,
        fragment_entry_point: "fs_main",
        vertex_stride: render_vertex_stride(),
        vertex_attributes: &vertex_attributes,
    })
}

/// Draws the world's current [`BakedFrame`] to `target` and returns the
/// tightly-packed RGBA8 readback.
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

/// Asserts `pixel` is dominantly `channel` (value `> 150`) with the
/// other two channels quiet (`< 80`) — the `hello_triangle`
/// dominant-channel style.
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

/// Pre-physics vs post-physics frames differ: the falling box visibly
/// moves from its spawn pose to its resting pose.
///
/// **Negative control by construction:** the pre frame is drawn after
/// one schedule tick with zero physics progress would still show the
/// spawn pose — an unstepped pipeline draws exactly the pre frame
/// forever, so if stepping did nothing these two frames would be
/// byte-identical and every assert below would fail.
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn pre_vs_post_physics_frames_differ() {
    // Given: the game-proof scene, baked once before any stepping.
    let mut world = World::new();
    let scene = spawn_game_scene(&mut world);
    let mut schedule = game_schedule();
    tick_once(&mut world, &mut schedule);

    let device = real_device();
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let pipeline = bridge_pipeline(&device);
    let pre = draw_and_readback(&device, &target, &pipeline, &world);

    // The box starts at its spawn pixel (red), its resting pixel is clear.
    let box_start_px = world_to_pixel(BOX_START[0], BOX_START[1]);
    let box_rest_px = world_to_pixel(0.0, -0.70);
    assert_dominant_channel(
        pixel_at(&pre, WIDTH, box_start_px.0, box_start_px.1),
        0,
        "pre-physics: the box spawn pixel should be entity red",
    );
    assert_eq!(
        pixel_at(&pre, WIDTH, box_rest_px.0, box_rest_px.1),
        [0, 0, 0, 255],
        "pre-physics: the resting pixel should still be the clear color"
    );

    // When: the full scripted run, then a redraw of the SAME target
    // (shared target, so stale pixels would survive a missing clear).
    run_game(&mut world, &mut schedule, &scene);
    let post = draw_and_readback(&device, &target, &pipeline, &world);

    // Then: the frames differ, the spawn pixel cleared, the resting
    // pixel shows the box.
    assert_ne!(
        pre, post,
        "stepping the simulation must change the drawn frame"
    );
    assert_eq!(
        pixel_at(&post, WIDTH, box_start_px.0, box_start_px.1),
        [0, 0, 0, 255],
        "post-physics: the spawn pixel must return to the clear color"
    );
    assert_dominant_channel(
        pixel_at(&post, WIDTH, box_rest_px.0, box_rest_px.1),
        0,
        "post-physics: the resting pixel should be entity red",
    );
}

/// The ground pixels stay green across the whole run: the static body
/// never moves and the bake never smears it.
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn ground_pixels_stable_across_the_run() {
    // Given: the game-proof scene, baked once before any stepping.
    let mut world = World::new();
    let scene = spawn_game_scene(&mut world);
    let mut schedule = game_schedule();
    tick_once(&mut world, &mut schedule);

    let device = real_device();
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let pipeline = bridge_pipeline(&device);
    let pre = draw_and_readback(&device, &target, &pipeline, &world);

    // The ground center row: world y = -1.2 lands at row 11, spanning
    // the full width — sample center plus both edges' interior.
    let ground_samples = [
        world_to_pixel(-1.5, GROUND_POS[1]),
        world_to_pixel(0.0, GROUND_POS[1]),
        world_to_pixel(1.5, GROUND_POS[1]),
    ];
    for (x, y) in ground_samples {
        assert_dominant_channel(
            pixel_at(&pre, WIDTH, x, y),
            1,
            "pre-physics: ground pixel ({x}, {y}) should be entity green",
        );
    }

    // When: the full scripted run (the box lands ON this ground) plus redraw.
    run_game(&mut world, &mut schedule, &scene);
    let post = draw_and_readback(&device, &target, &pipeline, &world);

    // Then: every ground sample is still exactly as green.
    for (x, y) in ground_samples {
        assert_dominant_channel(
            pixel_at(&post, WIDTH, x, y),
            1,
            "post-physics: ground pixel ({x}, {y}) should still be entity green",
        );
        assert_eq!(
            pixel_at(&pre, WIDTH, x, y),
            pixel_at(&post, WIDTH, x, y),
            "ground pixel ({x}, {y}) must be pixel-stable across the run"
        );
    }
}

/// The scripted platform's blue pixels move with the script: start
/// pose blue before, end pose blue after, each clearing the other.
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn scripted_platform_redraws_at_its_scripted_pose() {
    // Given: the game-proof scene, baked once before any stepping.
    let mut world = World::new();
    let scene = spawn_game_scene(&mut world);
    let mut schedule = game_schedule();
    tick_once(&mut world, &mut schedule);

    let device = real_device();
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let pipeline = bridge_pipeline(&device);
    let pre = draw_and_readback(&device, &target, &pipeline, &world);

    let script_start_px = world_to_pixel(PLATFORM_START_X, PLATFORM_Y);
    let script_end_px = world_to_pixel(PLATFORM_END_X, PLATFORM_Y);
    assert_dominant_channel(
        pixel_at(&pre, WIDTH, script_start_px.0, script_start_px.1),
        2,
        "pre-physics: the platform script-start pixel should be entity blue",
    );
    assert_eq!(
        pixel_at(&pre, WIDTH, script_end_px.0, script_end_px.1),
        [0, 0, 0, 255],
        "pre-physics: the script-end pixel should still be the clear color"
    );

    // When: the full scripted run (platform teleports mid-run) plus redraw.
    run_game(&mut world, &mut schedule, &scene);
    let post = draw_and_readback(&device, &target, &pipeline, &world);

    // Then: the script start cleared and the script end shows the platform.
    assert_eq!(
        pixel_at(&post, WIDTH, script_start_px.0, script_start_px.1),
        [0, 0, 0, 255],
        "post-physics: the script-start pixel must return to the clear color"
    );
    assert_dominant_channel(
        pixel_at(&post, WIDTH, script_end_px.0, script_end_px.1),
        2,
        "post-physics: the script-end pixel should be entity blue",
    );
}
