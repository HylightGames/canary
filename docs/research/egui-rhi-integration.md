# egui → Canary RHI integration

Date: 2026-09-27. Milestone: v0.0.13 §4 (CanaryUI first-game slice).
Status: recommendation recorded here; no new ADR (backend choice already
decided by ADR 0011; the UI-render path through `canary-render` is
anticipated by `docs/architecture/ui-toolkit.md:169-178`).

## Decision

How `egui`'s renderer-agnostic output reaches the screen in v0.0.13:

1. `canary-ui-egui` (new adapter crate) drives `egui::Context` with normalized
   platform input (translated to `egui::RawInput`, positions in logical
   pixels to match the platform-slice convention).
2. Per frame it calls `Context::run(raw_input, closure) -> FullOutput`,
   then `ctx.tessellate(shapes, pixels_per_point) -> Vec<ClippedPrimitive>`.
3. Each `ClippedPrimitive { clip_rect, primitive }` becomes Canary-owned
   render data: the mesh (`Mesh::Primitive`) is de-indexed to a triangle
   soup on the CPU and submitted as textured triangles through the existing
   `canary-render` RHI trait (`CommandEncoder` + `set_scissor` for the clip
   rect, converted with `clamp_scissor` to pixel units).
4. `TexturesDelta` from `FullOutput` is applied to device textures before
   paint (set/apply first, free-after — the same ordering as the canonical
   `egui_glow` painter): full font-atlas re-upload on `set` deltas for v0.0.13
   (partial-update `pos` is ignored; correctness holds, efficiency deferred).
5. UI triangles are submitted through the **same** device/queue/frame as the
   scene and presented by the same `VulkanPresenter` path proven in §2.
   There is no UI-only graphics path and no second swapchain.

## Sources

- `egui` 0.36 `Context::run` / `tessellate` / `TexturesDelta` contract
  (current upstream docs at time of writing; re-verify signatures against
  the pinned version's docs.rs before implementing — step C).
- `egui_glow`'s `Painter` (the reference CPU-tessellation consumer):
  apply-`set`-before-paint, `free`-after-paint ordering.
- In-tree precedent: the mesh/soup render path already documents
  no-index-buffer support, so de-index-on-CPU matches an existing backend
  limitation rather than adding one.

## Alternatives considered

1. **A separate UI-only Vulkan path** (dedicated pipeline/swapchain for UI).
   Rejected: the roadmap explicitly forbids it — UI output must reach the
   same RHI path as the scene, or the "same frame" proof is meaningless.
2. **Community `egui-wgpu` integration.** Rejected: ADR 0016 bans wgpu from
   the tree; Canary renders `ash`-direct. The integration surface
   (textured triangles + scissor) is small enough that a thin Canary-owned
   consumer is less code than adapting a foreign backend.
3. **A custom native (non-egui) renderer for the first slice.** Rejected:
   deferred by the accepted design (`ui-toolkit.md:88-93`); immediate-mode
   HUD widgets are exactly what egui is for, and the backend-neutral
   boundary (`canary-ui-core`) keeps the swap option open.

## Tradeoffs accepted

- Full-atlas re-upload on font changes wastes upload bandwidth; fine for
  one HUD, revisit with partial updates when text becomes dynamic.
- CPU de-indexing duplicates index data per frame; matches the existing
  soup-path limitation, acceptable at HUD triangle counts.
- UI color arrives as `Float32x4` (see `VertexFormat` addition); gamma
  handling is deferred per the `TextureDescriptor` stance — HUD colors may
  differ slightly from a gamma-correct reference until that stance lands.
- `egui` version pin follows the standard caret + `Cargo.lock` policy
  (`build-system.md:115-147`); 0.36 is current at time of writing.

## What would change this decision

- `egui` removing or fundamentally changing the tessellate-to-meshes
  contract (check at each `egui` upgrade).
- A second UI backend arriving (e.g. `canary-ui-native`): the
  `canary-ui-core` trait boundary absorbs it; this note's egui-specific
  steps stay inside `canary-ui-egui`.
