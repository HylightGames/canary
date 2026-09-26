# Rendering

The first native Vulkan/offscreen rendering slice landed in `v0.0.6`,
with ECS extraction and file-loaded mesh/texture paths added in `v0.0.9`
and `v0.0.10`. This document records the implemented boundary and the
remaining presentation, render-graph, and material design. The
RHI/render-graph split is recorded in
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md);
the concrete backend-implementation decision is in
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md).
This document covers the fuller design.

**`v0.0.6` implemented the RHI trait's first real slice**: `canary-render`
(the trait itself) and `canary-render-vulkan` (the first backend, via
`ash`), proven with a real offscreen hello-triangle test — see
[`v0.0.6-roadmap.md`](../roadmap/v0.0.6-roadmap.md) for exactly what's
built versus still deferred (the render graph, materials, multiple draw
calls, window surfaces, and every backend beyond Vulkan remain future
work).

**`v0.0.9` wired real ECS state to that RHI** without changing it: a new
`canary-render-ecs` bridge crate extracts `GlobalTransform` + a flat
`Renderable` component out of the `World`, CPU-bakes one frame of NDC
vertices per tick, and draws it through the existing trait with one
fresh buffer plus one draw per frame. Zero RHI churn by construction
(the trait surface in `engine/canary-render/src/lib.rs` is untouched:
`create_buffer`, `create_color_target`, `create_pipeline`,
`create_command_encoder`, `submit_and_wait`, `read_color_target_rgba8`,
nothing else). Proven by three `#[ignore]`-gated offscreen pixel tests
plus the `spinning-cube` example rewritten on top of the bridge. Full
detail below; the deferred list at the end names what stays v0.0.10+
RHI work.

## `v0.0.9`: the ECS-to-render bridge (`canary-render-ecs`)

### Why a third crate

Problem: something has to own the `World` query, the projection math,
and the draw call, and it needs `canary-ecs`, `canary-transform`,
`glam`, and `canary-render` all at once. `canary-render` itself has
zero dependencies of any kind (checked mechanically with `cargo tree`,
not just claimed), and that property is what keeps every backend
independently optional: a game build compiles only the backend crates
it names. Giving `canary-render` an ECS or math dependency would end
that guarantee, and a dependency flowing the other way (a feature on
`canary-render` pulling in a backend) is a literal Cargo cycle, the
same reason ADR 0016's first draft had to be corrected during `v0.0.6`.

Chosen approach: the bridge lives in its own crate,
`engine/canary-render-ecs`, depending on `canary-ecs`,
`canary-scheduler`, `canary-transform`, `canary-render`, and `glam`
`0.33` (see `engine/canary-render-ecs/Cargo.toml`). Composition points
upward, per the usual direction: leaves know nothing about the bridge,
the bridge knows about them.

Rejected alternatives: putting the query and bake inside
`canary-render` (breaks the zero-dependency invariant above); putting
them inside the Vulkan backend (ties ECS-facing API to one graphics
API, defeating the replaceable-backend goal); growing a second
example-local implementation (that was the pre-bridge state, and two
competing copies of the same projection math is how they silently
diverge).

Consequences: the RHI trait never names an ECS type, the bridge never
names a concrete device type (`draw_baked_frame` is generic over
`D: RenderDevice`), and any future backend works with the bridge
unmodified. The price is one more crate in the workspace, which is
exactly what the one-crate-per-subsystem rule expects.

### `Renderable`: flat color, triangle soup, and why

Problem: the RHI dictates what per-entity data can possibly reach the
screen. At `v0.0.9` `VertexFormat` offered only `Float32x2` and
`Float32x3` (`engine/canary-render/src/lib.rs` via `types.rs`);
`CommandEncoder` binds exactly one vertex buffer ("no index buffer yet")
and `draw` consumes a plain `vertex_count` with no instancing. There
were no uniforms, no descriptor sets, no materials, and no textures
anywhere on the trait. A component carrying UVs, normals, material
handles, or indexed geometry would have had nothing to bind to.
(`v0.0.10` keeps this soup path verbatim and adds a single-texture
slice beside it — UVs reuse the existing `Float32x2`, no new format —
see the `v0.0.10` section below.)

