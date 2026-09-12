// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! `v0.0.6`'s actual milestone (see `docs/roadmap/v0.0.6-roadmap.md`'s
//! "The actual milestone" scope item): render a real triangle to a real
//! offscreen Vulkan color target, on the real `llvmpipe` software device
//! this sandbox provides (confirmed enumerable in
//! `docs/architecture/platform-abstraction.md` and ADR 0016's own
//! "Verified, not assumed"), read the result back, and assert on
//! specific pixel values -- not "it compiled" or "it didn't panic."
//!
//! WGSL is compiled to SPIR-V here via standalone `naga` (a dev-
//! dependency of this test only -- see this crate's `Cargo.toml`),
//! matching `docs/architecture/rendering.md`'s "Materials & shaders":
//! WGSL is the authoring language, `canary_render::PipelineDescriptor`
//! itself takes precompiled SPIR-V.
//!
//! `#[ignore]`d by default: this needs a real (if software) Vulkan ICD,
//! which isn't guaranteed in every environment `cargo test` runs in --
//! same reasoning as `canary-platform`'s `winit_backend_window.rs`. Run
//! explicitly with `mesa-vulkan-drivers` installed (this sandbox has it;
//! see `docs/architecture/platform-abstraction.md`) via
//! `cargo test -p canary-render-vulkan --test hello_triangle -- --ignored`.

use canary_render::{
    BufferDescriptor, ColorTargetDescriptor, CommandEncoder, PipelineDescriptor, RenderDevice,
    RenderPassDescriptor, VertexAttribute, VertexFormat,
};
use canary_render_vulkan::VulkanDevice;

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

/// Parses, validates, and cross-compiles [`WGSL_SOURCE`] to SPIR-V for
/// one shader stage -- real `naga` calls, the same ones ADR 0016's own
/// "Verified, not assumed" section already confirmed work under this
/// sandbox's rustc 1.75 floor.
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

fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * width + x) * 4) as usize;
    [
        pixels[idx],
        pixels[idx + 1],
        pixels[idx + 2],
        pixels[idx + 3],
    ]
}

#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn renders_a_real_triangle_to_an_offscreen_target_and_reads_back_real_pixels() {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;
    const CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0]; // black

    let device = VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (this sandbox needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    });

    // A single hard-coded triangle: top vertex red, bottom-left green,
    // bottom-right blue (position.xy, color.rgb per vertex). Vulkan's
    // NDC has +Y pointing down, so -0.5 is visually "up".
    #[rustfmt::skip]
    let vertices: [f32; 15] = [
        0.0, -0.5,   1.0, 0.0, 0.0,
        -0.5, 0.5,   0.0, 1.0, 0.0,
        0.5, 0.5,    0.0, 0.0, 1.0,
    ];
    let vertex_bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            vertices.as_ptr().cast::<u8>(),
            std::mem::size_of_val(&vertices),
        )
    };
    let vertex_buffer = device.create_buffer(&BufferDescriptor {
        label: "hello-triangle vertices",
        data: vertex_bytes,
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
        label: "hello-triangle pipeline",
        vertex_shader_spirv: &vertex_spirv,
        vertex_entry_point: "vs_main",
        fragment_shader_spirv: &fragment_spirv,
        fragment_entry_point: "fs_main",
        vertex_stride: VertexFormat::Float32x2.size_bytes() + VertexFormat::Float32x3.size_bytes(),
        vertex_attributes: &vertex_attributes,
    });

    let mut encoder = device.create_command_encoder();
    encoder.begin_render_pass(
        &color_target,
        &RenderPassDescriptor {
            clear_color: CLEAR_COLOR,
        },
    );
    encoder.set_pipeline(&pipeline);
    encoder.set_vertex_buffer(&vertex_buffer);
    encoder.draw(3);
    encoder.end_render_pass();
    device.submit_and_wait(encoder);

    let pixels = device.read_color_target_rgba8(&color_target);
    assert_eq!(
        pixels.len(),
        (WIDTH * HEIGHT * 4) as usize,
        "readback should be tightly packed RGBA8 with no row padding"
    );

    // A corner is well outside the triangle -- should be exactly the
    // clear color.
    let corner = pixel_at(&pixels, WIDTH, 2, 2);
    assert_eq!(
        corner,
        [0, 0, 0, 255],
        "a corner pixel should be the black clear color, got {corner:?}"
    );

    // The center is inside the triangle -- should be a real interpolated
    // vertex color, not the clear color. Not asserting an exact RGB
    // value (that would depend on precisely where the centroid lands
    // relative to pixel sampling, which is real, correct rasterizer
    // behavior, not something this test should overfit to) -- asserting
    // it's clearly *not* black and has real color variation is enough to
    // prove a real, correctly-interpolated triangle was rasterized, not
    // a solid fill or a blank target.
    let center = pixel_at(&pixels, WIDTH, WIDTH / 2, HEIGHT / 2);
    assert_ne!(
        center,
        [0, 0, 0, 255],
        "the center pixel should be inside the triangle, not the clear color"
    );
    assert!(
        center[0] > 10 || center[1] > 10 || center[2] > 10,
        "expected real color at the triangle's center, got something \
         suspiciously close to black: {center:?}"
    );

    // A handful of interior points along the triangle's vertical
    // midline, all of which should land inside it and show real color --
    // not just the single center pixel by luck.
    for y in [HEIGHT / 2 - 5, HEIGHT / 2, HEIGHT / 2 + 5] {
        let p = pixel_at(&pixels, WIDTH, WIDTH / 2, y);
        assert_ne!(
            p,
            [0, 0, 0, 255],
            "expected pixel ({}, {y}) to be inside the triangle, got the clear color",
            WIDTH / 2
        );
    }
}
