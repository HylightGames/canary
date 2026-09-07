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
    pub data: &'a [u8],
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
    /// Two 32-bit floats (8 bytes total) — e.g. a 2D position.
    Float32x2,
    /// Three 32-bit floats (12 bytes total) — e.g. an RGB color.
    Float32x3,
}

impl VertexFormat {
    /// The size of this format in bytes.
    pub const fn size_bytes(self) -> u32 {
        match self {
            VertexFormat::Float32x2 => 8,
            VertexFormat::Float32x3 => 12,
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