Chosen approach (`engine/canary-render-ecs/src/renderable.rs`):
`pub struct Renderable { pub vertices: Vec<[f32; 3]>, pub color: [f32; 3] }`,
object-space triangles with `len % 3 == 0`, one flat RGB triple per
entity, plus `triangle_count()` and `is_valid()` (`len % 3 == 0`, empty
counts as valid with zero triangles). The flat color is replicated into
each baked vertex's `Float32x3` color attribute at bake time, which is
the only channel the RHI leaves open for telling two entities apart,
and per-entity colors are what let the pixel tests distinguish them.

Rejected alternatives: a `Mesh` resource plus material handles (needs
descriptor-backed data the RHI lacks; deferred to v0.0.10 asset work);
indexed geometry (no index binding exists); per-vertex colors (no
producer for them yet, and per-entity color is sufficient for every
test the bridge needs to pass).

Consequences: triangle soup is pre-expanded, so memory grows with face
count rather than vertex count, acceptable for a CPU-baked stage and
replaceable when index support lands. Construction stays infallible by
design (`new` stores as-is); the triangle invariant is enforced at
extract time, where invalid renderables are skipped, not panicked over,
since a half-built prefab is game-content trouble, not an engine
invariant violation. Emitting a partial tail would corrupt the whole
frame's vertex alignment past that point, so skipping is load-bearing,
not lenient.

### Extract, don't query: `BakedFrame` as the scheduled snapshot

