// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Encoder misuse loudness: `set_texture` against the wrong pipeline must
//! fail on the host, not record undefined work for the GPU.
//!
//! `#[ignore]`d by default like `hello_triangle`: needs a real Vulkan ICD.
//! Run explicitly via
//! `cargo test -p canary-render-vulkan --test texture_binding_misuse -- --ignored`.

use canary_render::{
    ColorTargetDescriptor, CommandEncoder, PipelineDescriptor, RenderDevice, RenderPassDescriptor,
    TextureDescriptor, VertexAttribute, VertexFormat,
};
use canary_render_vulkan::VulkanDevice;

const SOUP_WGSL: &str = r#"
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

/// Parses, validates, and cross-compiles [`SOUP_WGSL`] to SPIR-V — the same
/// real `naga` calls `hello_triangle` makes.
fn compile_stage(stage: naga::ShaderStage) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(SOUP_WGSL).expect("failed to parse WGSL");
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
}

#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn set_texture_after_a_non_textured_pipeline_fails_loudly() {
    let device = VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (this sandbox needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    });
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: 8,
        height: 8,
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
    let soup_pipeline = device.create_pipeline(&PipelineDescriptor {
        label: "soup pipeline with no texture layout",
        vertex_shader_spirv: &vertex_spirv,
        vertex_entry_point: "vs_main",
        fragment_shader_spirv: &fragment_spirv,
        fragment_entry_point: "fs_main",
        vertex_stride: VertexFormat::Float32x2.size_bytes() + VertexFormat::Float32x3.size_bytes(),
        vertex_attributes: &vertex_attributes,
    });
    let texture = device.create_texture(&TextureDescriptor {
        label: "misuse probe texture",
        width: 2,
        height: 2,
        rgba8: &[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    });

    // Given: a bound pipeline whose layout has no descriptor sets, when a
    // texture bind is recorded against it, then the encoder must panic on
    // the host — binding set 0 into a set-less layout is undefined work
    // the driver (without validation layers) would otherwise accept
    // silently.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut encoder = device.create_command_encoder();
        encoder.begin_render_pass(
            &target,
            &RenderPassDescriptor {
                clear_color: [0.0, 0.0, 0.0, 1.0],
            },
        );
        encoder.set_pipeline(&soup_pipeline);
        encoder.set_texture(&texture);
    }));

    assert!(
        result.is_err(),
        "set_texture after a non-textured pipeline must panic loudly instead of recording a misbound descriptor set"
    );
}

#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn draw_without_anything_bound_fails_loudly() {
    let device = VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (this sandbox needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    });
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: 8,
        height: 8,
    });

    // Given: a render pass with no pipeline and no vertex buffer bound,
    // when a draw is recorded, then the encoder must panic on the host
    // — `vkCmdDraw` with nothing bound is driver-undefined (the same
    // class the `set_texture` guard above was built for). The abandoned
    // encoder is dropped inside `catch_unwind`, which also exercises
    // the `Drop` impl that frees its command buffer.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut encoder = device.create_command_encoder();
        encoder.begin_render_pass(
            &target,
            &RenderPassDescriptor {
                clear_color: [0.0, 0.0, 0.0, 1.0],
            },
        );
        encoder.draw(3);
        encoder.end_render_pass();
        device.submit_and_wait(encoder);
    }));

    assert!(
        result.is_err(),
        "draw with no pipeline bound must panic loudly instead of recording driver-undefined work"
    );
}
