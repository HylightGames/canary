// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! A rotating cube, rendered offscreen through the real `v0.0.6` RHI
//! (`canary-render` + `canary-render-vulkan`), encoded to an animated
//! GIF. This is the example `examples/README.md` named, back in
//! `v0.0.1`, as the first thing that would belong here once rendering
//! existed -- see that file's own history.
//!
//! **Why this looks the way it does, given what the RHI actually
//! offers as of `v0.0.6`/`v0.0.7`/`v0.0.8`:** no uniform buffers or push
//! constants (so the GPU can't be handed a rotation angle to transform
//! vertices itself), no depth buffer, and no backface culling
//! (`canary-render-vulkan`'s pipeline hard-codes `CullMode::NONE` --
//! see that crate's `pipeline.rs`). None of that is a bug to work
//! around; `canary_render::RenderDevice`'s own docs are explicit that
//! it's "deliberately minimal for `v0.0.6`... not a speculatively
//! complete GPU abstraction." So this example does the honest thing
//! given that scope, entirely on the CPU, once per frame:
//!
//! 1. Rotate the cube's 8 object-space vertices (a fixed tilt for a
//!    pleasant 3-faces-visible angle, plus an animated spin).
//! 2. Project each vertex to normalized device coordinates with a
//!    simple perspective divide, by hand -- no matrix library, since
//!    the whole transform is two rotations and one division, and
//!    getting *that* right by hand is far less risky than it would be
//!    to hand-roll a GIF encoder, which is why the encoding half of
//!    this example reaches for a real dependency (`gif`) instead.
//! 3. Sort the 12 triangles back-to-front by camera-space depth (a
//!    literal painter's algorithm) before uploading them, since nothing
//!    downstream will discard occluded fragments for us. This is
//!    exactly correct for a convex, non-self-intersecting shape viewed
//!    from outside itself -- a cube qualifies -- and would not be
//!    sufficient for arbitrary/concave geometry, which is precisely why
//!    a real depth buffer is real, expected future RHI work rather than
//!    something this example tries to generalize its way around.
//! 4. Feed the sorted, already-NDC-space triangles to the *exact* WGSL
//!    shader `canary-render-vulkan`'s own `hello_triangle` integration
//!    test already validates (a trivial 2D-position + RGB-color
//!    passthrough) -- reusing a shader this project has already proven
//!    correct, rather than writing and trusting a new one, for a shape
//!    only more complex in how many triangles reach it, not in what the
//!    GPU is asked to do with each one.

use canary_render::{
    BufferDescriptor, ColorTargetDescriptor, CommandEncoder, PipelineDescriptor, RenderDevice,
    RenderPassDescriptor, VertexAttribute, VertexFormat,
};
use canary_render_vulkan::VulkanDevice;

/// The same trivial passthrough shader `canary-render-vulkan`'s own
/// `hello_triangle` test uses: `input.position` is already in clip
/// space by the time it reaches the GPU, because every real transform
/// in this example happens on the CPU beforehand (see this file's
/// module docs for why).
const WGSL_SOURCE: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec3<f32>,
}
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
}
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(input.position, 0.0, 1.0);
    out.color = input.color;
    return out;
}
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
"#;

const WIDTH: u32 = 480;
const HEIGHT: u32 = 360;
const FRAME_COUNT: u32 = 36;
const CLEAR_COLOR: [f32; 4] = [0.08, 0.08, 0.12, 1.0]; // dark slate, not pure black

// A tilt alone (see build_frame_vertices) only ever reveals one full
// face plus a hairline sliver of whichever face the tilt leans toward
// -- confirmed empirically (rendered and viewed) before landing on
// this: with zero yaw the left/right faces are exactly edge-on to the
// camera, which looks like a flat trapezoid, not a cube. This offset
// is where frame 0's rotation starts from, so the very first frame
// (what a static preview shows) is already a clear three-faces-visible
// view rather than that degenerate angle -- the animation still spins
// through every angle, including edge-on ones, same as any real
// rotating object would.
const INITIAL_YAW_RADIANS: f32 = 0.6;

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

/// Rotates `v` around the Y axis by `radians`.
fn rotate_y(v: Vertex3, radians: f32) -> Vertex3 {
    let (sin, cos) = radians.sin_cos();
    [v[0] * cos + v[2] * sin, v[1], -v[0] * sin + v[2] * cos]
}

/// Rotates `v` around the X axis by `radians`.
fn rotate_x(v: Vertex3, radians: f32) -> Vertex3 {
    let (sin, cos) = radians.sin_cos();
    [v[0], v[1] * cos - v[2] * sin, v[1] * sin + v[2] * cos]
}

