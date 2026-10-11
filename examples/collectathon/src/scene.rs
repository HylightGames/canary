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

use canary_assets::Texture;
use canary_render::{
    BufferDescriptor, CommandEncoder, PipelineDescriptor, RenderDevice, TextureDescriptor,
    VertexAttribute, VertexFormat,
};
use canary_render_ecs::{
    record_baked_frame, textured_vertex_attributes, textured_vertex_stride, BakedFrame,
    BakedTexturedFrame, OrthoView, OrthographicCamera, TEXTURED_WGSL,
};
use canary_render_vulkan::{VulkanBuffer, VulkanDevice, VulkanPipeline, VulkanTexture};
use glam::Mat4;

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
///
/// This is the legacy hand-built projection: it assumes the square arena
/// fills the target vertically. For the camera-driven equivalent that
/// widens horizontally on non-square targets, see [`world_ndc_ortho`].
pub fn world_ndc(x_px: f32, y_px: f32) -> [f32; 2] {
    [
        (x_px / ARENA_HALF_PX).clamp(-MAX_NDC, MAX_NDC),
        (-(y_px / ARENA_HALF_PX)).clamp(-MAX_NDC, MAX_NDC),
    ]
}

/// The collectathon's 2D camera: frames the square arena vertically, so
/// the room's half-extent maps exactly onto the NDC vertical range and
/// wider targets show more horizontally through the aspect term.
///
/// A plain [`OrthographicCamera`] value (never perspective/3D — see
/// `canary-render-ecs`'s camera docs for the rejection): the scheduled
/// ortho bake systems resolve this shape via [`resolve_ortho_camera`](canary_render_ecs::resolve_ortho_camera),
/// and [`collectathon_view`] pins it to the identity transform the
/// sample's center-origin world assumes.
///
/// [`resolve_ortho_camera`](canary_render_ecs::resolve_ortho_camera)
pub fn collectathon_camera() -> OrthographicCamera {
    OrthographicCamera::from_arena_half_extent(ARENA_HALF_PX)
}

/// Resolves the collectathon camera into an [`OrthoView`] for a target of
/// `aspect` (width divided by height).
///
/// The view matrix is the identity: the sample's world is already
/// center-origin with y positive downward, and the camera sits in the
/// room plane with no rotation — the conventional 2D setup the ortho
/// bake documents. Returns `None` when `aspect` is not a positive finite
/// ratio, or when the arena camera itself is invalid (impossible for the
/// constant half-extent, checked rather than assumed): callers fall back
/// to the legacy [`world_ndc`] path, which keeps camera-less output
/// byte-identical.
pub fn collectathon_view(aspect: f32) -> Option<OrthoView> {
    if !(aspect > 0.0 && aspect.is_finite()) {
        return None;
    }
    let camera = collectathon_camera();
    if !camera.is_valid() {
        return None;
    }
    Some(OrthoView {
        view: Mat4::IDENTITY,
        half_height: camera.half_height,
        near: camera.near,
        far: camera.far,
    })
}

