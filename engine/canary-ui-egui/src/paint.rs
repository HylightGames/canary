//! `UiPaint` through the RHI: the `.13` submit path.
//!
//! [`UiPainter`] is generic over [`RenderDevice`](canary_render::RenderDevice),
//! so this adapter names no Vulkan/ash type: the same painter submits
//! through any backend implementing the RHI. It records into the caller's
//! already-open render pass — the sample draws the scene first, then calls
//! [`UiPainter::paint`] before ending the pass, so one pass composites
//! scene plus UI with no intermediate clear (the RHI has no load-op; a
//! second pass would wipe the scene).
//!
//! Per-batch vertex buffers (one upload per batch per frame) are the
//! honest shape at `.13` HUD scale under the RHI's upload-once buffer
//! model — the same fresh-buffer-per-frame pattern
//! `canary-render-ecs`'s bridge uses — not a performance claim. A buffer
//! cache arrives when a second consumer outgrows this, not smuggled in
//! here.

use std::collections::HashMap;

use canary_render::{
    BufferDescriptor, CommandEncoder, PipelineDescriptor, RenderDevice, ScissorRect,
    TextureDescriptor, VertexAttribute, VertexFormat,
};
use canary_ui_core::{UiDrawBatch, UiPaint, UiTextureId, UiTextureOp};

/// The UI authoring source: textured passthrough plus a per-vertex RGBA
/// multiply. Positions arrive pre-transformed to clip space (the painter's
/// [`encode_batch`] maps logical pixels to NDC on the CPU — all real
/// transforms happen there, mirroring the soup bridge's
/// `RENDER_WGSL` stance); the fragment samples the bound texture and
/// multiplies by the vertex color, so texture alpha and vertex alpha
/// compose before the pipeline's src-alpha blending composites the batch
/// over the scene. The two bindings (`tex` at binding 0, `samp` at
/// binding 1, both set 0) mirror the RHI's
/// `create_blended_textured_pipeline` contract.
pub const UI_PAINT_WGSL: &str = r#"
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

/// The vertex attribute layout [`UiPainter`] uploads: NDC position, then
/// UV, then linear RGBA.
///
/// Offsets derive from [`VertexFormat::size_bytes`] rather than hard-coding
/// `8`/`16`, for the same fail-loud reason `canary-render-ecs`'s layout
/// helpers document; `ui_attributes_match_layout` pins the layout without
/// a GPU.
pub fn ui_vertex_attributes() -> [VertexAttribute; 3] {
    [
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
    ]
}

/// The byte stride between consecutive UI vertices: 8 position + 8 UV + 16
/// color = 32. Derived, not magic, so stride and layout share one source
/// of truth.
pub fn ui_vertex_stride() -> u32 {
    VertexFormat::Float32x2.size_bytes() * 2 + VertexFormat::Float32x4.size_bytes()
}

/// The render target UI paint submits into: physical extent plus the scale
/// that maps the paint's logical pixels onto it.
///
/// The adapter tessellates at 1.0 pixels-per-point (logical pixels), so
/// the painter scales clip rects by `pixels_per_point` for scissor rects
/// and maps NDC against the physical extent. Kept as one struct so
/// [`UiPainter::paint`] stays at three parameters.
#[derive(Debug, Clone, Copy)]
pub struct UiViewport {
    /// Render-target width in physical pixels.
    pub target_width_px: u32,
    /// Render-target height in physical pixels.
    pub target_height_px: u32,
    /// Physical pixels per logical (paint-space) pixel. Must be positive
    /// and finite; anything else is caller misuse, refused loudly.
    pub pixels_per_point: f32,
}

impl UiViewport {
    /// Viewport width in logical pixels.
    fn logical_width(self) -> f32 {
        // `u32` to `f32` widens; precision loss above 2^24 needs a
        // 16-megapixel-wide target, far beyond any real display.
        self.target_width_px as f32 / self.pixels_per_point
    }

    /// Viewport height in logical pixels.
    fn logical_height(self) -> f32 {
        // Same widening justification as [`Self::logical_width`].
        self.target_height_px as f32 / self.pixels_per_point
    }
}