/// Projects a camera-space point (camera at the origin, looking down
/// +Z) to normalized device coordinates via a simple perspective
/// divide. Vulkan's NDC has +Y pointing down (the same detail
/// `hello_triangle`'s own module docs call out); negating `y` here is
/// what keeps "up" in this cube's object space looking like "up" on
/// screen instead of upside down.
fn project(v: Vertex3, focal_length: f32, aspect_ratio: f32) -> (f32, f32) {
    let x = (v[0] * focal_length) / (v[2] * aspect_ratio);
    let y = -(v[1] * focal_length) / v[2];
    (x, y)
}

/// One frame's worth of already-sorted, already-projected vertex data,
/// in the exact `Float32x2` position + `Float32x3` color layout
/// [`WGSL_SOURCE`] expects.
fn build_frame_vertices(spin_radians: f32) -> Vec<f32> {
    const TILT_RADIANS: f32 = -std::f32::consts::FRAC_PI_6; // -30 degrees: shows the top face
    const CAMERA_DISTANCE: f32 = 3.2;
    const FOCAL_LENGTH: f32 = 2.2;
    let aspect_ratio = WIDTH as f32 / HEIGHT as f32;

    // Transform all 8 corners once; every face's two triangles reuse
    // these rather than re-transforming shared corners per triangle.
    let camera_space: Vec<Vertex3> = CUBE_CORNERS
        .iter()
        .map(|&corner| {
            let spun = rotate_y(corner, spin_radians);
            let tilted = rotate_x(spun, TILT_RADIANS);
            [tilted[0], tilted[1], tilted[2] + CAMERA_DISTANCE]
        })
        .collect();

    // One entry per triangle (two per face): its average camera-space
    // depth (for the painter's-algorithm sort) and its three corner
    // indices plus color.
    struct Triangle {
        avg_depth: f32,
        corners: [usize; 3],
        color: [f32; 3],
    }
    let mut triangles: Vec<Triangle> = Vec::with_capacity(FACES.len() * 2);
    for face in &FACES {
        for tri in face.corners.chunks_exact(3) {
            let depth =
                (camera_space[tri[0]][2] + camera_space[tri[1]][2] + camera_space[tri[2]][2]) / 3.0;
            triangles.push(Triangle {
                avg_depth: depth,
                corners: [tri[0], tri[1], tri[2]],
                color: face.color,
            });
        }
    }
    // Farthest first, nearest last -- painter's algorithm: each later
    // triangle is drawn on top of, not blended with, anything earlier.
    triangles.sort_by(|a, b| b.avg_depth.total_cmp(&a.avg_depth));

    let mut vertices = Vec::with_capacity(triangles.len() * 3 * 5);
    for triangle in &triangles {
        for &corner_index in &triangle.corners {
            let (ndc_x, ndc_y) = project(camera_space[corner_index], FOCAL_LENGTH, aspect_ratio);
            vertices.extend_from_slice(&[ndc_x, ndc_y]);
            vertices.extend_from_slice(&triangle.color);
        }
    }
    vertices
}

/// Compiles [`WGSL_SOURCE`] to SPIR-V for one shader stage -- the same
/// real `naga` calls `canary-render-vulkan`'s own `hello_triangle` test
/// already validates against this workspace's toolchain.
fn compile_stage(stage: naga::ShaderStage) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(WGSL_SOURCE).expect("failed to parse WGSL");
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
    let vertex_attributes = [
        VertexAttribute {
            shader_location: 0,
            format: VertexFormat::Float32x2,
            offset: 0,
        },
        VertexAttribute {
            shader_location: 1,
            format: VertexFormat::Float32x3,
            offset: VertexFormat::Float32x2.size_bytes(),
        },
    ];
    let pipeline = device.create_pipeline(&PipelineDescriptor {
        label: "spinning-cube pipeline",
        vertex_shader_spirv: &vertex_spirv,
        vertex_entry_point: "vs_main",
        fragment_shader_spirv: &fragment_spirv,
        fragment_entry_point: "fs_main",
        vertex_stride: VertexFormat::Float32x2.size_bytes() + VertexFormat::Float32x3.size_bytes(),
        vertex_attributes: &vertex_attributes,
    });

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
        let vertex_data = build_frame_vertices(spin_radians);
        let vertex_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                vertex_data.as_ptr().cast::<u8>(),
                std::mem::size_of_val(vertex_data.as_slice()),
            )
        };
        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: "spinning-cube vertices",
            data: vertex_bytes,
        });
        let vertex_count = (vertex_data.len() / 5) as u32;

        let mut encoder = device.create_command_encoder();
        encoder.begin_render_pass(
            &color_target,
            &RenderPassDescriptor {
                clear_color: CLEAR_COLOR,
            },
        );
        encoder.set_pipeline(&pipeline);
        encoder.set_vertex_buffer(&vertex_buffer);
        encoder.draw(vertex_count);
        encoder.end_render_pass();
        device.submit_and_wait(encoder);

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
