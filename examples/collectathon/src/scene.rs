//! The offscreen scene: player quad plus pickup diamonds over a clear
//! color, drawn through the RHI into the same color target the UI paints
//! into.
//!
//! The shader is the same proven shape as the `ui-game` scene shader (NDC
//! position plus flat color, no uniforms): the RHI has no uniforms yet, so
//! per-frame motion uploads a fresh vertex buffer — the same "fresh buffer
//! per frame" approach `spinning-cube` documents, correct given today's
//! trait rather than a workaround. One draw call covers the whole scene:
//! the player quad first, then one diamond per uncollected pickup.

use canary_render::{
    BufferDescriptor, CommandEncoder, PipelineDescriptor, RenderDevice, VertexAttribute,
    VertexFormat,
};
use canary_render_vulkan::{VulkanBuffer, VulkanDevice, VulkanPipeline};

use crate::game::ARENA_HALF_PX;

/// Clamped NDC parking spot: strictly inside the edge so the whole quad
/// stays on screen.
const MAX_NDC: f32 = 0.9;
/// Player quad half-size in NDC.
const PLAYER_HALF: f32 = 0.06;
/// Pickup diamond half-size in NDC.
const PICKUP_HALF: f32 = 0.035;
/// Player quad color: amber.
const PLAYER_COLOR: [f32; 3] = [1.0, 0.6, 0.0];
/// Shard diamond color: cyan.
const SHARD_COLOR: [f32; 3] = [0.2, 0.9, 0.9];
/// Goal diamond color: gold.
const GOAL_COLOR: [f32; 3] = [1.0, 0.85, 0.2];

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

/// One pickup to draw: the caller filters to uncollected pickups, so the
/// scene never invents visibility state the simulation does not hold.
#[derive(Debug, Clone, Copy)]
pub struct PickupDraw {
    /// World offset in logical pixels, scene center origin.
    pub x: f32,
    /// World offset in logical pixels, positive downward.
    pub y: f32,
    /// True for the room goal (gold), false for a shard (cyan).
    pub is_goal: bool,
}

/// Maps a world offset (logical pixels, scene center origin) to NDC,
/// clamped so geometry never leaves the frame. World y grows downward
/// (screen convention); NDC y grows upward, so the y component is
/// negated — pressing down moves the quad down.
pub fn world_ndc(x_px: f32, y_px: f32) -> [f32; 2] {
    [
        (x_px / ARENA_HALF_PX).clamp(-MAX_NDC, MAX_NDC),
        (-(y_px / ARENA_HALF_PX)).clamp(-MAX_NDC, MAX_NDC),
    ]
}

/// Pushes one colored vertex into the frame's vertex stream.
fn push_vertex(vertices: &mut Vec<f32>, x: f32, y: f32, color: [f32; 3]) {
    vertices.extend_from_slice(&[x, y, color[0], color[1], color[2]]);
}

/// Pushes one axis-aligned quad (two triangles) at `center`.
fn push_quad(vertices: &mut Vec<f32>, center: [f32; 2], half: f32, color: [f32; 3]) {
    let (cx, cy) = (center[0], center[1]);
    for (x, y) in [
        (cx - half, cy - half),
        (cx + half, cy - half),
        (cx - half, cy + half),
        (cx - half, cy + half),
        (cx + half, cy - half),
        (cx + half, cy + half),
    ] {
        push_vertex(vertices, x, y, color);
    }
}

/// Pushes one diamond (two triangles) at `center`.
fn push_diamond(vertices: &mut Vec<f32>, center: [f32; 2], half: f32, color: [f32; 3]) {
    let (cx, cy) = (center[0], center[1]);
    for (x, y) in [
        (cx, cy - half),
        (cx + half, cy),
        (cx - half, cy),
        (cx, cy + half),
        (cx - half, cy),
        (cx + half, cy),
    ] {
        push_vertex(vertices, x, y, color);
    }
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
            label: "collectathon scene pipeline",
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

    /// Records the player quad plus one diamond per pickup into the
    /// caller's open pass. Player and pickups arrive as NDC/world pairs;
    /// the mapping is one concept, one function ([`world_ndc`]).
    ///
    /// Returns the uploaded vertex buffer: the caller must hold it until
    /// after the submission that reads it — dropping it earlier is
    /// use-after-free, the same contract
    /// [`record_baked_frame`](canary_render_ecs::record_baked_frame)
    /// documents.
    pub fn draw_scene(
        &self,
        device: &VulkanDevice,
        encoder: &mut <VulkanDevice as RenderDevice>::CommandEncoder<'_>,
        player_ndc: [f32; 2],
        pickups: &[PickupDraw],
    ) -> VulkanBuffer {
        let mut vertices = Vec::with_capacity(5 * (6 + pickups.len() * 6));
        push_quad(&mut vertices, player_ndc, PLAYER_HALF, PLAYER_COLOR);
        for pickup in pickups {
            let color = if pickup.is_goal {
                GOAL_COLOR
            } else {
                SHARD_COLOR
            };
            push_diamond(
                &mut vertices,
                world_ndc(pickup.x, pickup.y),
                PICKUP_HALF,
                color,
            );
        }
        let vertex_count = u32::try_from(vertices.len() / 5).unwrap_or(0);
        let vertex_bytes: Vec<u8> = vertices
            .iter()
            .flat_map(|vertex: &f32| vertex.to_ne_bytes())
            .collect();
        let vertex_buffer: VulkanBuffer = device.create_buffer(&BufferDescriptor {
            label: "collectathon scene",
            data: &vertex_bytes,
        });
        encoder.set_pipeline(&self.pipeline);
        encoder.set_vertex_buffer(&vertex_buffer);
        encoder.draw(vertex_count);
        vertex_buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_center_maps_to_screen_center() {
        assert_eq!(world_ndc(0.0, 0.0), [0.0, 0.0]);
    }

    #[test]
    fn offscreen_world_parks_at_the_rim() {
        assert_eq!(world_ndc(10_000.0, 0.0), [MAX_NDC, 0.0]);
        assert_eq!(world_ndc(-10_000.0, 0.0), [-MAX_NDC, 0.0]);
        assert_eq!(world_ndc(ARENA_HALF_PX, 0.0), [MAX_NDC, 0.0]);
    }

    #[test]
    fn world_down_maps_to_screen_down() {
        // World y grows downward; NDC y grows upward, so positive world y
        // must land on negative NDC y.
        assert_eq!(world_ndc(0.0, ARENA_HALF_PX), [0.0, -MAX_NDC]);
        assert_eq!(world_ndc(0.0, -ARENA_HALF_PX), [0.0, MAX_NDC]);
    }
}
