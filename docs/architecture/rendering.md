# Rendering

No rendering code exists in v0.0.1 — this is architecture for Era 3 (see
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md)),
written now so the decision is reasoned about once rather than improvised
under deadline pressure later. The concrete backend decision is recorded in
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md);
this document covers the fuller design.

## Two layers: RHI and render graph

Rendering is split into two layers that must not be conflated:

1. **Render Hardware Interface (RHI)** — a thin, explicit abstraction over
   GPU concepts (devices, buffers, textures, pipelines, command encoding)
   modeled closely on modern explicit APIs (Vulkan/Metal/DX12/WebGPU), not on
   older fixed-function-flavored APIs. This is the *only* layer allowed to
   know which concrete graphics API is in use.
2. **Render graph / high-level renderer** — passes, resource dependencies
   (this pass reads what that pass wrote), materials, and the extract step
   from ECS state (see
   [engine-overview.md](engine-overview.md#how-a-frame-is-expected-to-flow-target-design-post-era-2)).
   This layer depends only on the RHI trait, never on a concrete backend
   directly.

The reason for the split, not just "because layering is generally good": it
is what makes the RHI backend genuinely replaceable (per the "replaceable
engine subsystems" goal) without touching the render graph, materials, or
any gameplay-facing rendering API — a new RHI implementation is a new crate
satisfying an existing trait.

## Backend choice: bootstrap on wgpu, architect for a custom RHI later

`wgpu` — a mature, actively developed, pure-Rust graphics library targeting
Vulkan, Metal, DirectX 12, and OpenGL natively, plus WebGPU/WebGL2 on
WebAssembly — is the initial RHI implementation. It already backs Firefox's,
Servo's, and Deno's WebGPU implementations, and multiple Rust engines
(Bevy, Fyrox, rend3) build on it in production. Reimplementing that surface
area before there's a renderer to justify it would be exactly the kind of
premature, unjustified effort this foundation is trying to avoid.

The RHI **trait boundary is designed so a fully custom RHI (Vulkan-first,
with direct access to features wgpu doesn't yet expose — mesh shaders,
hardware ray tracing extensions, console graphics APIs) can replace the
wgpu-backed implementation later without the render graph or any
gameplay-facing code changing.** This is the concrete instance of "bootstrap
pragmatically, architect for replacement" that recurs across this
foundation's decisions. See
[`docs/research/technology-evaluations.md`](../research/technology-evaluations.md)
for the sourcing on wgpu's current capabilities and known gaps (multi-GPU,
hardware ray-tracing extensions, and console NDA'd APIs are the main ones as
of this writing).

## Render graph responsibilities

- **Pass declaration**: each render pass declares what it reads and writes
  (textures, buffers), so the graph can order passes automatically and
  detect resource conflicts at graph-build time rather than as a runtime GPU
  validation error.
- **Transient resource management**: intermediate textures/buffers that only
  exist within a frame are pooled/aliased by the graph, not manually
  allocated and freed by each pass author.
- **Extract, don't query**: the graph consumes a read-only snapshot of
  ECS-visible render state (transforms, visible meshes, materials) prepared
  by an explicit extract step, rather than passes reaching back into the
  live ECS `World` mid-frame. This is what allows the render graph to
  execute on a different thread, and eventually a frame behind simulation,
  without a redesign.

## Materials & shaders

Target design uses WGSL (WebGPU Shading Language) as the primary shader
authoring language, since it's what `wgpu`/`naga` natively understand and
translate to SPIR-V/HLSL/MSL as needed — avoiding a second, engine-specific
shading language to maintain. Material definitions are data (not code) where
possible, describing which shader variant and which parameters apply, so
that non-programmer contributors and future editor tooling
(`docs/ui/editor-design.md`) can author materials without touching Rust.

## 2D is a specialization of this architecture, not a separate one

Per [`docs/vision/project-goals.md`](../vision/project-goals.md#2d-and-3d-games-and-beyond),
Canary is not a 3D engine that 2D games have to work around. Concretely,
that means the render graph and RHI described above are designed so 2D
rendering — an orthographic camera, sprite batching, tilemap rendering —
is a *configuration* of the same pipeline (a render pass reading sprite
data and an orthographic projection, batched for the same GPU submission
model everything else uses), not a fork of the renderer or a separate
code path maintained in parallel. Physics mirrors this precisely at the
subsystem level: [`docs/architecture/physics.md`](physics.md)'s default
backend ships genuinely separate 2D and 3D crates rather than one 3D
system 2D games route around — rendering's approach (one architecture,
2D as a specialization) and physics's approach (two real
implementations, one per dimensionality) look different because they're
solving different problems, but both exist specifically so 2D is never
the afterthought.

## What's explicitly out of scope for the foreseeable future

- Software/CPU rendering fallback — not a goal; if hardware acceleration is
  unavailable, that's a platform-support gap to document, not a renderer
  mode to build.
- Bespoke console-NDA graphics API backends — gated on the same
  "motivated backer or community" condition as console support generally
  (see [`docs/vision/project-goals.md`](../vision/project-goals.md#non-goals-for-now)).
- Competing with Unreal's Nanite/Lumen on day one. The layered RHI/render-
  graph design is what gives Canary a credible *path* to AAA-grade rendering
  without requiring it be there from the start.
