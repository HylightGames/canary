//! GPU draw path: the proven shader, its vertex layout, and the one-buffer-one-draw call.
//!
//! This module is the last third of the bridge: it takes a baked
//! [`BakedFrame`](crate::extract::BakedFrame) and issues it to the GPU through
//! the existing RHI. Nothing here invents GPU behavior — the shader, the
//! attribute layout, and the record pattern (one pass, one buffer, one draw,
//! `submit_and_wait`) are all reused verbatim from `canary-render-vulkan`'s
//! `hello_triangle` integration test, which already proves them against real
//! pixels. A new shader here would be unproven scope; a new layout would
//! silently disagree with the baked floats.
//!
//! # Zero RHI churn, by construction
//!
//! This module only *calls* the [`RenderDevice`](canary_render::RenderDevice)
//! trait — `create_buffer`, `create_command_encoder`, and (via the encoder)
//! `begin_render_pass` / `set_pipeline` / `set_vertex_buffer` / `draw` /
//! `end_render_pass`, then `submit_and_wait`. It declares no new trait
//! methods, no new descriptor fields, no uniforms, no push constants, no
//! index buffers, no depth state. Per-frame animation works despite the
//! write-once buffer model by creating one fresh buffer per frame from the
//! freshly baked bytes — the same approach `examples/spinning-cube` uses,
//! and the correct one given that [`BufferDescriptor`](canary_render::BufferDescriptor)
//! documents "no story for updating a buffer's content after creation".

use canary_render::{
    BufferDescriptor, CommandEncoder, RenderDevice, RenderPassDescriptor, VertexAttribute,
    VertexFormat,
};

use crate::extract::BakedFrame;

/// The vertex shader authoring source the bridge draws every frame.
///
/// Byte-identical to `WGSL_SOURCE` in
/// `engine/canary-render-vulkan/tests/hello_triangle.rs`: a trivial
/// 2D-position + RGB-color passthrough (`input.position` is already in clip
/// space because all real transforms happen in the CPU bake; see
/// [`crate::extract`]). Reusing the proven shader instead of writing a new
/// one is load-bearing — `hello_triangle` already asserts real rasterized
/// pixels for exactly this source, so the bridge inherits that proof rather
/// than trusting a fresh string. Any edit here must be mirrored there (and
/// re-proven by that test) or the two silently diverge; a `diff` of the two
/// raw-string bodies is the acceptance check.
pub const RENDER_WGSL: &str = r#"
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

/// Clear color used for every bridge render pass: opaque black.
///
/// Fixed (not a parameter) because the RHI's
/// [`RenderPassDescriptor`](canary_render::RenderPassDescriptor) carries the
/// clear color per pass and the bridge owns its pass: threading a
/// caller-supplied clear color through [`draw_baked_frame`] would be a new
/// API choice with no consumer yet (Task 5's pixel tests assert against this
/// exact black). Matches `hello_triangle`'s `CLEAR_COLOR`. A configurable
/// clear (or per-scene background) is v0.0.10+ scope.
pub const DEFAULT_CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The vertex attribute layout the bridge uploads: NDC position then color.
///
/// Location 0 is [`Float32x2`](canary_render::VertexFormat::Float32x2) at byte
/// offset 0 (the baked `x, y`); location 1 is
/// [`Float32x3`](canary_render::VertexFormat::Float32x3) at byte offset 8 (the
/// baked `r, g, b`). This mirrors `hello_triangle`'s `vertex_attributes`
/// entry-for-entry — including deriving the color offset from
/// [`VertexFormat::Float32x2::size_bytes`](canary_render::VertexFormat::size_bytes)
/// rather than hard-coding `8`, so a future format-size change fails loudly
/// at the layout site instead of silently misaligning attributes. Offsets are
/// verified without a GPU by `attributes_match_hello_triangle_layout`.
pub fn render_vertex_attributes() -> [VertexAttribute; 2] {
    [
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
    ]
}

/// The byte stride between consecutive baked vertices: 20.
///
/// `8` (position [`Float32x2`](canary_render::VertexFormat::Float32x2)) + `12`
/// (color [`Float32x3`](canary_render::VertexFormat::Float32x3)) — one baked
/// vertex is exactly 5 floats. Derived from the same `size_bytes()` calls as
/// [`render_vertex_attributes`] (not a magic `20`) so stride and layout share
/// one source of truth; `vertex_stride_is_twenty_bytes` pins the numeric
/// value the shader's `vec2 + vec3` input requires.
pub fn render_vertex_stride() -> u32 {
    VertexFormat::Float32x2.size_bytes() + VertexFormat::Float32x3.size_bytes()
}

