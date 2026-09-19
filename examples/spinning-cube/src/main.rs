// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! A rotating cube, rendered offscreen through the real RHI (`canary-render`
//! + `canary-render-vulkan`) and encoded to an animated GIF — driven, since
//!   `v0.0.9`, by the ECS-to-render bridge (`canary-render-ecs`) instead of
//!   hand-rolled per-frame math in this file.
//!
//! # Per-frame flow, step by step
//!
//! Every one of the 36 frames goes through exactly these steps, in this
//! order:
//!
//! 1. **Animate.** The cube root entity's [`Transform`] rotation is
//!    overwritten with the frame's spin: a fixed −30° tilt about X composed
//!    with the animated yaw about Y, as a single [`glam::Quat`]. Only the
//!    root is touched; the six face entities inherit the motion through the
//!    hierarchy.
//! 2. **Run the schedule.** `schedule.run(&mut world)` executes the two
//!    registered write systems in registration order, each alone in its own
//!    stage: transform propagation first (recomputing every face's
//!    [`GlobalTransform`] from the freshly animated root), then the
//!    aspect-aware bake ([`extract_scene`] →
//!    [`bake_scene_to_vertices_with_aspect`] → overwrite the [`BakedFrame`]
//!    resource). This mirrors the `EcsSubsystem` tick pattern from Task 4 of
//!    the `v0.0.9` rendering stage: propagation before bake, always.
//! 3. **Draw.** [`draw_baked_frame`] uploads the [`BakedFrame`] floats as one
//!    fresh vertex buffer and issues one render pass + one draw through the
//!    real Vulkan device. Fully safe: the `f32`→bytes conversion is
//!    `flat_map(to_ne_bytes)` inside the bridge — the old
//!    `unsafe from_raw_parts` reinterpretation this example used to do is
//!    gone.
//! 4. **Read back and encode.** `read_color_target_rgba8` returns the frame's
//!    pixels, which are appended to the animated GIF (`gif` crate, 480×360,
//!    40 ms per frame, infinite repeat).
//!
//! # What moved into the bridge vs what stays example-specific
//!
//! The bridge (`canary-render-ecs`) now owns everything that used to be
//! hand-rolled here: [`extract_scene`] (the `World` query), the perspective
//! projection with its Y-flip negate, the back-to-front painter sort, the
//! proven passthrough WGSL shader ([`RENDER_WGSL`], byte-identical to
//! `hello_triangle`'s), the vertex attribute layout and stride, and the
//! one-buffer-one-draw upload in [`draw_baked_frame`]. None of that is
//! duplicated here — `build_frame_vertices`/`project`/the hand sort are
//! deleted, and grepping this file for `rotate_`/`project`/`sort_by` must
//! find nothing but prose.
//!
//! What stays here is the part no engine crate could own: the cube itself
//! (the [`CUBE_CORNERS`]/[`FACES`] constants and their flattening into
//! [`Renderable`] triangle soup), the per-frame spin animation, the
//! device/target/pipeline setup, and the GIF encoding.
//!
//! # Why one root plus six face entities
//!
//! [`Renderable`] carries exactly one flat RGB color per entity — the RHI has
//! no uniforms, materials, or textures to vary color any other way. A single
//! cube entity therefore cannot keep the six flat per-face colors, so the
//! cube is a root entity (holding the animated [`Transform`]) with six face
//! entities parented to it, each holding its face's two triangles plus that
//! face's color. This is also what makes the example exercise hierarchy
//! propagation for real: the animation writes one [`Transform`], and the
//! scheduled propagation is what moves all six faces.
//!
//! # Retained limits (unchanged from the pre-bridge version)
//!
//! - **Convex-only painter sort.** The bridge sorts triangles back-to-front
//!   by average camera-space depth because the RHI has no depth buffer and
//!   no backface culling. Exactly correct for a convex cube viewed from
//!   outside; not sufficient for concave geometry, which needs a real depth
//!   buffer (deferred RHI work, not something this example works around).
//! - **Fixed camera.** There is no camera component yet: the camera distance
//!   and focal length live as constants inside the bridge's bake, reused
//!   from this example's proven values.
//! - **Write-once buffers.** The RHI still has no story for updating a
//!   buffer after creation, so each frame uploads a fresh buffer — now
//!   inside [`draw_baked_frame`], not inline here.
//! - **Clear color.** The clear color is now the bridge's
//!   [`DEFAULT_CLEAR_COLOR`] (opaque black), which [`draw_baked_frame`]
//!   owns along with the pass — the old dark-slate `CLEAR_COLOR` is retired
//!   rather than threaded through a draw call that takes no clear
//!   parameter.
//!
//! [`Transform`]: canary_transform::Transform
//! [`GlobalTransform`]: canary_transform::GlobalTransform
//! [`extract_scene`]: canary_render_ecs::extract_scene
//! [`bake_scene_to_vertices_with_aspect`]:
//!     canary_render_ecs::bake_scene_to_vertices_with_aspect
//! [`BakedFrame`]: canary_render_ecs::BakedFrame
//! [`draw_baked_frame`]: canary_render_ecs::draw_baked_frame
//! [`RENDER_WGSL`]: canary_render_ecs::RENDER_WGSL
//! [`Renderable`]: canary_render_ecs::Renderable
//! [`DEFAULT_CLEAR_COLOR`]: canary_render_ecs::DEFAULT_CLEAR_COLOR