/// Projects a world offset through the collectathon camera into NDC for
/// a target of `aspect` (width divided by height), clamped so geometry
/// never leaves the frame.
///
/// This is the same projection
/// [`bake_scene_to_vertices_ortho`](canary_render_ecs::bake_scene_to_vertices_ortho)
/// applies — `ndc_x` scales by the half-height and aspect, `ndc_y` is the
/// negated half-height fraction (the Y-flip convention: world y grows
/// downward here, NDC y grows upward) — evaluated for one point instead
/// of a baked soup. At square aspect (`aspect == 1.0`) it agrees exactly
/// with [`world_ndc`]; on wider targets `x` shrinks by the aspect while
/// `y` is unchanged.
///
/// Returns `None` when `aspect` is not a positive finite ratio or either
/// coordinate (or the projected result) is non-finite: the caller keeps
/// the legacy [`world_ndc`] output rather than uploading garbage.
pub fn world_ndc_ortho(x_px: f32, y_px: f32, aspect: f32) -> Option<[f32; 2]> {
    let view = collectathon_view(aspect)?;
    if !x_px.is_finite() || !y_px.is_finite() {
        return None;
    }
    let ndc = [
        (x_px / view.half_height) / aspect,
        (-(y_px) / view.half_height),
    ];
    if !ndc.iter().all(|v| v.is_finite()) {
        return None;
    }
    Some([
        ndc[0].clamp(-MAX_NDC, MAX_NDC),
        ndc[1].clamp(-MAX_NDC, MAX_NDC),
    ])
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
///
/// Holds the flat-color soup pipeline for the legacy scene and the
/// bridge's mesh bake, plus the single-texture pipeline for the bridge's
/// textured bake — both record into the caller's open pass, so the scene
/// and the UI submit once, together.
pub struct ScenePipeline {
    pipeline: VulkanPipeline,
    textured_pipeline: VulkanPipeline,
}

impl ScenePipeline {
    /// Compiles the scene shader and the bridge's textured shader, and
    /// creates both pipelines on `device`.
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
        let textured_vs = crate::shader::compile_stage(TEXTURED_WGSL, naga::ShaderStage::Vertex);
        let textured_fs = crate::shader::compile_stage(TEXTURED_WGSL, naga::ShaderStage::Fragment);
        let textured_attributes = textured_vertex_attributes();
        let textured_pipeline = device.create_textured_pipeline(&PipelineDescriptor {
            label: "collectathon textured scene pipeline",
            vertex_shader_spirv: &textured_vs,
            vertex_entry_point: "vs_main",
            fragment_shader_spirv: &textured_fs,
            fragment_entry_point: "fs_main",
            vertex_stride: textured_vertex_stride(),
            vertex_attributes: &textured_attributes,
        });
        Self {
            pipeline,
            textured_pipeline,
        }
    }

    /// Records a bridge-baked mesh frame into the caller's open pass:
    /// `set_pipeline` → `set_vertex_buffer` → `draw(vertex_count)`.
    ///
    /// Returns the uploaded vertex buffer (`None` for an empty frame,
    /// which records nothing): the caller must hold it until after the
    /// submission that reads it — dropping it earlier is use-after-free,
    /// the same contract
    /// [`record_baked_frame`](canary_render_ecs::record_baked_frame)
    /// documents, which this wraps.
    pub fn record_soup_frame(
        &self,
        device: &VulkanDevice,
        encoder: &mut <VulkanDevice as RenderDevice>::CommandEncoder<'_>,
        frame: &BakedFrame,
    ) -> Option<VulkanBuffer> {
        record_baked_frame(device, encoder, &self.pipeline, frame)
    }

    /// Records a bridge-baked textured frame into the caller's open pass:
    /// `set_pipeline` → `set_vertex_buffer` → `set_texture` →
    /// `draw(vertex_count)`.
    ///
    /// The in-pass mirror of
    /// [`draw_textured_frame`](canary_render_ecs::draw_textured_frame)'s
    /// interior (which opens its own pass and cannot run inside this
    /// one): fresh vertex upload plus fresh texture upload from
    /// `texture`'s bytes, recorded after the caller's pipeline bind so
    /// the texture lands in the textured layout. Call order is
    /// load-bearing — pipeline before texture, which the backend reports
    /// loudly when violated.
    ///
    /// Returns the uploaded vertex buffer plus the GPU texture (`None`
    /// for an empty frame, which records nothing): the caller must hold
    /// both until after the submission that reads them.
    pub fn record_textured_frame(
        &self,
        device: &VulkanDevice,
        encoder: &mut <VulkanDevice as RenderDevice>::CommandEncoder<'_>,
        frame: &BakedTexturedFrame,
        texture: &Texture,
    ) -> Option<(VulkanBuffer, VulkanTexture)> {
        // Same malformed-frame guard as the bridge's own draw paths: a
        // misaligned tail would make `vertex_count`'s floored division
        // under-read the draw while the upload carries the extra bytes.
        // The stride is 4 floats per textured vertex (position plus UV).
        debug_assert!(
            frame.vertices.len() % 4 == 0,
            "malformed BakedTexturedFrame: {} floats is not a multiple of the 4-float textured stride",
            frame.vertices.len()
        );
        if frame.is_empty() {
            return None;
        }
        let vertex_bytes: Vec<u8> = frame
            .vertices
            .iter()
            .flat_map(|vertex: &f32| vertex.to_ne_bytes())
            .collect();
        let vertex_buffer: VulkanBuffer = device.create_buffer(&BufferDescriptor {
            label: "collectathon textured scene",
            data: &vertex_bytes,
        });
        let gpu_texture = device.create_texture(&TextureDescriptor {
            label: "collectathon textured scene texture",
            width: texture.width(),
            height: texture.height(),
            rgba8: texture.rgba8(),
        });
        encoder.set_pipeline(&self.textured_pipeline);
        encoder.set_vertex_buffer(&vertex_buffer);
        encoder.set_texture(&gpu_texture);
        encoder.draw(frame.vertex_count());
        Some((vertex_buffer, gpu_texture))
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

    #[test]
    fn collectathon_camera_frames_the_arena() {
        // The camera must be valid and name the arena half-extent as its
        // vertical half-height: the room fills the target vertically.
        let camera = collectathon_camera();
        assert!(camera.is_valid(), "the arena camera must project");
        assert_eq!(
            camera.half_height, ARENA_HALF_PX,
            "vertical half-height must equal the arena half-extent"
        );
    }

    #[test]
    fn collectathon_view_rejects_an_unusable_aspect() {
        // The windowed sample runs 960x600 (aspect 1.6); anything
        // non-positive or non-finite must resolve to absence (legacy
        // fallback), never to garbage pixels.
        assert!(collectathon_view(960.0 / 600.0).is_some());
        assert!(collectathon_view(1.0).is_some());
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                collectathon_view(bad).is_none(),
                "aspect {bad} must resolve to no camera"
            );
            assert!(
                world_ndc_ortho(10.0, 10.0, bad).is_none(),
                "aspect {bad} must project to no point"
            );
        }
    }

    #[test]
    fn ortho_helper_agrees_with_world_ndc_at_square_aspect() {
        // At square aspect the camera path is the same math as the legacy
        // hand-built path, so every in-bounds point must match exactly and
        // the parking-spot clamp must agree out of bounds.
        for (x, y) in [
            (0.0, 0.0),
            (20.0, -10.0),
            (-70.0, 45.0),
            (ARENA_HALF_PX, 0.0),
            (10_000.0, -10_000.0),
        ] {
            assert_eq!(
                world_ndc_ortho(x, y, 1.0),
                Some(world_ndc(x, y)),
                "ortho and legacy must agree at square aspect for ({x}, {y})"
            );
        }
    }

    #[test]
    fn collectathon_scene_uses_ortho_bake_not_legacy_fallback() {
        use canary_render_ecs::{
            bake_scene_to_vertices_ortho, bake_scene_to_vertices_with_aspect, RenderItem,
        };
        use canary_transform::GlobalTransform;

        // Given: one arena-space triangle (world y positive downward, `z`
        // coplanar with the in-plane camera) plus the collectathon camera
        // at the windowed sample's aspect (960x600).
        let aspect = 960.0 / 600.0;
        let view = collectathon_view(aspect).expect("window aspect must resolve");
        let item = RenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::IDENTITY),
            vertices: vec![[20.0, -10.0, 0.0], [60.0, -10.0, 0.0], [20.0, 30.0, 0.0]],
            color: [1.0, 0.6, 0.0],
        };
        let items = std::slice::from_ref(&item);

        // When: baking through the ortho camera path and through the
        // legacy fixed-perspective fallback at the same aspect.
        let ortho = bake_scene_to_vertices_ortho(items, view, aspect);
        let legacy = bake_scene_to_vertices_with_aspect(items, aspect);

        // Then: the ortho bake emits the triangle (coplanar `z` is inside
        // `[near, far]`), its first vertex equals the one-point ortho
        // helper for the same corner, and it differs from the legacy
        // bake — proving the camera path is used, not the fallback.
        assert_eq!(
            ortho.len(),
            3 * 5,
            "one triangle must survive the ortho bake"
        );
        let want = world_ndc_ortho(20.0, -10.0, aspect).expect("corner must project");
        for (actual, expected) in ortho[0..2].iter().zip(want.iter()) {
            assert!(
                (actual - expected).abs() < 1e-5,
                "ortho vertex {actual} must match the camera helper {expected}"
            );
        }
        assert_ne!(
            ortho, legacy,
            "ortho output must differ from the legacy fallback at aspect {aspect}"
        );
    }
}
