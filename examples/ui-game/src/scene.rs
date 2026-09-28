// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The offscreen scene: one player triangle over a clear color, drawn
//! through the RHI into the same color target the UI paints into.
//!
//! The shader is the same proven shape as the presentation proof's content
//! shader (NDC position plus flat color, no uniforms): the RHI has no
//! uniforms yet, so per-frame motion uploads a fresh three-vertex buffer —
//! the same "fresh buffer per frame" approach `spinning-cube` documents,
//! correct given today's trait rather than a workaround.

use canary_render::{
    BufferDescriptor, CommandEncoder, PipelineDescriptor, RenderDevice, VertexAttribute,
    VertexFormat,
};
use canary_render_vulkan::{VulkanBuffer, VulkanDevice, VulkanPipeline};

/// The scene shader: position plus flat color, straight to clip space.
const SCENE_WGSL: &str = r#"
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

/// World range mapped to the screen edges: the player stays visible while
/// exploring, then parks at the rim instead of leaving the frame.
/// `pub(crate)` so the simulation clamps to the same bound it renders —
/// state and screen agree, no invisible drift past the edge.
pub(crate) const HALF_RANGE_PX: f32 = 200.0;
/// Clamped NDC parking spot: strictly inside the edge so the whole
/// triangle stays on screen.
const MAX_NDC: f32 = 0.9;
/// Player triangle half-size in NDC x.
const TRI_HALF: f32 = 0.05;
/// Player triangle color: amber.
const TRI_COLOR: [f32; 3] = [1.0, 0.6, 0.0];

/// Maps a world offset (logical pixels, scene center origin) to NDC,
/// clamped so the triangle never leaves the frame. One concept, one
/// function: both axes share the range and the parking clamp.
/// World y grows downward (screen convention); NDC y grows upward, so the
/// y component is negated — pressing down moves the triangle down.
pub fn player_ndc(x_px: f32, y_px: f32) -> [f32; 2] {
    [
        (x_px / HALF_RANGE_PX).clamp(-MAX_NDC, MAX_NDC),
        (-(y_px / HALF_RANGE_PX)).clamp(-MAX_NDC, MAX_NDC),
    ]
}

/// The scene pipeline: compiled once, drawn every frame.
pub struct ScenePipeline {
    pipeline: VulkanPipeline,
}

impl ScenePipeline {
    /// Compiles the scene shader and creates the pipeline on `device`.
    pub fn new(device: &VulkanDevice) -> Self {
        let vertex_spirv = crate::shader::compile_stage(SCENE_WGSL, naga::ShaderStage::Vertex);
        let fragment_spirv = crate::shader::compile_stage(SCENE_WGSL, naga::ShaderStage::Fragment);
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
            label: "ui-game scene pipeline",
            vertex_shader_spirv: &vertex_spirv,
            vertex_entry_point: "vs_main",
            fragment_shader_spirv: &fragment_spirv,
            fragment_entry_point: "fs_main",
            vertex_stride: VertexFormat::Float32x2.size_bytes()
                + VertexFormat::Float32x3.size_bytes(),
            vertex_attributes: &vertex_attributes,
        });
        Self { pipeline }
    }

    /// Records the player triangle at `ndc` into the caller's open pass.
    /// Three arguments counting `self` matches the `draw_baked_frame`
    /// precedent: device, encoder, one frame-data value — the 2D offset is
    /// a single concept, not two parameters waiting to happen.
    ///
    /// Returns the uploaded vertex buffer: the caller must hold it until
    /// after the submission that reads it — dropping it earlier is
    /// use-after-free (a second frame on the same target deterministically
    /// loses the device in practice), the same contract
    /// [`record_baked_frame`](canary_render_ecs::record_baked_frame)
    /// documents.
    pub fn draw_player(
        &self,
        device: &VulkanDevice,
        encoder: &mut <VulkanDevice as RenderDevice>::CommandEncoder<'_>,
        ndc: [f32; 2],
    ) -> VulkanBuffer {
        let vertices: [f32; 15] = [
            ndc[0] - TRI_HALF,
            ndc[1] - TRI_HALF,
            TRI_COLOR[0],
            TRI_COLOR[1],
            TRI_COLOR[2], //
            ndc[0] + TRI_HALF,
            ndc[1] - TRI_HALF,
            TRI_COLOR[0],
            TRI_COLOR[1],
            TRI_COLOR[2], //
            ndc[0],
            ndc[1] + TRI_HALF,
            TRI_COLOR[0],
            TRI_COLOR[1],
            TRI_COLOR[2],
        ];
        let vertex_bytes: Vec<u8> = vertices
            .iter()
            .flat_map(|vertex: &f32| vertex.to_ne_bytes())
            .collect();
        let vertex_buffer: VulkanBuffer = device.create_buffer(&BufferDescriptor {
            label: "ui-game player triangle",
            data: &vertex_bytes,
        });
        encoder.set_pipeline(&self.pipeline);
        encoder.set_vertex_buffer(&vertex_buffer);
        encoder.draw(3);
        vertex_buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_center_maps_to_screen_center() {
        assert_eq!(player_ndc(0.0, 0.0), [0.0, 0.0]);
    }

    #[test]
    fn offscreen_world_parks_at_the_rim() {
        assert_eq!(player_ndc(10_000.0, 0.0), [MAX_NDC, 0.0]);
        assert_eq!(player_ndc(-10_000.0, 0.0), [-MAX_NDC, 0.0]);
        assert_eq!(player_ndc(HALF_RANGE_PX, 0.0), [MAX_NDC, 0.0]);
    }

    #[test]
    fn world_down_maps_to_screen_down() {
        // World y grows downward; NDC y grows upward, so positive world y
        // must land on negative NDC y.
        assert_eq!(player_ndc(0.0, HALF_RANGE_PX), [0.0, -MAX_NDC]);
        assert_eq!(player_ndc(0.0, -HALF_RANGE_PX), [0.0, MAX_NDC]);
    }
}
