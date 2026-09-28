// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The `.13` UI slice's blending proof: a half-alpha textured draw over a
//! solid clear must composite (src × srcA + dst × (1 − srcA)), not replace.
//! Without blending, `egui` text and translucent panels would rasterize as
//! opaque boxes — the exact failure this test exists to catch.
//!
//! A same-draw control through the unblended textured pipeline proves the
//! setup distinguishes blend on from blend off: the control must read back
//! opaque, the blended draw must read back mixed.
//!
//! `#[ignore]`d by default, same as `hello_triangle`: needs a real Vulkan
//! ICD. Run explicitly:
//!
//! ```sh
//! cargo test -p canary-render-vulkan --test blended_textured_pipeline -- --ignored
//! ```

use canary_render::{
    BufferDescriptor, ColorTargetDescriptor, CommandEncoder, PipelineDescriptor, RenderDevice,
    RenderPassDescriptor, TextureDescriptor, VertexAttribute, VertexFormat,
};
use canary_render_vulkan::VulkanDevice;

/// Position + UV + RGBA color: the `.13` UI vertex shape (see
/// `canary-ui-egui`'s painter). The fragment multiplies the sampled texel
/// by the vertex color, so texture alpha and vertex alpha compose.
const WGSL_SOURCE: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
}
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(input.position, 0.0, 1.0);
    out.uv = input.uv;
    out.color = input.color;
    return out;
}
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, input.uv) * input.color;
}
"#;

/// Same `naga` WGSL → SPIR-V path as `hello_triangle`'s harness: parse,
/// validate, cross-compile one stage.
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
        // Wildcard, not an exhaustive variant list: naga grows `ShaderStage`
        // over majors; this harness only ever compiles vertex + fragment.
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

/// Draws the covering triangle with `pipeline` over a blue clear and reads
/// back the center pixel. The caller owns begin/end/submit so both the
/// control and the blended draw share one setup.
fn draw_center_pixel(
    device: &VulkanDevice,
    pipeline: &canary_render_vulkan::VulkanPipeline,
    vertex_buffer: &canary_render_vulkan::VulkanBuffer,
    gpu_texture: &canary_render_vulkan::VulkanTexture,
) -> [u8; 4] {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 64;
    let color_target = device.create_color_target(&ColorTargetDescriptor {
        width: WIDTH,
        height: HEIGHT,
    });
    let mut encoder = device.create_command_encoder();
    encoder.begin_render_pass(
        &color_target,
        &RenderPassDescriptor {
            clear_color: [0.0, 0.0, 1.0, 1.0],
        },
    );
    encoder.set_pipeline(pipeline);
    encoder.set_vertex_buffer(vertex_buffer);
    encoder.set_texture(gpu_texture);
    encoder.draw(3);
    encoder.end_render_pass();
    device.submit_and_wait(encoder);

    let pixels = device.read_color_target_rgba8(&color_target);
    pixel_at(&pixels, WIDTH, WIDTH / 2, HEIGHT / 2)
}

#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn blended_textured_draws_composite_half_alpha_over_clear() {
    let device = VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (this sandbox needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    });

    // A triangle covering the whole target: opaque red vertex color, UVs
    // pinned to the single texel. (pos.xy, uv, color.rgba per vertex.)
    #[rustfmt::skip]
    let vertices: [f32; 24] = [
        -1.0, -1.0,   0.0, 0.0,   1.0, 0.0, 0.0, 1.0,
         3.0, -1.0,   0.0, 0.0,   1.0, 0.0, 0.0, 1.0,
        -1.0,  3.0,   0.0, 0.0,   1.0, 0.0, 0.0, 1.0,
    ];
    let vertex_bytes: Vec<u8> = vertices
        .iter()
        .flat_map(|v: &f32| v.to_ne_bytes())
        .collect();
    let vertex_buffer = device.create_buffer(&BufferDescriptor {
        label: "blend-proof vertices",
        data: &vertex_bytes,
    });

    // One white texel at half alpha: the sampled fragment is
    // (1, 0, 0, 128/255) after the red multiply.
    let gpu_texture = device.create_texture(&TextureDescriptor {
        label: "blend-proof texture",
        width: 1,
        height: 1,
        rgba8: &[255, 255, 255, 128],
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
            format: VertexFormat::Float32x2,
            offset: VertexFormat::Float32x2.size_bytes(),
        },
        VertexAttribute {
            shader_location: 2,
            format: VertexFormat::Float32x4,
            offset: VertexFormat::Float32x2.size_bytes() * 2,
        },
    ];
    let desc = PipelineDescriptor {
        label: "blend-proof pipeline",
        vertex_shader_spirv: &vertex_spirv,
        vertex_entry_point: "vs_main",
        fragment_shader_spirv: &fragment_spirv,
        fragment_entry_point: "fs_main",
        vertex_stride: VertexFormat::Float32x2.size_bytes() * 2
            + VertexFormat::Float32x4.size_bytes(),
        vertex_attributes: &vertex_attributes,
    };

    // Control: the unblended textured pipeline replaces the clear — the
    // source fragment verbatim, including its half alpha. If this is
    // not the raw source, the setup (not the blending) is wrong.
    let plain = device.create_textured_pipeline(&desc);
    let control = draw_center_pixel(&device, &plain, &vertex_buffer, &gpu_texture);
    assert_eq!(
        control,
        [255, 0, 0, 128],
        "the unblended control must replace the blue clear with the raw source, got {control:?}"
    );

    // Blended: src(1,0,0,0.502) over dst(0,0,1,1) composites to
    // (0.502, 0, 0.498, 1.0) — u8 (128, 0, 127, 255) within rounding.
    let blended = device.create_blended_textured_pipeline(&desc);
    let pixel = draw_center_pixel(&device, &blended, &vertex_buffer, &gpu_texture);
    for (channel, expected) in [(pixel[0], 128), (pixel[2], 127)] {
        assert!(
            channel.abs_diff(expected) <= 2,
            "blended composite must mix red over blue, expected ~{expected}, got {pixel:?}"
        );
    }
    assert_eq!(pixel[1], 0, "green stays zero, got {pixel:?}");
    assert!(
        pixel[3].abs_diff(255) <= 1,
        "composite alpha must stay opaque, got {pixel:?}"
    );
}
