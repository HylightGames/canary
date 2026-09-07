// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine's Render Hardware Interface (RHI): a thin, explicit
//! abstraction over GPU concepts (devices, buffers, pipelines, command
//! encoding), modeled on modern explicit graphics APIs — not on older
//! fixed-function-flavored ones. This is the *only* layer allowed to
//! know which concrete graphics API is in use; a render graph (real,
//! intended, not yet built — see `docs/architecture/rendering.md`)
//! would depend only on the trait here, never on a concrete backend
//! crate directly.
//!
//! See `docs/architecture/rendering.md`,
//! [ADR 0004](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md)
//! (the RHI/render-graph split), and
//! [ADR 0016](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0016-native-rendering-backends.md)
//! (native per-graphics-API backend crates, `canary-render-vulkan`
//! first) for the full design.
//!
//! **Deliberately minimal for `v0.0.6`** (see
//! `docs/roadmap/v0.0.6-roadmap.md`): exactly enough to create a vertex
//! buffer, an offscreen color target, a pipeline from precompiled SPIR-V,
//! record one render pass with one draw call, and read the result back —
//! not a speculatively complete GPU abstraction. Widening this trait as
//! later backends or a real render graph need more of it (textures with
//! sampling, depth/stencil, multiple draw calls, descriptor sets, ...) is
//! expected, not a sign this first cut was wrong.

mod types;

pub use types::{
    BufferDescriptor, ColorTargetDescriptor, PipelineDescriptor, RenderPassDescriptor,
    VertexAttribute, VertexFormat,
};

/// A GPU device capable of creating the resources this trait's other
/// associated types represent, and submitting recorded command work to
/// the GPU. The entry point into the RHI — a concrete backend (e.g.
/// `canary-render-vulkan`'s `VulkanDevice`) implements this directly;
/// nothing above this trait is allowed to reference that concrete type.
///
/// Resource creation here is infallible at the trait level, a
/// deliberate `v0.0.6` simplification, not an assumption that real GPU
/// allocation can't fail: a backend that hits a real allocation failure
/// panics for now, rather than this trait threading `Result` through
/// every creation method for a failure mode this release's own
/// milestone (one triangle, one tiny offscreen target, on a software
/// device) has no real chance of hitting. Fallible resource creation is
/// real, expected future work once something (a real render graph
/// juggling many resources, a memory-constrained target) actually needs
/// to recover from it rather than treat it as fatal.
pub trait RenderDevice {
    /// A GPU-resident buffer (used here for vertex data).
    type Buffer;
    /// A GPU-resident, offscreen color render target.
    type ColorTarget;
    /// A compiled graphics pipeline (shader stages + vertex layout).
    type Pipeline;
    /// Records GPU commands for one submission. See [`CommandEncoder`].
    type CommandEncoder<'a>: CommandEncoder<Self>
    where
        Self: 'a;

    /// Creates a GPU buffer, uploading `desc.data` as its initial (and,
    /// for `v0.0.6`'s scope, only) content.
    fn create_buffer(&self, desc: &BufferDescriptor<'_>) -> Self::Buffer;

    /// Creates an offscreen color render target — no swapchain, no
    /// window surface. Live window presentation is deliberately out of
    /// scope for `v0.0.6`; see `docs/roadmap/v0.0.6-roadmap.md`.
    fn create_color_target(&self, desc: &ColorTargetDescriptor) -> Self::ColorTarget;

    /// Creates a graphics pipeline from precompiled SPIR-V (see
    /// `docs/architecture/rendering.md`'s "Materials & shaders" for why
    /// SPIR-V specifically: WGSL is the authoring language, cross-
    /// compiled via standalone `naga`, not through this trait).
    fn create_pipeline(&self, desc: &PipelineDescriptor<'_>) -> Self::Pipeline;

    /// Begins recording a new command buffer.
    fn create_command_encoder(&self) -> Self::CommandEncoder<'_>;

    /// Submits recorded commands to the GPU and blocks until they've
    /// finished executing. `v0.0.6`'s scope has exactly one submission
    /// per frame and no reason yet to overlap CPU/GPU work across
    /// frames — an async, non-blocking submission model is real, likely
    /// future work once there's an actual render loop to benefit from
    /// it, not something this first cut needs to get right yet.
    fn submit_and_wait(&self, encoder: Self::CommandEncoder<'_>);

    /// Reads `target`'s current contents back to host memory as tightly
    /// packed 8-bit RGBA (`target.width * target.height * 4` bytes,
    /// row-major, no padding). Exists so `v0.0.6`'s hello-triangle test
    /// can assert on real rendered pixels rather than trust that
    /// nothing panicked — see that release's own "the actual milestone"
    /// scope item.
    fn read_color_target_rgba8(&self, target: &Self::ColorTarget) -> Vec<u8>;
}

/// Records GPU commands for one submission to a [`RenderDevice`] of type
/// `D`. Deliberately minimal: one render pass, one pipeline, one vertex
/// buffer, one draw call — see [`RenderDevice`]'s own docs for why.
pub trait CommandEncoder<D: RenderDevice + ?Sized> {
    /// Begins a render pass targeting `target`, clearing it to
    /// `desc.clear_color` first. `v0.0.6`'s scope is one render pass per
    /// encoder; nothing here prevents calling this again once
    /// `end_render_pass` is implemented to actually support that, but
    /// no backend is required to support it yet.
    fn begin_render_pass(&mut self, target: &D::ColorTarget, desc: &RenderPassDescriptor);

    /// Binds `pipeline` for subsequent draw calls in the current render
    /// pass.
    fn set_pipeline(&mut self, pipeline: &D::Pipeline);

    /// Binds `buffer` as the vertex buffer for subsequent draw calls in
    /// the current render pass. `v0.0.6`'s scope is exactly one vertex
    /// buffer, bound once — no multiple vertex-buffer slots, no index
    /// buffer yet.
    fn set_vertex_buffer(&mut self, buffer: &D::Buffer);

    /// Draws `vertex_count` vertices from the currently bound vertex
    /// buffer, using the currently bound pipeline. No instancing.
    fn draw(&mut self, vertex_count: u32);

    /// Ends the current render pass.
    fn end_render_pass(&mut self);
}