/// What one [`UiPainter::paint`] call did, for logging and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiPaintStats {
    /// Batches that recorded a draw.
    pub batches_drawn: u32,
    /// Batches skipped (empty triangle list, or texture with no live
    /// upload — never drawn with garbage).
    pub batches_skipped: u32,
    /// Triangles drawn across all batches.
    pub triangles_drawn: u32,
}

/// What one [`UiPainter::paint`] call recorded: the stats plus the
/// uploaded vertex buffers it drew from.
///
/// The caller holds this until after the submission that reads it —
/// dropping the buffers before submit is use-after-free, for the same
/// reason `canary-render-ecs`'s bridge documents.
pub struct UiPaintDraw<D: RenderDevice> {
    /// What the call drew and skipped.
    pub stats: UiPaintStats,
    /// Per-batch vertex uploads, in draw order. Held, not read: their
    /// presence keeps the GPU-side buffers alive across submit.
    pub buffers: Vec<D::Buffer>,
}

/// Submits [`UiPaint`] through any RHI backend: owns the blended textured
/// pipeline plus the live GPU textures named by [`UiTextureId`].
///
/// Created with [`UiPainter::new`] (pipeline compiled from caller-supplied
/// SPIR-V — the adapter stays `naga`-free; whatever compiles
/// [`UI_PAINT_WGSL`] is the consumer's job, mirroring how
/// `canary-render-ecs` owns its WGSL while consumers compile it).
/// Per frame the caller runs [`UiPainter::apply_texture_ops`] (before the
/// pass — uploads are resource creation, not recording) and then
/// [`UiPainter::paint`] inside its open render pass.
pub struct UiPainter<D: RenderDevice> {
    pipeline: D::Pipeline,
    textures: HashMap<UiTextureId, D::Texture>,
}

impl<D: RenderDevice> UiPainter<D> {
    /// Compiles the UI pipeline from precompiled SPIR-V (see
    /// [`UI_PAINT_WGSL`]) and starts with no live textures: the first
    /// frame's `Set` ops populate them.
    pub fn new(device: &D, vertex_shader_spirv: &[u32], fragment_shader_spirv: &[u32]) -> Self {
        let attributes = ui_vertex_attributes();
        let pipeline = device.create_blended_textured_pipeline(&PipelineDescriptor {
            label: "canary-ui paint pipeline",
            vertex_shader_spirv,
            vertex_entry_point: "vs_main",
            fragment_shader_spirv,
            fragment_entry_point: "fs_main",
            vertex_stride: ui_vertex_stride(),
            vertex_attributes: &attributes,
        });
        Self {
            pipeline,
            textures: HashMap::new(),
        }
    }

    /// Applies one frame's texture ops: `Set` (re)creates the GPU texture
    /// from the full upload — the RHI offers upload-once creation and no
    /// updates, so atlas changes re-create (deltas arrive only when glyphs
    /// change, typically the first frames); `Free` drops it. Unknown `Free`
    /// ids are ignored: `egui` never emits them, and refusing would turn a
    /// backend ordering quirk into a painter failure.
    pub fn apply_texture_ops(&mut self, device: &D, ops: &[UiTextureOp]) {
        for op in ops {
            match op {
                UiTextureOp::Set {
                    id,
                    width,
                    height,
                    pixels,
                } => {
                    // Empty uploads are a caller bug the backend reports
                    // loudly (like every other empty upload on the RHI) —
                    // the painter does not second-guess them here.
                    let texture = device.create_texture(&TextureDescriptor {
                        label: "canary-ui texture",
                        width: *width,
                        height: *height,
                        rgba8: pixels,
                    });
                    self.textures.insert(*id, texture);
                }
                UiTextureOp::Free { id } => {
                    self.textures.remove(id);
                }
            }
        }
    }

