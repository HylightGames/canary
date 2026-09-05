# Rendering

No rendering code exists in v0.0.1 — this is architecture for Era 3 (see
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md)),
written now so the decision is reasoned about once rather than improvised
under deadline pressure later. The RHI/render-graph split is recorded in
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md);
the concrete backend-implementation decision is in
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md).
This document covers the fuller design.

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

## Backend choice: native per-graphics-API crates, no bootstrap dependency

Each graphics API gets its own crate implementing the RHI trait
directly against that API's native bindings — no single third-party
abstraction library (`wgpu` or otherwise) as a required intermediary.
See [ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md)
for the full reasoning and what was verified before committing to this;
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md)
covers why the RHI/render-graph split itself exists (unchanged by
ADR 0016).

**`canary-render-vulkan`** (via [`ash`](https://github.com/ash-rs/ash))
is first: Vulkan alone covers Linux, Windows, and Android in one
backend, and `ash` needed zero version pins against this project's
rustc-1.75 floor — the cleanest dependency check run so far. **A real
Vulkan device is confirmed enumerable in this project's sandbox**
(`llvmpipe`, Mesa's software rasterizer, via `mesa-vulkan-drivers`),
meaning this backend can be built *and tested* in CI-like environments
without real GPU hardware, not merely compiled against.

**`canary-render-metal`** (native Apple support, avoiding a
Vulkan-to-Metal translation layer like MoltenVK), **`canary-render-dx12`**,
and **`canary-render-gl`** (OpenGL 4.x core plus WebGL2/GLES, for older
hardware and web targets) follow the same pattern — real, intended, and
explicitly not sequenced on a timeline yet, per
[`future-roadmap.md`](../roadmap/future-roadmap.md)'s "don't assign fake
specificity" discipline. None of these four is structurally privileged:
each is a separate crate behind the same trait, opt-in via a Cargo
feature on `canary-render`, so a game build only compiles and links the
backend(s) it actually enables — the same "no privileged built-ins"
guarantee [`physics.md`](physics.md)'s `PhysicsBackend` trait already
gives, applied here to rendering. A backend crate's public surface must
never leak its native API's types (`ash::vk::*`, etc.) past the RHI
trait boundary, mirroring the existing rule against physics backends
leaking third-party types.

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
authoring language, cross-compiled to each native backend's expected
representation (SPIR-V for Vulkan, MSL for Metal, HLSL for DirectX 12,
GLSL for OpenGL/WebGL) via [`naga`](https://github.com/gfx-rs/wgpu/tree/trunk/naga)
used standalone — a separate, independent crate from `wgpu` (confirmed
directly, not assumed; see
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md)),
so this doesn't require `wgpu` itself as a dependency. One shading
language across every backend, rather than a second, engine-specific one
to maintain. Material definitions are data (not code) where possible,
describing which shader variant and which parameters apply, so that
non-programmer contributors and future editor tooling
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
