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
    TextureDescriptor, VertexAttribute, VertexFormat,
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
    /// A GPU-resident sampled texture: RGBA8 texels uploaded once at
    /// creation, bound per draw via [`CommandEncoder::set_texture`].
    ///
    /// This is the texture half of Phase 3b's minimal slice — exactly
    /// one image, one default sampler, one level. The general materials
    /// system (sampler choice, mipmaps, arrays, multi-texture slots) is
    /// deferred, not partially present: nothing here names any of it.
    type Texture;
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

    /// Creates a GPU texture, uploading `desc.rgba8` as its initial
    /// (and, for this release's scope, only) content.
    ///
    /// # Why upload-once, and what is deferred
    ///
    /// The write-once discipline mirrors [`RenderDevice::create_buffer`]:
    /// there are no buffer/texture *updates* anywhere on this trait, so
    /// per-frame animation re-creates rather than mutates — the correct
    /// shape given a scope with no update API, not a performance claim.
    /// A texture cache, streaming uploads, and any sampler/mipmap/sRGB
    /// choice are the deferred materials system's work; this method
    /// takes plain bytes plus dimensions and promises sampling, nothing
    /// more.
    fn create_texture(&self, desc: &TextureDescriptor<'_>) -> Self::Texture;

    /// Creates a single-texture graphics pipeline from precompiled
    /// SPIR-V: identical to [`RenderDevice::create_pipeline`] except the
    /// pipeline layout additionally binds exactly one sampled texture
    /// (bound later via [`CommandEncoder::set_texture`]).
    ///
    /// # Why a second method instead of a descriptor flag
    ///
    /// Adding a `sampled_texture: bool` field to [`PipelineDescriptor`]
    /// would break every existing struct literal — all soup-path call
    /// sites, the `hello_triangle` proof, the spinning-cube example —
    /// for a flag most of them would set to `false`. A separate method
    /// keeps the addition purely additive: every existing pipeline
    /// construction compiles verbatim, and the textured path is visibly
    /// a second, bounded entry point rather than a mode flag threaded
    /// through the first one.
    ///
    /// # The contract the shader must uphold
    ///
    /// The fragment shader must declare exactly one sampled texture at
    /// set 0: binding 0 is the 2D sampled image, binding 1 is its
    /// sampler (two bindings, one texture — WGSL's `texture_2d` plus
    /// `sampler` compile to separate image/sampler descriptors, not to
    /// a single combined one, so the layout provides both). A shader
    /// declaring zero textures, two textures, or the same texture at a
    /// different set has no defined rendering under this method: that
    /// is the general materials system's scope, explicitly not this
    /// method's. UVs arrive as an ordinary vertex attribute — the
    /// existing [`VertexFormat::Float32x2`], not a new format — because
    /// a UV pair *is* two floats and needs no new enum variant.
    fn create_textured_pipeline(&self, desc: &PipelineDescriptor<'_>) -> Self::Pipeline;

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
    ///
    /// Draw-before-read contract: the target must have gone through at
    /// least one submitted render pass first. A freshly created target
    /// holds undefined contents, and copying from it is invalid even
    /// when a lenient driver appears to tolerate it — backends fail
    /// loudly in dev on violation (see `canary-render-vulkan`'s
    /// target-drawn tracking).
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

    /// Binds `texture` as the single sampled texture for subsequent draw
    /// calls in the current render pass.
    ///
    /// # Why exactly one texture, bound after the pipeline
    ///
    /// This is the sampling half of Phase 3b's minimal slice: one
    /// texture, no slot index, no sampler parameter — the pipeline
    /// created by [`RenderDevice::create_textured_pipeline`] already
    /// names the one layout this binds into. Multi-texture slots,
    /// per-draw material selection, and rebinding mid-pass are the
    /// deferred materials system's scope. Must be called after
    /// [`CommandEncoder::set_pipeline`]: the encoder binds into the
    /// currently bound pipeline's layout, and with no pipeline bound
    /// there is no layout to bind into (backends report that misuse
    /// loudly rather than recording undefined work).
    ///
    /// [`RenderDevice::create_textured_pipeline`]:
    ///     crate::RenderDevice::create_textured_pipeline
    fn set_texture(&mut self, texture: &D::Texture);

    /// Draws `vertex_count` vertices from the currently bound vertex
    /// buffer, using the currently bound pipeline. No instancing.
    fn draw(&mut self, vertex_count: u32);

    /// Ends the current render pass.
    fn end_render_pass(&mut self);
}
