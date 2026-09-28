// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Descriptor types for [`crate::RenderDevice`]'s resource-creation
//! methods — plain data, no backend types, so creating one of these
//! never requires depending on a concrete backend crate.

/// Describes a GPU buffer to create, with its initial (and, for
/// `v0.0.6`'s scope, only) content.
#[derive(Debug, Clone, Copy)]
pub struct BufferDescriptor<'a> {
    /// A human-readable label, surfaced in backend debug tooling
    /// (Vulkan validation-layer object names, RenderDoc captures, ...)
    /// where the backend supports it. Never required for correctness.
    pub label: &'a str,
    /// The buffer's initial content, uploaded at creation time. `v0.0.6`
    /// has no story for updating a buffer's content after creation —
    /// real, likely future work once something other than a single
    /// static triangle needs it.
    ///
    /// Must be non-empty: backends reject an empty upload loudly
    /// (rather than creating a zero-byte GPU allocation) at creation
    /// time.
    pub data: &'a [u8],
}

/// Describes a GPU texture to create, with its full initial (and, for
/// this release's scope, only) content.
///
/// # Why this slice, and what is deferred
///
/// This is the minimal texture half of Phase 3b's bounded RHI addition:
/// one RGBA8 image, dimensions plus bytes, uploaded once at creation —
/// the same write-once discipline [`BufferDescriptor`] documents for
/// vertex data. There is deliberately no sampler choice (the backend
/// binds its one default), no mipmap chain (exactly one level), no
/// sRGB transfer-function handling (bytes are sampled as-is), no
/// texture arrays, and no second texture slot. All of those belong to
/// the general materials system — a real, intended future design that
/// gets built when a second consumer needs more than one texture, not
/// speculatively here against a single quadrant fixture.
#[derive(Debug, Clone, Copy)]
pub struct TextureDescriptor<'a> {
    /// A human-readable label, surfaced in backend debug tooling
    /// (Vulkan validation-layer object names, RenderDoc captures, ...)
    /// where the backend supports it. Never required for correctness.
    pub label: &'a str,
    /// Width in pixels. Must be greater than zero.
    pub width: u32,
    /// Height in pixels. Must be greater than zero.
    pub height: u32,
    /// The texture's texels as tightly packed 8-bit RGBA, row-major from
    /// the top row: exactly `width * height * 4` bytes. This is the same
    /// layout [`canary_assets`](https://github.com/HylightGames/canary)'s
    /// `Texture::rgba8` produces, so upload stays a `memcpy` with no
    /// reshuffling at the graphics boundary.
    pub rgba8: &'a [u8],
}

/// Describes an offscreen color render target to create.
#[derive(Debug, Clone, Copy)]
pub struct ColorTargetDescriptor {
    /// Width in pixels. Must be greater than zero.
    pub width: u32,
    /// Height in pixels. Must be greater than zero.
    pub height: u32,
}

/// One vertex attribute's format — deliberately just the two shapes
/// `v0.0.6`'s hello-triangle vertex (a 2D position plus an RGB color)
/// needs. More (integer formats, normalized formats, 4-component
/// formats, ...) is real, expected future work once a real vertex needs
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    /// Two 32-bit floats (8 bytes total) — e.g. a 2D position, or (since
    /// Phase 3b's texture slice) a 2D texture coordinate: a UV pair needs
    /// no new format, so the textured vertex layout reuses this variant
    /// rather than widening the enum for one consumer.
    Float32x2,
    /// Three 32-bit floats (12 bytes total) — e.g. an RGB color.
    Float32x3,
    /// Four 32-bit floats (16 bytes total) — e.g. an RGBA color.
    /// Added for the `.13` UI slice: `egui` vertices carry an sRGB
    /// color beside position and UV, and this trait has no normalized
    /// integer formats, so the UI adapter expands each color channel
    /// to a float on the host. Gamma handling stays deferred with the
    /// materials system (same stance as [`TextureDescriptor`]'s "no
    /// sRGB transfer-function handling"): bytes are normalized as-is.
    Float32x4,
}

impl VertexFormat {
    /// The size of this format in bytes.
    pub const fn size_bytes(self) -> u32 {
        match self {
            VertexFormat::Float32x2 => 8,
            VertexFormat::Float32x3 => 12,
            VertexFormat::Float32x4 => 16,
        }
    }
}

/// One vertex attribute's shader binding location and layout within a
/// vertex buffer's per-vertex stride.
#[derive(Debug, Clone, Copy)]
pub struct VertexAttribute {
    /// The `@location(N)` this attribute binds to in the vertex shader.
    pub shader_location: u32,
    /// This attribute's format.
    pub format: VertexFormat,
    /// This attribute's byte offset within one vertex's data.
    pub offset: u32,
}