Problem: the doctrine this document already states ("the graph consumes
a read-only snapshot of ECS-visible render state prepared by an
explicit extract step, rather than passes reaching back into the live
`World` mid-frame") needs a concrete mechanism, and the GPU must stay
out of the `Schedule`. A scheduled system's body is `FnMut(&World)` or
`FnMut(&mut World)`; capturing a `&VulkanDevice` inside that closure
ties the device borrow (owned by `main`'s frame scope) to the
schedule's registration lifetime, which is fragile by construction.

Chosen approach
(`engine/canary-render-ecs/src/extract.rs`,
`engine/canary-render-ecs/src/systems.rs`): `extract_scene` snapshots
the `World::query2::<GlobalTransform, Renderable>` intersection into
owned `RenderItem`s (world matrix copy plus cloned soup and color, no
borrow retained), the pure bake turns them into a `BakedFrame`
(`pub vertices: Vec<f32>`, NDC x/y plus flat r/g/b, 5 floats per
vertex), and a scheduled write system overwrites the `BakedFrame` ECS
resource every tick via `insert_resource` (at most one resource per
type, so overwrite semantics are total: no stale frame can survive a
tick). `draw_baked_frame` (`engine/canary-render-ecs/src/pipeline.rs`)
then runs explicitly after `schedule.run()` returns, with device,
target, and pipeline still owned by the calling binary. `BakedFrame`
carries plain floats, no device handles, no lifetimes, so nothing GPU
shaped ever crosses the scheduling boundary.

Rejected alternatives: a scheduled system closing over the device
(lifetime-fragile, and a direct violation of the extract doctrine);
reading the live `World` from the draw call (same violation, plus it
holds a borrow across submission); a second scheduler at the `App`
layer to order all of this (splits ordering authority across two
schedules with no cross-schedule conflict analysis; deferred to
v0.0.10+ as the plan always intended).

Consequences: the schedule never sees the GPU, the draw never sees the
`World`, and the bake step is a pure function testable without Vulkan.
When the RHI grows push constants or uniform buffers, the bake is what
gets replaced; the extract step stays. `BakedFrame` upholds a stride
alignment invariant (`vertices.len() % 5 == 0`, same `FLOATS_PER_VERTEX`
the draw derives its count from; the textured frame's own 4-float
stride likewise): the bakes guarantee it, the draws `debug_assert` it,
and hand-constructed frames must uphold it too — a misaligned tail
would make the draw under-read while the upload carries extra bytes.

### CPU-bake semantics: constants, Y-flip, painter sort, limits

Problem: with no per-frame transform path on the RHI (buffers are
write-once: `create_buffer` takes initial-content bytes and
`BufferDescriptor` documents no update story), the world matrix and the
camera projection have to happen on the CPU, once per frame, with the
result uploaded as a fresh buffer per frame. That bake must define a
camera, a projection, and an occlusion strategy from a trait that
provides none of them.

Chosen approach
(`engine/canary-render-ecs/src/extract.rs::bake_scene_to_vertices_with_aspect`):
each object-space vertex is transformed by its item's world matrix
(`GlobalTransform::matrix` plus `transform_point3`), shifted +3.2 on Z
into camera space (`CAMERA_DISTANCE`, camera modeled at the origin
looking down +Z), and perspective-projected with focal length 2.2
(`FOCAL_LENGTH`) and the caller-supplied aspect
(`x = (x * FOCAL) / (z * aspect)`, square targets use the neutral 1.0
default). Both constants are reused verbatim from the pre-bridge
spinning-cube's proven values. Y is negated because Vulkan NDC points
+Y down while object space is Y-up; without the negate, up renders as
down. Triangles are painter-sorted back-to-front by average
camera-space depth (`total_cmp` for a deterministic order on every
bit pattern) and emitted as `Float32x2` position plus `Float32x3`
color. Triangles at or behind the camera plane (depth at or below a
`1e-6` epsilon, so near-plane float noise cannot sneak an
astronomical-but-finite vertex past the guard) are skipped: their
projection divides by `z`, which is infinite at zero and mirrored
behind, so either outcome would poison the buffer with `inf`/`NaN`.
Finite camera-space inputs can still overflow the perspective divide
itself (e.g. huge x against a near-minimum z), so a post-projection
finiteness check skips those triangles whole — decided per triangle,
never per vertex, since a partial triangle would misalign the whole
soup. Non-finite `Renderable` colors and UVs likewise fail loudly in
dev (`debug_assert` at bake time; construction stays infallible by
design) rather than uploading undefined pixels.

Rejected alternatives: GPU-side transforms via uniforms or push
constants (no such trait method exists; inventing one is v0.0.10 RHI
work, not bridge work); a depth buffer (same, needs real RHI state the
Vulkan backend hard-codes `CullMode::NONE` against today); a camera
component with view matrices (no second consumer exists yet; a
half-camera now becomes compatibility surface the real one must honor
later, so the constants stay constants).

Consequences and explicit limits: the sort is exactly correct for
convex, non-self-intersecting shapes viewed from outside (a cube
qualifies) and insufficient for concave or interpenetrating geometry,
where per-pixel depth resolution is required. That needs a real depth
buffer, not a smarter sort. There is no camera component, no
configurable clear color (`DEFAULT_CLEAR_COLOR` is opaque black,
matching hello-triangle's, owned by the pass the bridge records), and
byte upload uses safe `flat_map(to_ne_bytes)`; the old example's
`unsafe from_raw_parts` reinterpretation was deliberately not carried
over, since it needs a `SAFETY` case for zero benefit.

### Schedule ordering: propagation first, via solo-write staging

Problem: bake must read `GlobalTransform`s that propagation recomputed
this same tick. Nothing may run bake on stale globals, and the ordering
mechanism has to come from the scheduler as built, with no scheduler
changes.

Chosen approach (`engine/canary-render-ecs/src/systems.rs`,
`engine/canary-runtime/src/main.rs:48-79`): `bake_access` declares
reads on `GlobalTransform` and `Renderable` plus
`writes_resource::<BakedFrame>`. That resource write is the whole
trick. `Schedule` stages greedily: a system joins the current stage
only if everything stays read-only and conflict-free, and every system
that writes anything gets a stage entirely to itself. Propagation
(registered first, `writes::<GlobalTransform>`) occupies its own
stage; bake's `GlobalTransform` read conflicts with that write, so
bake can never merge into it and lands in a strictly later stage. Had
bake declared reads only, it could share an early read stage and bake
stale transforms, possibly concurrently. `EcsSubsystem` owns the
registration order (propagation, then `register_render_bake`) and
`ticks` it with `schedule.run(&mut self.world)`; registering bake
first would bake stale globals, which the order test proves by failing
in that arrangement.

Rejected alternatives: scheduler changes for explicit priorities or
dependencies (a larger redesign than a two-system pipeline needs);
declaring bake read-only and hoping registration order suffices
(order without a conflict is not an ordering guarantee under greedy
staging); the App-level `Schedule` (deferred, as above).

Consequences: two write systems mean two solo stages per tick, always
in registration order. The subsystem constructor owns that order, and
the `bake_runs_after_propagation_sees_fresh_global` unit test pins it:
a moved `Transform` with a stale cached global bakes to the fresh
position after one `schedule.run()`.

### What the pixel tests prove

Problem: unit tests pin the bake math, but math without rasterization
is trust, not proof. Wrong stride, wrong attribute offsets, flipped
projection, or stale-frame uploads all survive pure-function tests and
die only on real pixels.

Chosen approach
(`engine/canary-render-ecs/tests/render_ecs_readback.rs`,
`#[ignore]`-gated like hello-triangle since every test needs a real
Vulkan ICD; dev-deps mirror the Vulkan crate's own pins,
`naga = "30"`, `indexmap = "2"`): a shared 128x128 target, per-frame fresh buffers,
`submit_and_wait`, `read_color_target_rgba8` asserts. Three tests.
`two_entities_render_distinct_colors` spawns red-left and blue-right
quads at non-overlapping NDC and asserts dominant-channel interiors
(over 150 on the entity channel, under 80 elsewhere, alpha exactly
255) plus neighbor-pixel coverage and an exactly-clear gap pixel, which
proves extraction picks up both entities, each bakes through its own
transform, colors survive upload, and one draw rasterizes both.
`moved_entity_redraws_moved` redraws the same shared target after a
`Transform` move and a second `schedule.run()`, asserting the old
pixel returns to exactly `[0, 0, 0, 255]` (stale frame replaced, target
re-cleared, not painted over) and the mirrored pixel shows the color;
this is the test that fails if propagation and bake ever run in the
wrong order. `empty_scene_clears_to_clear_color` proves the degenerate
end: an empty bake still begins, ends, and submits the pass (target
clears) without creating a zero-size buffer or issuing a zero-vertex
draw. Thresholds follow hello-triangle's dominant-channel style
because llvmpipe proves rasterization correctness, not exact edge
rounding or driver quirks.

### Draw-before-read: a target must earn its pixels first

`read_color_target_rgba8` carries a draw-before-read contract: the
target must have gone through at least one submitted render pass first.
A freshly created target holds undefined contents, and copying from it
is invalid even when a lenient driver appears to tolerate it — the
Vulkan backend tracks this per target and fails loudly in dev on
violation, but the contract itself lives at the trait level so every
future backend enforces the same ordering.

### The example is the animated proof

`examples/spinning-cube` was rewritten on the bridge (the plan's
rewrite option, keeping the GIF artifact) instead of superseded: one
cube root holding the animated `Transform` plus six face entities, one
`Renderable` per face, because a single flat color per entity cannot
hold six face colors any other way. Each of the 36 frames overwrites
the root rotation as one quaternion (fixed −30° X tilt composed with
the animated Y yaw, replacing the hand-rolled `rotate_y`/`rotate_x`),
runs the propagation-then-bake schedule, draws the `BakedFrame`,
reads back, and appends to the 480x360 GIF. The wide 4:3 target bakes
through an aspect-aware system (`bake_frame_wide`, registered under the
example's own `example_bake_access` — deliberately not the bridge's
`bake_access`, since the example bakes from a fresh `extract_scene`
and never touches `ExtractScratch`) since baking square would stretch the
image; no projection or sort code remains in the example
(`build_frame_vertices`/`project`/the hand sort are deleted). Device,
target, pipeline setup, and GIF encoding stay example-side, which is
the part no engine crate could own.

## `v0.0.10`: file-loaded meshes plus the texture-only RHI slice

Problem: `v0.0.9` proved the bridge against hand-fed soup, but no file
had ever crossed into the engine — `spinning-cube` carried its own
corner and face arrays. `v0.0.10` feeds real files from disk through
the renderer in two slices, against `canary-assets` as the first
consumer of its loading primitive (see
[`asset-system.md`](asset-system.md)).

### Meshes with zero RHI churn

Chosen approach
(`engine/canary-render-ecs/src/mesh_renderable.rs`): `MeshRenderable`
names one mesh asset by `AssetHandle<Mesh>` plus the flat RGB triple
its triangles carry into the bake — a handle rather than inline
vertices, so the store stays the single owner and the asset stays
honestly indexed. Index-to-soup expansion happens at the bridge
(`expand_mesh_to_soup`), not in the asset, because the soup layout is
the bridge's knowledge; direct indexing there is sound since the
loader bounds-checks every index. `extract_mesh_scene` resolves each
handle against the `AssetStore<Mesh>` resource and drops the rest:
stale handles skip (game content, not an engine invariant), and a
missing store yields an empty snapshot rather than an error, since a
soup-only world legitimately has none. Scheduling mirrors the soup
bake (`bake_mesh_access` declares the store read so ordering falls out
of the scheduler's conflict rules; `register_mesh_render_bake`
registers it after propagation in `EcsSubsystem::tick`). The RHI trait
is untouched: file-bytes-to-pixels for geometry with zero new methods.

`examples/spinning-cube` loads its cube faces from the checked-in
`box.glb` fixture through the bridge's own `expand_mesh_to_soup` (no
parallel copy of the index math), sliced into six two-triangle faces
with the long-standing per-face palette preserved. It still builds one
`Renderable` per face rather than one `MeshRenderable` per mesh,
because a single flat color per entity cannot hold six face colors and
`box.glb` holds two multi-face meshes — the file-loaded-entity path
itself is proven by the bridge's pixel tests, and the example proves
the same bytes reach the same GIF through the soup path. Hand-written
corner and face arrays are deleted.

### Textures via a minimal, bounded RHI addition

Chosen approach: exactly three additive trait methods plus one
descriptor, each documented as the minimal slice with the general
system named as deferred. `TextureDescriptor`
(`engine/canary-render/src/types.rs`) carries one RGBA8 image —
dimensions plus bytes in the same row-major layout `Texture::rgba8`
produces, so upload stays a copy — with no sampler choice, no mipmaps,
no sRGB handling, and no second slot. `RenderDevice::create_texture`
uploads once at creation (the write-once discipline `create_buffer`
already documents; no update story exists). A separate
`create_textured_pipeline` method, rather than a flag on
`PipelineDescriptor`, keeps every existing pipeline construction
compiling verbatim — purely additive by construction. UVs arrive as an
ordinary vertex attribute in the existing `Float32x2` format, not a
new enum variant, since a UV pair is two floats.
`CommandEncoder::set_texture` binds the one sampled texture after the
pipeline, with no slot index and no sampler parameter; the fragment
shader contract is exactly one sampled texture at set 0. The Vulkan
backend implements all of it (`engine/canary-render-vulkan/src/texture.rs`,
descriptor set layout, one default sampler, one level), and
`canary-render` still holds zero dependencies of any kind (checked
mechanically with `cargo tree`).

Bridge side
(`engine/canary-render-ecs/src/textured_renderable.rs`,
`pipeline.rs`, `systems.rs`): `TexturedRenderable` pairs a mesh handle
with a texture handle and carries no flat color, since the texture is
the color. `expand_mesh_to_textured_soup` returns `None` for a mesh
without UVs rather than inventing coordinates, and
`extract_textured_scene` skips those entities plus stale handles plus
missing stores, mirroring the mesh path's doctrine. The textured bake
is the same projection with UVs carried through untouched (same camera
constants, Y-flip, painter sort, behind-camera skip), emitting `x, y,
u, v` into a separate `BakedTexturedFrame` resource — separate because
stride, layout, pipeline, and bound texture all differ from the soup
frame, and sharing one type across two layouts would corrupt both
draws. `draw_textured_frame` uploads the frame plus the
caller-supplied texture; scheduling follows the same
access-plus-registration pattern (`bake_textured_access`,
`register_textured_render_bake`). The sampled test shader is a second
constant (`TEXTURED_WGSL`) beside the byte-identical soup one.

Proven by seven `#[ignore]`-gated offscreen pixel tests
(`engine/canary-render-ecs/tests/render_ecs_readback.rs`): the three
soup tests pass verbatim (no RHI regression), two mesh tests prove a
file-loaded quad matches hand-fed soup and redraws moved, and two
texture tests prove the PNG fixture renders quadrant-correct plus the
negative control — textured geometry drawn through the untextured
pipeline is not quadrant-correct by construction, so the sampling
proof cannot pass on clear-color luck.

### Explicitly deferred (post-`v0.0.10` RHI work, not bridge gaps)

`v0.0.10` closed two items this list named at `v0.0.9`: mesh assets
and the texture-only slice. What remains still needs trait surface
that does not exist, or generalizes what `v0.0.10` deliberately kept
to one. Push constants and uniform buffers (the eventual replacement
for the CPU bake); depth testing, depth buffers, and backface culling
(the eventual replacement for painter sort and its convex-only
limit); `write_buffer` or any buffer-update story, plus texture
updates, streaming uploads, and any texture cache (the eventual
replacement for one fresh buffer per frame and upload-once textures);
materials worthy of the name (multi-texture slots, per-draw material
selection, sampler choice, mipmaps, sRGB transfer-function handling,
blending, a shader-variant system); swapchain and window-surface
presentation (everything here is still offscreen color targets); a
real camera component (view matrix, projection choice); and the
broader App-level scheduler that would order rendering against audio
and UI once those exist (physics ordering already landed: the physics
step registers first in the subsystem schedule, ahead of propagation
and every bake — see
[`docs/architecture/physics.md`](physics.md#status-in-this-foundation)). Absences stated plainly: no
swapchain, no depth, no materials — single-texture sampling only, no
async loading, no cooking, no cache, no hot reload, no importers, and
no second mesh or texture format.

Window presentation also needs two foundations before the v0.0.13 UI
milestone: a minimal adapter/limits capability query and a backend-neutral
`Window`-to-surface seam, as locked by ADRs 0020 and 0022. That surface
contract must define format selection, resize/recreation, and recoverable
acquire/present errors. It remains deliberately absent from the current
offscreen RHI; this is a prerequisite for the next presentation slice,
not a request to build a render graph or a second backend early.

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
each is a separate, independently-optional crate behind the same trait —
confirmed directly, not just designed this way: `canary-render` itself
has zero dependencies at all (`cargo tree` shows nothing, not even
transitively), so it structurally cannot pull in a concrete backend's
dependencies under any configuration, achieved through Cargo's
dependency graph rather than a feature flag on `canary-render` (which
would in fact be a literal dependency cycle — see
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md)
for why that specific mechanism doesn't work, corrected once actually
building `canary-render-vulkan` made the reason concrete). A game build
only compiles and links whichever backend crate(s) it actually depends
on — the same "no privileged built-ins" guarantee
[`physics.md`](physics.md)'s `PhysicsBackend` trait already gives,
applied here to rendering. A backend crate's public surface must
never leak its native API's types (`ash::vk::*`, etc.) past the RHI
trait boundary, mirroring the existing rule against physics backends
leaking third-party types.

The Vulkan backend hardens the host side of that boundary rather than
trusting the driver to catch misuse. Validation layers
(`VK_LAYER_KHRONOS_validation`) are installed opportunistically in
debug builds only — never in release, never a behavior gate, absent on
drivers without them (mesa/llvmpipe CI included) — with validation
*errors* failing loudly and anything below ignored; the host-side
guards below remain the enforcement regardless. The command encoder
refuses unbound draws loudly (no pipeline or no vertex buffer bound is
driver-undefined, in the same class as the `set_texture` ordering
violation it already refused) and frees abandoned encoders in `Drop`
(panic mid-pass, early return), ending an open pass so the command
buffer stays valid to free. Dropping the device while resources created
from it still exist panics unconditionally in every profile — safe
caller code must uphold the documented resources-first/device-last
order, since the alternative is silent Vulkan use-after-destroy. And
`VulkanInitError` itself honors the boundary rule: fallible variants
erase the `ash` error into an owned code/message pair at construction,
so downstream crates match and display without ever naming `ash`
types (deliberately no blanket `From<vk::Result>`, which would re-admit
third-party types into every `?` site's inferred bounds).

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