    /// Records `paint`'s batches into the caller's open render pass, in
    /// order (later batches draw over earlier ones — painter's algorithm,
    /// no depth). For each batch: look up the GPU texture (missing → skip,
    /// counted), set the scissor, upload the NDC-encoded triangles, draw.
    /// Batches with no triangles record nothing (a zero-size buffer
    /// creation is driver-risky) and count as skipped.
    ///
    /// Returns what was recorded plus the uploads to hold across submit.
    /// A no-op (zero stats, no recording) when `paint.is_empty()`.
    pub fn paint(
        &self,
        device: &D,
        encoder: &mut D::CommandEncoder<'_>,
        viewport: UiViewport,
        paint: &UiPaint,
    ) -> UiPaintDraw<D> {
        assert!(
            viewport.pixels_per_point > 0.0 && viewport.pixels_per_point.is_finite(),
            "UiViewport::pixels_per_point must be positive and finite, got {}",
            viewport.pixels_per_point
        );
        let mut stats = UiPaintStats::default();
        let mut buffers = Vec::with_capacity(paint.batches.len());
        for batch in &paint.batches {
            let Some(texture) = self.textures.get(&batch.texture) else {
                stats.batches_skipped += 1;
                continue;
            };
            if batch.triangles.is_empty() {
                stats.batches_skipped += 1;
                continue;
            }
            let floats = encode_batch(batch, viewport);
            let bytes: Vec<u8> = floats
                .iter()
                .flat_map(|vertex: &f32| vertex.to_ne_bytes())
                .collect();
            let buffer = device.create_buffer(&BufferDescriptor {
                label: "canary-ui paint batch",
                data: &bytes,
            });
            encoder.set_pipeline(&self.pipeline);
            encoder.set_texture(texture);
            encoder.set_scissor(clip_to_scissor(batch, viewport));
            encoder.set_vertex_buffer(&buffer);
            let triangles = u32::try_from(batch.triangles.len()).unwrap_or(u32::MAX);
            // Saturate, never wrap: a hostile batch length must degrade to
            // an oversized draw, not an arithmetic panic in library code.
            encoder.draw(triangles.saturating_mul(3));
            stats.batches_drawn += 1;
            stats.triangles_drawn = stats.triangles_drawn.saturating_add(triangles);
            buffers.push(buffer);
        }
        UiPaintDraw { stats, buffers }
    }
}

/// Encodes one batch's triangles as NDC clip-space floats: 8 floats per
/// vertex (position, UV, color), 3 vertices per triangle, batches in paint
/// order. Logical Y points down (viewport top-left origin); NDC Y points
/// up, so the Y axis flips. Pure (no GPU) so tests pin the mapping.
///
/// Public (like the layout helpers above) so consumers composing their
/// own submit path — and tests — share the one mapping.
pub fn encode_batch(batch: &UiDrawBatch, viewport: UiViewport) -> Vec<f32> {
    let width = viewport.logical_width();
    let height = viewport.logical_height();
    let mut floats = Vec::with_capacity(batch.triangles.len() * 3 * 8);
    for triangle in &batch.triangles {
        for vertex in &triangle.vertices {
            floats.push(vertex.position[0] / width * 2.0 - 1.0);
            floats.push(1.0 - vertex.position[1] / height * 2.0);
            floats.extend_from_slice(&vertex.uv);
            floats.extend_from_slice(&vertex.color);
        }
    }
    floats
}