/// Describes a graphics pipeline to create from precompiled SPIR-V.
/// `v0.0.6`'s scope: exactly one vertex-buffer binding (no multiple
/// buffer slots, no instance-rate attributes), no descriptor
/// sets/uniforms, no blending or depth/stencil state — a real, minimal
/// hello-triangle pipeline, not a general one.
#[derive(Debug, Clone, Copy)]
pub struct PipelineDescriptor<'a> {
    /// A human-readable label; see [`BufferDescriptor::label`]'s docs
    /// for the same caveat.
    pub label: &'a str,
    /// The vertex shader stage, as SPIR-V words — produced by
    /// cross-compiling WGSL via standalone `naga`, per
    /// `docs/architecture/rendering.md`'s "Materials & shaders".
    pub vertex_shader_spirv: &'a [u32],
    /// The vertex shader's entry point function name.
    pub vertex_entry_point: &'a str,
    /// The fragment shader stage, as SPIR-V words.
    pub fragment_shader_spirv: &'a [u32],
    /// The fragment shader's entry point function name.
    pub fragment_entry_point: &'a str,
    /// The byte stride between consecutive vertices in the bound vertex
    /// buffer.
    pub vertex_stride: u32,
    /// This pipeline's vertex attributes.
    pub vertex_attributes: &'a [VertexAttribute],
}

/// Describes how to begin a render pass.
#[derive(Debug, Clone, Copy)]
pub struct RenderPassDescriptor {
    /// The color the target is cleared to before any drawing, as linear
    /// RGBA in `[0.0, 1.0]`.
    pub clear_color: [f32; 4],
}

/// A scissor rectangle for [`crate::CommandEncoder::set_scissor`]:
/// pixel units, top-left origin, relative to the current render
/// target's extent. Added for the `.13` UI slice (`egui` clip rects);
/// backends clamp it against the open target (see
/// [`clamp_scissor`]) so drivers never see an out-of-bounds rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScissorRect {
    /// Left edge in pixels from the target's left edge.
    pub x: u32,
    /// Top edge in pixels from the target's top edge.
    pub y: u32,
    /// Width in pixels. Zero means "draws nothing".
    pub width: u32,
    /// Height in pixels. Zero means "draws nothing".
    pub height: u32,
}

/// Clamps `rect` to a `target_width` × `target_height` target,
/// returning `None` when nothing of the rect survives (fully
/// outside, or zero area). Pure and backend-neutral so every backend
/// applies the same rule: drivers must never receive an
/// out-of-bounds scissor, and callers can skip draws that clamp to
/// nothing without recording no-op work.
pub const fn clamp_scissor(
    target_width: u32,
    target_height: u32,
    rect: ScissorRect,
) -> Option<ScissorRect> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    if rect.x >= target_width || rect.y >= target_height {
        return None;
    }
    let max_width = target_width - rect.x;
    let max_height = target_height - rect.y;
    let width = if rect.width < max_width {
        rect.width
    } else {
        max_width
    };
    let height = if rect.height < max_height {
        rect.height
    } else {
        max_height
    };
    Some(ScissorRect {
        x: rect.x,
        y: rect.y,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_format_sizes_match_their_documented_byte_widths() {
        assert_eq!(VertexFormat::Float32x2.size_bytes(), 8);
        assert_eq!(VertexFormat::Float32x3.size_bytes(), 12);
        assert_eq!(VertexFormat::Float32x4.size_bytes(), 16);
    }

    #[test]
    fn scissor_rect_fully_inside_target_is_unchanged() {
        let rect = ScissorRect {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        assert_eq!(clamp_scissor(320, 240, rect), Some(rect));
    }

    #[test]
    fn scissor_rect_partially_outside_is_clamped_to_target() {
        let rect = ScissorRect {
            x: 300,
            y: 220,
            width: 100,
            height: 100,
        };
        assert_eq!(
            clamp_scissor(320, 240, rect),
            Some(ScissorRect {
                x: 300,
                y: 220,
                width: 20,
                height: 20,
            })
        );
    }

    #[test]
    fn scissor_rect_fully_outside_yields_none() {
        let right = ScissorRect {
            x: 320,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(clamp_scissor(320, 240, right), None);
        let below = ScissorRect {
            x: 0,
            y: 240,
            width: 10,
            height: 10,
        };
        assert_eq!(clamp_scissor(320, 240, below), None);
    }

    #[test]
    fn scissor_rect_with_zero_area_yields_none() {
        let flat = ScissorRect {
            x: 10,
            y: 10,
            width: 0,
            height: 50,
        };
        assert_eq!(clamp_scissor(320, 240, flat), None);
    }
}