/// Draws one baked frame: fresh buffer upload, one render pass, one draw.
///
/// Records `begin_render_pass` (clearing to [`DEFAULT_CLEAR_COLOR`]) →
/// `set_pipeline` → `set_vertex_buffer` → `draw(vertex_count)` →
/// `end_render_pass`, then [`submit_and_wait`](canary_render::RenderDevice::submit_and_wait)
/// — the same single-pass, single-draw shape `hello_triangle` proves. The
/// generic `D: RenderDevice` keeps this backend-agnostic: the bridge never
/// names a concrete device type, so no backend crate leaks above the RHI.
///
/// Two deliberate behaviors worth knowing:
/// - **Safe byte upload.** The `f32` slice becomes bytes via
///   `flat_map(to_ne_bytes)` — fully safe, no `unsafe`. Spinning-cube's
///   `from_raw_parts` reinterpretation is forbidden here: it needs a
///   `SAFETY` justification and a review flag for zero benefit (one small
///   per-frame allocation either way).
/// - **Empty frames still clear.** When `frame` is empty there is no buffer
///   to bind and nothing to draw (a zero-size buffer creation is
///   driver-risky, and drawing zero vertices proves nothing), but the pass
///   is still begun, ended, and submitted — so the target shows the clear
///   color instead of stale contents from a previous frame. The vertex
///   buffer binding is held in an `Option` outside the pass precisely so it
///   outlives `submit_and_wait`: destroying a buffer the GPU has not
///   finished reading is use-after-free, and synchronous submit is what
///   makes this ordering sound.
pub fn draw_baked_frame<D: RenderDevice>(
    device: &D,
    target: &D::ColorTarget,
    pipeline: &D::Pipeline,
    frame: &BakedFrame,
) {
    let vertex_buffer = if frame.is_empty() {
        None
    } else {
        let vertex_bytes: Vec<u8> = frame
            .vertices
            .iter()
            .flat_map(|vertex: &f32| vertex.to_ne_bytes())
            .collect();
        Some(device.create_buffer(&BufferDescriptor {
            label: "render-ecs baked frame",
            data: &vertex_bytes,
        }))
    };

    let mut encoder = device.create_command_encoder();
    encoder.begin_render_pass(
        target,
        &RenderPassDescriptor {
            clear_color: DEFAULT_CLEAR_COLOR,
        },
    );
    if let Some(buffer) = vertex_buffer.as_ref() {
        encoder.set_pipeline(pipeline);
        encoder.set_vertex_buffer(buffer);
        encoder.draw(frame.vertex_count());
    }
    encoder.end_render_pass();
    device.submit_and_wait(encoder);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_stride_is_twenty_bytes() {
        // 8-byte Float32x2 position + 12-byte Float32x3 color: the exact
        // stride hello_triangle's pipeline descriptor passes.
        assert_eq!(render_vertex_stride(), 20);
        assert_eq!(
            render_vertex_stride(),
            VertexFormat::Float32x2.size_bytes() + VertexFormat::Float32x3.size_bytes()
        );
    }

    #[test]
    fn attributes_match_hello_triangle_layout() {
        let attributes = render_vertex_attributes();

        // Location 0: 2D position at offset 0.
        assert_eq!(attributes[0].shader_location, 0);
        assert_eq!(attributes[0].format, VertexFormat::Float32x2);
        assert_eq!(attributes[0].offset, 0);
        // Location 1: RGB color immediately after the position (offset 8).
        assert_eq!(attributes[1].shader_location, 1);
        assert_eq!(attributes[1].format, VertexFormat::Float32x3);
        assert_eq!(attributes[1].offset, VertexFormat::Float32x2.size_bytes());
        // Layout/stride coherence: the color attribute must end exactly
        // where the stride ends, or vertices would overlap or gap.
        assert_eq!(
            attributes[1].offset + VertexFormat::Float32x3.size_bytes(),
            render_vertex_stride()
        );
    }
}