/// Converts a batch's logical clip rect to a physical scissor: minimum
/// edges floor down, maximum edges ceil up (conservative — a partially
/// covered pixel still draws), negatives saturate to zero. Values are
/// clamped to the exactly-representable `u32` range before conversion, so
/// the cast is exact, never narrowing. Target-bounds clamping stays the
/// backend's job ([`canary_render::clamp_scissor`]); a fully-offscreen
/// rect becomes an empty scissor there, a legal no-op draw.
///
/// Public with [`encode_batch`]: one shared mapping for every submit path.
pub fn clip_to_scissor(batch: &UiDrawBatch, viewport: UiViewport) -> ScissorRect {
    const EXACT_RANGE: f32 = 16_777_216.0; // 2^24: every u32 below it converts exactly.
    let scale = viewport.pixels_per_point;
    let floor_px = |value: f32| (value * scale).floor().clamp(0.0, EXACT_RANGE) as u32;
    let ceil_px = |value: f32| (value * scale).ceil().clamp(0.0, EXACT_RANGE) as u32;
    let min_x = floor_px(batch.clip.min_x);
    let min_y = floor_px(batch.clip.min_y);
    ScissorRect {
        x: min_x,
        y: min_y,
        width: ceil_px(batch.clip.max_x).saturating_sub(min_x),
        height: ceil_px(batch.clip.max_y).saturating_sub(min_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_ui_core::{UiClipRect, UiTriangle, UiVertex};

    fn batch() -> UiDrawBatch {
        let vertex = UiVertex {
            position: [50.0, 25.0],
            uv: [0.5, 0.25],
            color: [1.0, 0.5, 0.25, 1.0],
        };
        UiDrawBatch {
            clip: UiClipRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 100.0,
                max_y: 50.0,
            },
            texture: UiTextureId::Managed(0),
            triangles: vec![UiTriangle {
                vertices: [vertex, vertex, vertex],
            }],
        }
    }

    fn viewport() -> UiViewport {
        UiViewport {
            target_width_px: 200,
            target_height_px: 100,
            pixels_per_point: 2.0,
        }
    }

    #[test]
    fn stride_is_thirty_two_bytes() {
        // 8-byte position + 8-byte UV + 16-byte RGBA color.
        assert_eq!(ui_vertex_stride(), 32);
    }

    #[test]
    fn ui_attributes_match_layout() {
        let attributes = ui_vertex_attributes();
        assert_eq!(attributes.len(), 3);
        assert_eq!(attributes[0].shader_location, 0);
        assert_eq!(attributes[0].format, VertexFormat::Float32x2);
        assert_eq!(attributes[0].offset, 0);
        assert_eq!(attributes[1].shader_location, 1);
        assert_eq!(attributes[1].format, VertexFormat::Float32x2);
        assert_eq!(attributes[1].offset, VertexFormat::Float32x2.size_bytes());
        assert_eq!(attributes[2].shader_location, 2);
        assert_eq!(attributes[2].format, VertexFormat::Float32x4);
        assert_eq!(
            attributes[2].offset,
            VertexFormat::Float32x2.size_bytes() * 2
        );
    }

    #[test]
    fn encode_maps_logical_pixels_to_ndc_with_y_flip() {
        // Viewport is 200x100 physical at 2.0 ppp: 100x50 logical.
        // Vertex at logical (50, 25) — the exact center — must encode to
        // NDC (0, 0); UV and color pass through untouched.
        let floats = encode_batch(&batch(), viewport());
        assert_eq!(floats.len(), 3 * 8);
        assert_eq!(&floats[0..8], &[0.0, 0.0, 0.5, 0.25, 1.0, 0.5, 0.25, 1.0]);
    }

    #[test]
    fn encode_maps_corners_to_clip_edges() {
        let corner = |x: f32, y: f32| UiVertex {
            position: [x, y],
            uv: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
        };
        let batch = UiDrawBatch {
            clip: UiClipRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 100.0,
                max_y: 50.0,
            },
            texture: UiTextureId::Managed(0),
            triangles: vec![UiTriangle {
                vertices: [corner(0.0, 0.0), corner(100.0, 0.0), corner(0.0, 50.0)],
            }],
        };
        let floats = encode_batch(&batch, viewport());
        // Top-left logical (0,0) is NDC (-1, 1); top-right (100,0) is
        // (1, 1); bottom-left (0,50) is (-1, -1).
        assert_eq!(&floats[0..2], &[-1.0, 1.0]);
        assert_eq!(&floats[8..10], &[1.0, 1.0]);
        assert_eq!(&floats[16..18], &[-1.0, -1.0]);
    }

    #[test]
    fn clip_scales_by_pixels_per_point() {
        // Logical 0..100 x 0..50 at 2.0 ppp: physical 0..200 x 0..100.
        assert_eq!(
            clip_to_scissor(&batch(), viewport()),
            ScissorRect {
                x: 0,
                y: 0,
                width: 200,
                height: 100,
            }
        );
    }

    #[test]
    fn clip_rounds_conservatively_and_saturates_negatives() {
        let batch = UiDrawBatch {
            clip: UiClipRect {
                min_x: -5.5,
                min_y: 10.2,
                max_x: 20.2,
                max_y: 30.7,
            },
            texture: UiTextureId::Managed(0),
            triangles: Vec::new(),
        };
        // At 2.0 ppp: min floors to (-11, 20.4) → saturates to (0, 20);
        // max ceils to (40.4, 61.4) → (41, 62); size is the difference.
        assert_eq!(
            clip_to_scissor(&batch, viewport()),
            ScissorRect {
                x: 0,
                y: 20,
                width: 41,
                height: 42,
            }
        );
    }
}