use canary_ecs::{Entity, World};
use canary_render::{ColorTargetDescriptor, PipelineDescriptor, RenderDevice};
use canary_render_ecs::{
    bake_access, bake_scene_to_vertices_with_aspect, draw_baked_frame, extract_scene,
    render_vertex_attributes, render_vertex_stride, BakedFrame, Renderable, RENDER_WGSL,
};
use canary_render_vulkan::VulkanDevice;
use canary_scheduler::Schedule;
use canary_transform::{register_transform_propagation, set_parent, GlobalTransform, Transform};
use glam::{Quat, Vec3};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 360;
const FRAME_COUNT: u32 = 36;

// A tilt alone only ever reveals one full face plus a hairline sliver of
// whichever face the tilt leans toward -- confirmed empirically (rendered
// and viewed) before landing on this: with zero yaw the left/right faces
// are exactly edge-on to the camera, which looks like a flat trapezoid,
// not a cube. This offset is where frame 0's rotation starts from, so the
// very first frame (what a static preview shows) is already a clear
// three-faces-visible view rather than that degenerate angle -- the
// animation still spins through every angle, including edge-on ones, same
// as any real rotating object would.
const INITIAL_YAW_RADIANS: f32 = 0.6;

/// Fixed tilt about the X axis, composed with the animated yaw every frame.
///
/// −30°: tips the cube forward so the top face stays visible throughout the
/// spin. Previously buried inside `build_frame_vertices`; now a named
/// constant because the per-frame rotation is built in `main`, far from any
/// projection code.
const TILT_RADIANS: f32 = -std::f32::consts::FRAC_PI_6;

/// One cube corner in object space.
type Vertex3 = [f32; 3];

/// The 8 corners of a unit cube, centered on the origin.
const CUBE_CORNERS: [Vertex3; 8] = [
    [-0.5, -0.5, -0.5], // 0
    [0.5, -0.5, -0.5],  // 1
    [0.5, 0.5, -0.5],   // 2
    [-0.5, 0.5, -0.5],  // 3
    [-0.5, -0.5, 0.5],  // 4
    [0.5, -0.5, 0.5],   // 5
    [0.5, 0.5, 0.5],    // 6
    [-0.5, 0.5, 0.5],   // 7
];

/// One face: two triangles (as corner indices into [`CUBE_CORNERS`]),
/// six corners total, plus that face's flat color. Winding order
/// doesn't matter here -- `canary-render-vulkan`'s pipeline hard-codes
/// no backface culling (see this file's module docs) -- only that each
/// pair of triangles actually covers the face.
struct Face {
    corners: [usize; 6],
    color: [f32; 3],
}

const FACES: [Face; 6] = [
    Face {
        corners: [0, 1, 2, 0, 2, 3],
        color: [0.90, 0.85, 0.20],
    }, // -Z: yellow
    Face {
        corners: [4, 6, 5, 4, 7, 6],
        color: [0.20, 0.45, 0.90],
    }, // +Z: blue
    Face {
        corners: [0, 3, 7, 0, 7, 4],
        color: [0.20, 0.85, 0.85],
    }, // -X: cyan
    Face {
        corners: [1, 5, 6, 1, 6, 2],
        color: [0.90, 0.25, 0.25],
    }, // +X: red
    Face {
        corners: [0, 4, 5, 0, 5, 1],
        color: [0.85, 0.25, 0.85],
    }, // -Y: magenta
    Face {
        corners: [3, 2, 6, 3, 6, 7],
        color: [0.30, 0.85, 0.30],
    }, // +Y: green
];

/// Flattens one [`Face`] into the [`Renderable`] its face entity carries:
/// the face's six corners resolved to object-space positions, plus the
/// face's flat color.
///
/// One [`Renderable`] per face (not one for the whole cube) because a
/// [`Renderable`] holds a single flat color per entity — the only way the
/// six per-face colors survive the bridge's per-entity-color model.
fn cube_face_renderable(face: &Face) -> Renderable {
    let vertices = face.corners.map(|corner| CUBE_CORNERS[corner]).to_vec();
    Renderable::new(vertices, face.color)
}

/// Bakes the current scene into the [`BakedFrame`] resource for this
/// example's 480×360 target: [`extract_scene`] →
/// [`bake_scene_to_vertices_with_aspect`] → overwrite the resource.
///
/// Registered with the bridge's [`bake_access`] declaration (reads
/// `GlobalTransform` + `Renderable`, writes-resource `BakedFrame`), so the
/// scheduler stages it exactly like the bridge's own
/// `register_render_bake` system — alone in a later stage, strictly after
/// propagation. The only difference from the bridge's bake system is the
/// aspect ratio: the bridge's bakes for a square target, while this
/// target is 4:3, and baking with the wrong aspect stretches the image
/// (see the bridge's own docs on `bake_scene_to_vertices_with_aspect`).
/// No projection or sort logic lives here — that all stays in the bridge.
fn bake_frame_wide(world: &mut World) {
    let aspect_ratio = WIDTH as f32 / HEIGHT as f32;
    let items = extract_scene(world);
    let vertices = bake_scene_to_vertices_with_aspect(&items, aspect_ratio);
    world.insert_resource(BakedFrame { vertices });
}

/// Spawns the cube: a root entity holding the animated [`Transform`], plus
/// one face entity per [`Face`] (identity local transform, that face's
/// [`Renderable`]) parented to the root.
///
/// Returns the root entity; `main` animates its [`Transform`] every frame
/// and propagation carries the motion to the faces.
fn spawn_cube(world: &mut World) -> Entity {
    let cube = world.spawn();
    world
        .insert(cube, Transform::identity())
        .expect("freshly spawned cube root accepts a Transform");
    world
        .insert(cube, GlobalTransform::default())
        .expect("freshly spawned cube root accepts a GlobalTransform");
    for face in &FACES {
        let face_entity = world.spawn();
        world
            .insert(face_entity, Transform::identity())
            .expect("freshly spawned face accepts a Transform");
        world
            .insert(face_entity, cube_face_renderable(face))
            .expect("freshly spawned face accepts a Renderable");
        set_parent(world, face_entity, Some(cube))
            .expect("freshly spawned face and cube root are both alive");
    }
    cube
}

/// Compiles the bridge's [`RENDER_WGSL`] to SPIR-V for one shader stage --
/// the same real `naga` calls `canary-render-vulkan`'s own
/// `hello_triangle` test already validates against this workspace's
/// toolchain.
///
/// The shader source itself lives in the bridge now (this example no
/// longer keeps its own copy): reusing the proven shader rather than
/// writing and trusting a new one, for a shape only more complex in how
/// many triangles reach it, not in what the GPU is asked to do with each
/// one.
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
        naga::ShaderStage::Compute => unreachable!("no compute stage in this example's shader"),
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

fn main() {
    let device = VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (this sandbox needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    });

    let color_target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });

    let vertex_spirv = compile_stage(naga::ShaderStage::Vertex);
    let fragment_spirv = compile_stage(naga::ShaderStage::Fragment);
    let vertex_attributes = render_vertex_attributes();
    let pipeline = device.create_pipeline(&PipelineDescriptor {
        label: "spinning-cube pipeline",
        vertex_shader_spirv: &vertex_spirv,
        vertex_entry_point: "vs_main",
        fragment_shader_spirv: &fragment_spirv,
        fragment_entry_point: "fs_main",
        vertex_stride: render_vertex_stride(),
        vertex_attributes: &vertex_attributes,
    });

    // The ECS half of the example: one animated cube (root + six faces),
    // and a schedule that propagates transforms before baking — the same
    // propagation-then-bake order the runtime's `EcsSubsystem` tick uses.
    let mut world = World::new();
    let cube = spawn_cube(&mut world);
    let mut schedule = Schedule::new();
    register_transform_propagation(&mut schedule);
    schedule.add_write_system(bake_access(), bake_frame_wide);

    let output_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "spinning_cube.gif".to_string());
    let mut output_file = std::fs::File::create(&output_path)
        .unwrap_or_else(|e| panic!("failed to create {output_path}: {e}"));
    let mut gif_encoder = gif::Encoder::new(&mut output_file, WIDTH as u16, HEIGHT as u16, &[])
        .expect("failed to start GIF encoder");
    gif_encoder
        .set_repeat(gif::Repeat::Infinite)
        .expect("failed to set GIF repeat mode");

    for frame_index in 0..FRAME_COUNT {
        let spin_radians =
            INITIAL_YAW_RADIANS + (frame_index as f32 / FRAME_COUNT as f32) * std::f32::consts::TAU;
        // Tilt composed outside the spin: `qx * qy` applies the yaw first,
        // then the tilt — the same order the old hand-rolled
        // `rotate_x(rotate_y(..))` evaluated in, now as one quaternion.
        let animated = world
            .get_mut::<Transform>(cube)
            .expect("cube root entity is alive for all 36 frames");
        animated.rotation =
            Quat::from_rotation_x(TILT_RADIANS) * Quat::from_rotation_y(spin_radians);
        // Keep the cube centered at the world origin: the bridge's bake
        // shifts everything into camera space itself.
        animated.translation = Vec3::ZERO;

        schedule.run(&mut world);

        let frame = world
            .resource::<BakedFrame>()
            .expect("the bake system inserts the BakedFrame resource every tick");
        draw_baked_frame(&device, &color_target, &pipeline, frame);

        let mut rgba = device.read_color_target_rgba8(&color_target);
        let mut frame = gif::Frame::from_rgba_speed(WIDTH as u16, HEIGHT as u16, &mut rgba, 10);
        frame.delay = 4; // 40ms/frame: a full 360-degree spin in ~1.44s
        gif_encoder
            .write_frame(&frame)
            .expect("failed to write GIF frame");

        println!("rendered frame {}/{FRAME_COUNT}", frame_index + 1);
    }

    println!("wrote {output_path}");
}
