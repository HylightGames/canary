//! Extract + CPU-bake: [`World`](canary_ecs::World) → [`RenderItem`]s → [`BakedFrame`].
//!
//! This module is the first two thirds of the bridge's one goal: turn live ECS
//! state into GPU-ready vertex data with no GPU involvement. Extraction reads
//! the [`World`](canary_ecs::World); baking is pure math over the extracted
//! snapshot. Both are synchronous, deterministic, and testable without Vulkan —
//! the GPU draw call itself lives in [`crate::pipeline`] and is the only part
//! of the pipeline that needs a real device.
//!
//! # Why CPU-bake instead of GPU transforms
//!
//! The RHI deliberately offers no way to hand the GPU a per-frame transform:
//! [`PipelineDescriptor`](canary_render::PipelineDescriptor) documents "no
//! descriptor sets/uniforms", and there are no push-constant, uniform-buffer,
//! or buffer-update methods anywhere on
//! [`RenderDevice`](canary_render::RenderDevice) (verified against
//! `engine/canary-render/src/lib.rs` — the trait surface is `create_buffer`,
//! `create_color_target`, `create_pipeline`, `create_command_encoder`,
//! `submit_and_wait`, `read_color_target_rgba8`, and nothing else). The only
//! upload path is `create_buffer`'s initial-content bytes, and buffers are
//! write-once. So the honest design given that scope — the same one
//! `examples/spinning-cube` already proved — is to apply each entity's world
//! matrix and the camera projection here on the CPU, once per frame, and
//! upload the resulting NDC-space vertices as a fresh buffer per frame. When
//! the RHI grows push constants or uniform buffers (deferred to v0.0.10+),
//! this bake step is what gets replaced; the extract step stays.
//!
//! # Why the Y-flip negate
//!
//! Vulkan NDC has +Y pointing down (called out in
//! `engine/canary-render-vulkan/tests/hello_triangle.rs`, where `-0.5` is
//! "visually up"). Object space follows the conventional Y-up orientation, so
//! the projection negates the Y coordinate: without the negate, "up" in game
//! space would render as "down" on screen.
//!
//! # Why painter-sort, and its convex-only limit
//!
//! The RHI has no depth buffer, no depth/stencil state, and no backface
//! culling (the Vulkan backend hard-codes `CullMode::NONE`). Nothing
//! downstream discards occluded fragments, so triangles are sorted
//! back-to-front by average camera-space depth before upload: each later
//! triangle overdraws whatever is behind it. This is exactly correct for
//! convex, non-self-intersecting shapes viewed from outside (a cube
//! qualifies) and is *not* sufficient for concave or interpenetrating
//! geometry, where per-pixel depth resolution is required. That needs a real
//! depth buffer — deferred RHI work for v0.0.10+ — not a smarter sort here.
//!
//! # Why fixed camera constants
//!
//! There is no camera component yet: `CAMERA_DISTANCE` and `FOCAL_LENGTH`
//! are reused verbatim from `examples/spinning-cube`'s proven values. A real
//! camera (component + view matrix + projection choice) is v0.0.10+ scope;
//! inventing a half-camera here would bake an API the real one must then
//! support for compatibility.
//!
//! # Why extraction skips instead of panicking
//!
//! An entity missing one of the two components, or carrying a [`Renderable`]
//! whose vertex list is not a whole number of triangles, is a *game-content*
//! condition — a half-built prefab, a mesh asset that failed validation
//! upstream — not an engine invariant violation. The engine must not panic on
//! game-content conditions, so [`extract_scene`] silently skips both cases:
//! entities with only one component never even surface (a [`World::query2`]
//! intersection only yields entities holding *both*), and invalid renderables
//! are filtered via [`Renderable::is_valid`]. Skipping an invalid renderable
//! rather than emitting its partial tail matters: a trailing 1–2 vertices
//! form no triangle, and feeding them to the single-vertex-buffer draw would
//! corrupt the frame's vertex alignment for every triangle after them.
//!
//! [`Renderable`]: crate::Renderable
//! [`Renderable::is_valid`]: crate::Renderable::is_valid
//! [`World::query2`]: canary_ecs::World::query2

use canary_ecs::World;
use canary_transform::GlobalTransform;
use glam::Vec3;

use crate::Renderable;

/// Distance the virtual camera sits behind the world origin, in world units.
///
/// The camera is modeled at the origin looking down +Z; every world-space
/// point is shifted +Z by this amount into camera space before projection.
/// Reused verbatim from `examples/spinning-cube` (see that file's
/// `build_frame_vertices`): no camera component exists yet, so the bridge
/// shares the example's proven constant rather than inventing a new one.
const CAMERA_DISTANCE: f32 = 3.2;

/// Focal length of the hand-rolled perspective projection.
///
/// Larger values narrow the field of view. Reused verbatim from
/// `examples/spinning-cube` for the same reason as `CAMERA_DISTANCE`.
const FOCAL_LENGTH: f32 = 2.2;

/// Default target aspect ratio (width divided by height) used by
/// [`bake_scene_to_vertices`].
///
/// `1.0` (a square target) is the neutral default: it leaves the horizontal
/// field of view unscaled. Callers drawing to non-square targets must use
/// [`bake_scene_to_vertices_with_aspect`] with the real ratio instead —
/// baking with the wrong aspect stretches the image horizontally.
const DEFAULT_ASPECT_RATIO: f32 = 1.0;

/// Number of `f32`s emitted per baked vertex: NDC `x`, `y` plus flat `r`, `g`,
/// `b`.
///
/// Matches the shader contract exactly: one
/// [`Float32x2`](canary_render::VertexFormat::Float32x2) position attribute
/// (2 floats) followed by one
/// [`Float32x3`](canary_render::VertexFormat::Float32x3) color attribute (3
/// floats). [`BakedFrame::vertex_count`] and the draw call both derive from
/// this constant so the layout has a single source of truth.
pub(crate) const FLOATS_PER_VERTEX: usize = 5;

/// Camera-space depths at or below this threshold are treated as on-or-behind
/// the camera plane.
///
/// Perspective projection divides by camera-space `z`; at `z == 0` the result
/// is infinite and behind the camera (`z < 0`) it mirrors to the wrong side
/// of the screen. Either outcome would poison the uploaded buffer with
/// `inf`/`NaN` vertices — a game-content condition (an entity walking through
/// the camera), not an engine bug — so such triangles are skipped. The value
/// is a small epsilon rather than exactly zero so that near-plane-grazing
/// floating-point noise cannot sneak an astronomical-but-finite vertex past
/// the guard. Non-finite corners (`NaN`/`inf` from degenerate file data or
/// transform overflow) skip by the same guard: comparisons alone cannot catch
/// them (`NaN <= threshold` is false), so finiteness is checked explicitly.
const MIN_CAMERA_DEPTH: f32 = 1e-6;

/// One entity's contribution to a frame: its world transform plus its
/// object-space triangle soup and flat color, copied out of the [`World`](canary_ecs::World).
///
/// A snapshot, not a borrow: extraction clones the data so that baking (pure
/// math, sortable, GPU-free) never holds a [`World`](canary_ecs::World)
/// borrow. Only well-formed items exist — [`extract_scene`] guarantees every
/// `vertices` here partitions into whole triangles — so bake can iterate
/// [`slice::chunks_exact`] without a partial-tail case.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderItem {
    /// The entity's cached world-space matrix, copied from its
    /// [`GlobalTransform`] at extract time.
    ///
    /// [`GlobalTransform`]: canary_transform::GlobalTransform
    pub global: GlobalTransform,
    /// Object-space vertex positions as consecutive triangles: every three
    /// entries form one triangle. Always a multiple of three — enforced by
    /// [`extract_scene`]'s [`Renderable::is_valid`] filter, which is what
    /// lets bake treat a partial tail as impossible rather than as a case
    /// to handle.
    ///
    /// [`Renderable::is_valid`]: crate::Renderable::is_valid
    pub vertices: Vec<[f32; 3]>,
    /// Flat RGB color shared by every vertex of this entity, replicated into
    /// each baked vertex's color attribute at bake time (the RHI has no
    /// uniforms or materials to carry it any other way).
    pub color: [f32; 3],
}

/// Scratch buffer holding last tick's snapshot so [`extract_scene_into`]
/// can refresh it in place.
///
/// Per-tick `vertices.clone()` into fresh [`Vec`]s costs a fresh
/// allocation per entity per tick even in steady state (same entities,
/// same mesh sizes); refreshing the previous tick's buffers via
/// [`Vec::clone_from`] reuses them instead, which measures ~3x faster
/// end-to-end on the soup path at 10k tris (fresh input buffers read
/// markedly slower than stable reused ones on the measured machine —
/// allocator/page steady-state, not memcpy bandwidth). A plain
/// [`Vec`], not a pool: slots are positional, entity order comes from
/// the query, and any order shuffle only costs a realloc, never
/// correctness (see [`extract_scene_into`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtractScratch(pub Vec<RenderItem>);

/// Reads every renderable entity out of `world` into a snapshot [`Vec`].
///
/// Queries the [`World::query2`] intersection of [`GlobalTransform`] and
/// [`Renderable`](crate::Renderable): entities missing *either* component are
/// never yielded by the query itself, and entities whose renderable fails
/// [`Renderable::is_valid`] are filtered here. Never panics on world
/// contents — see this module's docs for why skipping (not panicking, not
/// emitting partial triangles) is the correct response to missing or invalid
/// game content.
///
/// [`World::query2`]: canary_ecs::World::query2
/// [`GlobalTransform`]: canary_transform::GlobalTransform
pub fn extract_scene(world: &World) -> Vec<RenderItem> {
    let mut items = Vec::new();
    extract_scene_into(world, &mut items);
    items
}

/// Reads every renderable entity out of `world` into `out`, reusing its
/// buffers across ticks.
///
/// Value-identical to [`extract_scene`] on every call: each slot is fully
/// overwritten (transform, vertices, color) and the tail is truncated, so
/// whatever `out` held before — last tick's snapshot, a longer entity
/// list, garbage lengths — cannot leak into the result. [`Vec::clone_from`]
/// reuses a slot's vertex buffer whenever capacity suffices (the steady
/// state: same entities, same mesh sizes) and reallocates only on genuine
/// shape change (new entity, resized mesh, reshuffled query order). Skips
/// exactly what [`extract_scene`] skips, in the same order.
///
/// [`World::query2`]: canary_ecs::World::query2
pub fn extract_scene_into(world: &World, out: &mut Vec<RenderItem>) {
    let mut index = 0;
    for (_, global, renderable) in world
        .query2::<GlobalTransform, Renderable>()
        .filter(|(_, _, renderable)| renderable.is_valid())
    {
        if let Some(slot) = out.get_mut(index) {
            slot.global = *global;
            slot.vertices.clone_from(&renderable.vertices);
            slot.color = renderable.color;
        } else {
            out.push(RenderItem {
                global: *global,
                vertices: renderable.vertices.clone(),
                color: renderable.color,
            });
        }
        index += 1;
    }
    out.truncate(index);
}

/// One frame of GPU-ready vertex data: NDC `x`, `y` plus flat `r`, `g`, `b`
/// per vertex (`FLOATS_PER_VERTEX` floats each), in painter-sorted draw
/// order (far triangles first).
///
/// Produced by [`bake_scene_to_vertices`] (or its aspect-aware sibling) and
/// consumed by [`crate::pipeline::draw_baked_frame`], which uploads
/// [`BakedFrame::vertices`] as the initial content of a fresh vertex buffer.
/// Plain data — no device handles, no lifetimes — so it can cross the
/// extract/bake/draw boundary (and, in Task 4, live as an ECS resource)
/// without dragging GPU types into scheduling.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedFrame {
    /// Baked vertex floats: `x, y, r, g, b` per vertex, `5 * vertex_count()`
    /// floats total. May be empty (an empty scene bakes to an empty frame,
    /// which still clears the target when drawn).
    ///
    /// Alignment invariant: `len() % FLOATS_PER_VERTEX == 0` always —
    /// the bake functions guarantee it, and the draw call
    /// `debug_assert!`s it. Hand-constructed frames must uphold it too.
    pub vertices: Vec<f32>,
}

impl BakedFrame {
    /// The number of vertices in this frame: `vertices.len() / 5`.
    ///
    /// This is the `vertex_count` the RHI
    /// [`draw`](canary_render::CommandEncoder::draw) call consumes — one draw
    /// over the whole buffer, no indices, no instancing. Saturates at
    /// `u32::MAX` instead of wrapping on absurd inputs: truncating a frame
    /// no GPU could ever hold is strictly saner than drawing a wrapped
    /// near-zero count and presenting it as success.
    pub fn vertex_count(&self) -> u32 {
        u32::try_from(self.vertices.len() / FLOATS_PER_VERTEX).unwrap_or(u32::MAX)
    }

    /// Whether this frame holds no vertices.
    ///
    /// An empty frame is a legitimate bake result (empty scene, every entity
    /// skipped, every triangle behind the camera) — not an error. Drawing it
    /// still submits the clear pass, so the target shows the clear color
    /// rather than stale contents; see
    /// [`crate::pipeline::draw_baked_frame`].
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

/// One triangle in camera space, awaiting the painter-sort: its average
/// depth (the sort key), its three corner positions (the project inputs),
/// and the flat color all three vertices share.
#[derive(Debug, Clone, PartialEq)]
struct PendingTriangle {
    avg_depth: f32,
    corners: [Vec3; 3],
    color: [f32; 3],
}

/// Scratch buffer holding last tick's bake intermediates so
/// [`bake_scene_to_vertices_with_aspect_into`] can refresh them in place.
///
/// Per-tick `Vec<PendingTriangle>` construction costs a fresh allocation
/// per frame even in steady state (same entities, same triangle count);
/// clearing the previous tick's buffer and refilling it reuses the
/// allocation instead. A plain [`Vec`], not a pool: slots are positional
/// (one per surviving triangle, in extract order before the sort), and any
/// count change only costs a `reserve`, never correctness (see
/// [`bake_scene_to_vertices_with_aspect_into`]). The emitted vertex floats
/// are *not* stored here: they live in the [`BakedFrame`] resource itself,
/// whose buffer the scheduled bake system reuses the same way — one
/// scratch resource per intermediate, one owner per buffer, no aliasing.
///
/// The painter sort runs over the reused `order` index buffer rather than
/// over the triangles themselves: each `PendingTriangle` holds three
/// `Vec3` corners plus color plus depth (~52 bytes), so swapping whole
/// triangles during the sort moves an order of magnitude more bytes per
/// comparison than swapping indices. Emission gathers through the sorted
/// indices instead, keeping the output byte-identical to a direct sort
/// (`sort_by` is stable, and equal depths retain extract order either
/// way).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BakeScratch {
    triangles: Vec<PendingTriangle>,
    order: Vec<usize>,
}

/// Bakes `items` into NDC-space vertex floats for a square target.
///
/// Applies each item's world matrix ([`GlobalTransform::matrix`] +
/// `transform_point3`), shifts into camera space (+`CAMERA_DISTANCE` on Z),
/// perspective-projects with `FOCAL_LENGTH` and `DEFAULT_ASPECT_RATIO`,
/// painter-sorts triangles far-to-near by average camera-space depth, and
/// emits `x, y, r, g, b` per vertex. Triangles on or behind the camera plane
/// (depth `<= [`MIN_CAMERA_DEPTH`]`) are skipped — their projection is
/// undefined. See this module's docs for the why behind every one of those
/// choices.
///
/// For non-square targets, prefer [`bake_scene_to_vertices_with_aspect`]:
/// this function is exactly that function with the neutral square aspect, so
/// pixel-test targets (conventionally square) need no extra argument.
///
/// Per-frame callers should prefer [`bake_scene_to_vertices_into`], which
/// reuses both the output and the [`BakeScratch`] allocation: this function
/// is exactly that function with fresh buffers, so one-shot callers (tests,
/// examples) pay no scratch-plumbing cost.
///
/// [`GlobalTransform::matrix`]: canary_transform::GlobalTransform::matrix
pub fn bake_scene_to_vertices(items: &[RenderItem]) -> Vec<f32> {
    bake_scene_to_vertices_with_aspect(items, DEFAULT_ASPECT_RATIO)
}

/// Bakes `items` into `out`, reusing its buffer across frames.
///
/// Value-identical to [`bake_scene_to_vertices`] on every call: `out` is
/// fully cleared and refilled, so whatever it held before — last tick's
/// frame, a longer triangle list, spare capacity — cannot leak into the
/// result. Clearing (rather than overwriting in place) keeps the
/// painter-sort emission order byte-identical to the fresh path: triangles
/// are still collected, sorted far-to-near, then emitted. `reserve` grows
/// the buffer only on genuine shape change (more surviving triangles than
/// the previous high-water mark); the steady state performs zero
/// allocations. `scratch` carries the pending-triangle intermediates across
/// calls under the same contract (see [`BakeScratch`]).
pub fn bake_scene_to_vertices_into(
    items: &[RenderItem],
    out: &mut Vec<f32>,
    scratch: &mut BakeScratch,
) {
    bake_scene_to_vertices_with_aspect_into(items, DEFAULT_ASPECT_RATIO, out, scratch);
}

/// Bakes `items` into NDC-space vertex floats for a target of the given
/// aspect ratio (width divided by height).
///
/// Identical to [`bake_scene_to_vertices`] except for the horizontal scale:
/// `x = (x * `FOCAL_LENGTH`) / (z * aspect_ratio)`, matching
/// `examples/spinning-cube`'s `project` (`WIDTH / HEIGHT` there). A caller
/// drawing to a 480×360 target bakes with `480.0 / 360.0`; baking with the
/// wrong aspect stretches the image rather than failing, so getting this
/// argument right is a correctness obligation on the draw path (Task 6 wires
/// it to the real target dimensions).
pub fn bake_scene_to_vertices_with_aspect(items: &[RenderItem], aspect_ratio: f32) -> Vec<f32> {
    let mut scratch = BakeScratch::default();
    let mut out = Vec::new();
    bake_scene_to_vertices_with_aspect_into(items, aspect_ratio, &mut out, &mut scratch);
    out
}

/// Bakes `items` into `out` for a target of the given aspect ratio (width
/// divided by height), reusing both buffers across frames.
///
/// Identical to [`bake_scene_to_vertices_into`] except for the horizontal
/// scale; the aspect-fallback contract (silent square fallback in release,
/// debug loudness) matches [`bake_scene_to_vertices_with_aspect`]
/// entry-for-entry, because it is the same caller obligation with the same
/// poisoning consequence.
pub fn bake_scene_to_vertices_with_aspect_into(
    items: &[RenderItem],
    aspect_ratio: f32,
    out: &mut Vec<f32>,
    scratch: &mut BakeScratch,
) {
    debug_assert!(
        aspect_ratio > 0.0 && aspect_ratio.is_finite(),
        "aspect_ratio must be a positive finite width/height; got {aspect_ratio}"
    );
    // A zero/negative/non-finite aspect would divide into `inf`/`NaN`
    // NDC vertices below and poison the uploaded buffer, so fall back
    // to the neutral square aspect rather than propagating garbage to
    // the GPU. (Debug builds already failed loudly above.)
    let aspect_ratio = if aspect_ratio > 0.0 && aspect_ratio.is_finite() {
        aspect_ratio
    } else {
        DEFAULT_ASPECT_RATIO
    };
    // Refresh in place: the previous tick's pending triangles are dropped
    // but their allocation is kept, so the steady state (same surviving
    // triangle count) never reallocates here.
    let triangles = &mut scratch.triangles;
    triangles.clear();
    for item in items {
        // Colors ride through untouched, so a non-finite channel
        // reaches the upload verbatim (undefined UNORM output).
        // Checked here rather than in `Renderable::new`, which stays
        // infallible by design.
        debug_assert!(
            item.color.iter().all(|c| c.is_finite()),
            "non-finite Renderable color uploads as undefined pixels"
        );
        let matrix = item.global.matrix();
        for chunk in item.vertices.chunks_exact(3) {
            // `chunks_exact(3)` drops a partial tail instead of panicking;
            // via `extract_scene` a tail is impossible (invalid renderables
            // are filtered), and hand-built items degrade to a dropped
            // fragment rather than a corrupt frame.
            let corners = [
                matrix.transform_point3(Vec3::from(chunk[0])),
                matrix.transform_point3(Vec3::from(chunk[1])),
                matrix.transform_point3(Vec3::from(chunk[2])),
            ];
            // Camera at the origin looking down +Z: shift the world into
            // front-of-camera space, mirroring spinning-cube's
            // `tilted[2] + CAMERA_DISTANCE`.
            let camera_space = [
                Vec3::new(corners[0].x, corners[0].y, corners[0].z + CAMERA_DISTANCE),
                Vec3::new(corners[1].x, corners[1].y, corners[1].z + CAMERA_DISTANCE),
                Vec3::new(corners[2].x, corners[2].y, corners[2].z + CAMERA_DISTANCE),
            ];
            if camera_space
                .iter()
                .any(|v| !v.is_finite() || v.z <= MIN_CAMERA_DEPTH)
            {
                continue;
            }
            let avg_depth = (camera_space[0].z + camera_space[1].z + camera_space[2].z) / 3.0;
            triangles.push(PendingTriangle {
                avg_depth,
                corners: camera_space,
                color: item.color,
            });
        }
    }
    // Painter's algorithm: farthest first so nearer triangles overdraw.
    // `total_cmp` gives a deterministic order for every `f32` bit pattern
    // (no NaN-panic path exists in `sort_by`'s contract to worry about).
    // Sorted as indices into `triangles` (see `BakeScratch`): swapping
    // whole `PendingTriangle`s would move ~52 bytes per comparison.
    let triangles: &[PendingTriangle] = &scratch.triangles;
    let order = &mut scratch.order;
    order.clear();
    order.extend(0..triangles.len());
    order.sort_by(|&a, &b| triangles[b].avg_depth.total_cmp(&triangles[a].avg_depth));

    // Refresh in place: last frame's vertex floats are dropped but their
    // allocation is kept, so the steady state never reallocates here.
    // Emission order is untouched — the `clear` cannot leak into the
    // result, and nothing below reads `out` before writing it.
    // Gathered through the sorted indices: triangle for triangle, this is
    // the same far-to-near order a direct sort would emit.
    out.clear();
    out.reserve(triangles.len() * 3 * FLOATS_PER_VERTEX);
    for &index in order.iter() {
        let triangle = &triangles[index];
        // Project first, emit after: a partial triangle (fewer than
        // three vertices) would misalign the whole soup, so validity is
        // decided per triangle, never per vertex.
        let mut projected = [(0.0f32, 0.0f32); 3];
        let mut valid = true;
        for (corner, slot) in triangle.corners.iter().zip(projected.iter_mut()) {
            // Spinning-cube's `project`, generalized to caller-supplied
            // aspect: perspective divide, with the Y-flip negate that keeps
            // object-space "up" visually up under Vulkan's Y-down NDC.
            let ndc_x = (corner.x * FOCAL_LENGTH) / (corner.z * aspect_ratio);
            let ndc_y = -(corner.y * FOCAL_LENGTH) / corner.z;
            // Finite camera-space inputs can still overflow this divide
            // (e.g. huge x against a near-minimum z): an `inf` NDC
            // vertex would upload garbage the driver reads as geometry,
            // so the triangle is skipped exactly like a behind-camera
            // one rather than emitted half-valid.
            if !ndc_x.is_finite() || !ndc_y.is_finite() {
                valid = false;
                break;
            }
            *slot = (ndc_x, ndc_y);
        }
        if !valid {
            continue;
        }
        for (ndc_x, ndc_y) in &projected {
            out.extend_from_slice(&[*ndc_x, *ndc_y]);
            out.extend_from_slice(&triangle.color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_transform::GlobalTransform;
    use glam::Mat4;

    /// Asserts two `f32` values agree within `1e-5`; exact `==` on projected
    /// floats is brittle, so every projection assertion goes through here.
    fn assert_approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "expected {expected}, got {actual}"
        );
    }

    /// One unit right-triangle in the `z = 0` plane with the identity
    /// transform: the shared fixture for the pure-bake tests.
    fn unit_triangle(color: [f32; 3]) -> RenderItem {
        RenderItem {
            global: GlobalTransform::default(),
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            color,
        }
    }

    #[test]
    #[should_panic(expected = "aspect_ratio must be a positive finite")]
    #[cfg_attr(
        not(debug_assertions),
        ignore = "release builds fall back to square instead of panicking -- see below"
    )]
    fn invalid_aspect_fails_loudly_in_debug() {
        // Debug builds enforce the caller obligation up front; release
        // builds take the silent square fallback instead (next test).
        let item = unit_triangle([1.0, 0.0, 0.0]);
        let _ = bake_scene_to_vertices_with_aspect(std::slice::from_ref(&item), 0.0);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn invalid_aspect_falls_back_to_the_square_default_in_release() {
        // Release-only companion to the test above: with debug
        // assertions off, a zero/negative/non-finite aspect bakes
        // exactly like the square default, with no inf/NaN leaking
        // into the vertex buffer.
        let item = unit_triangle([1.0, 0.0, 0.0]);
        let items = std::slice::from_ref(&item);
        let expected = bake_scene_to_vertices(items);
        for bad_aspect in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let baked = bake_scene_to_vertices_with_aspect(items, bad_aspect);
            assert_eq!(
                baked, expected,
                "aspect {bad_aspect} must bake exactly like the square default"
            );
            assert!(
                baked.iter().all(|v| v.is_finite()),
                "aspect {bad_aspect} must not leak inf/NaN into the buffer"
            );
        }
    }

    #[test]
    fn bake_single_triangle_projects_with_y_flip() {
        let item = unit_triangle([1.0, 0.0, 0.0]);

        let baked = bake_scene_to_vertices(std::slice::from_ref(&item));

        // 3 vertices × 5 floats (x, y, r, g, b).
        assert_eq!(baked.len(), 15);
        // Identity transform + camera shift: camera-space z is 3.2
        // everywhere, focal length 2.2, square aspect.
        let focal = 2.2_f32;
        let distance = 3.2_f32;
        // Object origin → NDC origin.
        assert_approx_eq(baked[0], 0.0);
        assert_approx_eq(baked[1], 0.0);
        // Object +X → positive NDC x.
        assert_approx_eq(baked[5], focal / distance);
        assert_approx_eq(baked[6], 0.0);
        // Object +Y ("up") → *negative* NDC y: the Y-flip negate doing its
        // job under Vulkan's Y-down NDC. Without the negate this would be
        // +0.6875 and the triangle would render upside down.
        assert_approx_eq(baked[10], 0.0);
        assert_approx_eq(baked[11], -(focal / distance));
        assert!(
            baked[11] < 0.0,
            "positive object-Y must land at negative NDC-Y (Y-flip), got {}",
            baked[11]
        );
        // Flat color replicated into every vertex.
        for vertex in baked.chunks_exact(5) {
            assert_approx_eq(vertex[2], 1.0);
            assert_approx_eq(vertex[3], 0.0);
            assert_approx_eq(vertex[4], 0.0);
        }
    }

    #[test]
    fn bake_sorts_far_triangles_first() {
        let near = unit_triangle([1.0, 0.0, 0.0]);
        let far = RenderItem {
            global: GlobalTransform::from_matrix(Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0))),
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            color: [0.0, 0.0, 1.0],
        };

        // Deliberately near-first input: the output order must come from the
        // depth sort, not from input order.
        let baked = bake_scene_to_vertices(&[near, far]);

        assert_eq!(baked.len(), 30);
        // First triangle drawn = far (blue): painter's algorithm draws
        // back-to-front so the near triangle overdraws it.
        for vertex in baked[..15].chunks_exact(5) {
            assert_approx_eq(vertex[2], 0.0);
            assert_approx_eq(vertex[3], 0.0);
            assert_approx_eq(vertex[4], 1.0);
        }
        for vertex in baked[15..].chunks_exact(5) {
            assert_approx_eq(vertex[2], 1.0);
            assert_approx_eq(vertex[3], 0.0);
            assert_approx_eq(vertex[4], 0.0);
        }
    }

    #[test]
    fn bake_empty_scene_yields_empty_frame() {
        let frame = BakedFrame {
            vertices: bake_scene_to_vertices(&[]),
        };

        assert!(frame.vertices.is_empty());
        assert!(frame.is_empty());
        assert_eq!(frame.vertex_count(), 0);
    }

    #[test]
    fn bake_skips_triangles_behind_camera() {
        // World z = -5 → camera-space z = -1.8: behind the camera plane, so
        // its perspective projection is undefined (infinite at z = 0,
        // mirrored for z < 0) and the triangle must not reach the frame.
        let behind = RenderItem {
            global: GlobalTransform::from_matrix(Mat4::from_translation(Vec3::new(0.0, 0.0, -5.0))),
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            color: [1.0, 0.0, 0.0],
        };

        let baked = bake_scene_to_vertices(std::slice::from_ref(&behind));

        assert!(
            baked.is_empty(),
            "triangles behind the camera must be skipped, got {baked:?}"
        );
    }

    #[test]
    fn extract_skips_entities_missing_either_component() {
        let mut world = World::new();
        let triangle = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let both = world.spawn();
        world
            .insert(both, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(both, Renderable::new(triangle.clone(), [1.0, 0.0, 0.0]))
            .expect("fresh entity accepts Renderable");
        let global_only = world.spawn();
        world
            .insert(global_only, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        let renderable_only = world.spawn();
        world
            .insert(
                renderable_only,
                Renderable::new(triangle.clone(), [0.0, 0.0, 1.0]),
            )
            .expect("fresh entity accepts Renderable");

        let items = extract_scene(&world);

        // `query2` is an intersection: the two half-equipped entities never
        // surface, so exactly the fully-equipped one extracts.
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].vertices, triangle);
        assert_eq!(items[0].color, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn into_fresh_buffer_matches_extract_scene() {
        let mut world = World::new();
        for color in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            let entity = world.spawn();
            world
                .insert(entity, GlobalTransform::default())
                .expect("fresh entity accepts GlobalTransform");
            world
                .insert(
                    entity,
                    Renderable::new(
                        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                        color,
                    ),
                )
                .expect("fresh entity accepts Renderable");
        }

        let mut reused = Vec::new();
        extract_scene_into(&world, &mut reused);

        assert_eq!(reused, extract_scene(&world));
    }

    #[test]
    fn into_reused_buffer_matches_after_add_remove_resize() {
        let mut world = World::new();
        let keep = world.spawn();
        world
            .insert(keep, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(
                keep,
                Renderable::new(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [1.0, 0.0, 0.0],
                ),
            )
            .expect("fresh entity accepts Renderable");
        let drop_me = world.spawn();
        world
            .insert(drop_me, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(
                drop_me,
                Renderable::new(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0.0, 0.0, 1.0],
                ),
            )
            .expect("fresh entity accepts Renderable");

        // Prime the scratch on the two-entity world: buffers are now live.
        let mut reused = Vec::new();
        extract_scene_into(&world, &mut reused);
        assert_eq!(reused.len(), 2);

        // Shrink (despawn), grow (spawn with a bigger mesh), and resize
        // the survivor's mesh: exercises truncate, push, and realloc.
        world.despawn(drop_me).expect("entity is alive");
        let big = world.spawn();
        world
            .insert(big, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        let mut six = Vec::new();
        for _ in 0..2 {
            six.extend_from_slice(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        }
        world
            .insert(big, Renderable::new(six, [0.0, 1.0, 0.0]))
            .expect("fresh entity accepts Renderable");
        world
            .get_mut::<Renderable>(keep)
            .expect("keeper still has its Renderable")
            .vertices
            .extend_from_slice(&[[2.0, 2.0, 2.0], [3.0, 3.0, 3.0], [4.0, 4.0, 4.0]]);

        extract_scene_into(&world, &mut reused);

        // Value-identical to a fresh extract despite reused slots: no
        // stale tail, no stale vertices, resized buffers refreshed.
        assert_eq!(reused, extract_scene(&world));
        assert_eq!(reused.len(), 2);
    }

    #[test]
    fn extract_skips_invalid_renderables() {
        let mut world = World::new();
        let valid = world.spawn();
        world
            .insert(valid, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(
                valid,
                Renderable::new(
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    [0.0, 1.0, 0.0],
                ),
            )
            .expect("fresh entity accepts Renderable");
        // Two vertices: a trailing partial triangle, `!is_valid()`.
        let invalid = world.spawn();
        world
            .insert(invalid, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(
                invalid,
                Renderable::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], [1.0, 0.0, 0.0]),
            )
            .expect("fresh entity accepts Renderable");

        let items = extract_scene(&world);

        // The invalid renderable is skipped per the downstream contract its
        // own `is_valid` docs name: emitting its partial tail would corrupt
        // the frame's vertex alignment for every triangle after it.
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].color, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn bake_skips_triangles_with_nan_depth() {
        // Given: a triangle whose first vertex has a NaN depth, otherwise
        // in front of the camera.
        let nan_depth = RenderItem {
            global: GlobalTransform::default(),
            vertices: vec![[0.0, 0.0, f32::NAN], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            color: [1.0, 0.0, 0.0],
        };

        // When: baked.
        let baked = bake_scene_to_vertices(std::slice::from_ref(&nan_depth));

        // Then: skipped — the camera-space guard checks finiteness
        // first (`!v.is_finite()`), so the NaN depth never reaches the
        // projection; same as the textured bake's `all(finite && ...)`
        // clause (the two are De Morgan-equivalent, not divergent).
        assert!(
            baked.is_empty(),
            "a NaN depth has no defined projection and must skip, got {baked:?}"
        );
    }

    #[test]
    fn bake_skips_triangles_whose_projection_overflows_to_inf() {
        // Given: finite camera-space inputs whose perspective divide
        // overflows — `f32::MAX` x against an ordinary depth. Both
        // values pass the camera-space guard (finite, z well above
        // the minimum), so only a post-projection check can catch this:
        // the numerator overflows `f32` long before the divide.
        let overflowing = RenderItem {
            global: GlobalTransform::default(),
            vertices: vec![[f32::MAX, 0.0, 0.0], [1.0, 0.0, 3.0], [0.0, 1.0, 3.0]],
            color: [1.0, 0.0, 0.0],
        };

        // When: baked.
        let baked = bake_scene_to_vertices(std::slice::from_ref(&overflowing));

        // Then: skipped whole (a partial triangle would misalign the
        // soup), and whatever remains is all finite.
        assert!(
            baked.is_empty(),
            "an overflowing projection must skip the triangle, got {baked:?}"
        );
        assert!(
            baked.iter().all(|v| v.is_finite()),
            "no bake output may carry inf/NaN, got {baked:?}"
        );
    }

    #[test]
    fn bake_skips_triangles_with_nan_lateral_position() {
        // Given: a triangle with a NaN x coordinate (finite depth).
        let nan_x = RenderItem {
            global: GlobalTransform::default(),
            vertices: vec![[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            color: [0.0, 1.0, 0.0],
        };

        // When: baked.
        let baked = bake_scene_to_vertices(std::slice::from_ref(&nan_x));

        // Then: skipped — the affine multiply poisons `w` (`0 * NaN` is
        // NaN), so the whole transformed corner including its depth goes
        // NaN; the depth guard alone still lets it through (NaN fails the
        // `<=` comparison), so only a finiteness check catches it.
        assert!(
            baked.is_empty(),
            "a NaN lateral position must skip, got {baked:?}"
        );
        assert!(
            baked.iter().all(|v| v.is_finite()),
            "no bake output may carry inf/NaN, got {baked:?}"
        );
    }

    #[test]
    fn bake_into_reused_buffers_matches_fresh_bake_after_shape_change() {
        let red = unit_triangle([1.0, 0.0, 0.0]);
        let blue = RenderItem {
            global: GlobalTransform::from_matrix(Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0))),
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
            ],
            color: [0.0, 0.0, 1.0],
        };
        let mut out = vec![f32::NAN; 1024];
        let mut scratch = BakeScratch::default();

        // Prime both buffers on the bigger scene, then shrink to one
        // triangle: exercises the clear paths with stale longer contents.
        bake_scene_to_vertices_into(&[red.clone(), blue], &mut out, &mut scratch);
        let small = std::slice::from_ref(&red);
        bake_scene_to_vertices_into(small, &mut out, &mut scratch);

        // Value-identical to a fresh bake despite reused slots: no stale
        // tail, no stale floats, no NaN sentinel leaking through.
        assert_eq!(out, bake_scene_to_vertices(small));
        assert_eq!(out.len(), 3 * FLOATS_PER_VERTEX);
        assert!(out.iter().all(|v| v.is_finite()));
        // The aspect-aware entry point holds the same contract.
        let mut aspect_out = Vec::new();
        let mut aspect_scratch = BakeScratch::default();
        bake_scene_to_vertices_with_aspect_into(
            small,
            16.0 / 9.0,
            &mut aspect_out,
            &mut aspect_scratch,
        );
        assert_eq!(
            aspect_out,
            bake_scene_to_vertices_with_aspect(small, 16.0 / 9.0)
        );
    }

    #[test]
    fn second_bake_into_reused_buffers_reuses_allocations() {
        let red = unit_triangle([1.0, 0.0, 0.0]);
        let blue = RenderItem {
            global: GlobalTransform::from_matrix(Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0))),
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            color: [0.0, 0.0, 1.0],
        };
        let items = [red, blue];
        let mut out = Vec::new();
        let mut scratch = BakeScratch::default();
        bake_scene_to_vertices_into(&items, &mut out, &mut scratch);
        let out_ptr = out.as_ptr();
        let out_capacity = out.capacity();
        let triangles_ptr = scratch.triangles.as_ptr();
        let triangles_capacity = scratch.triangles.capacity();
        let first = out.clone();

        // When: the same scene bakes again into the same buffers (the
        // steady state: same entities, same triangle count).
        bake_scene_to_vertices_into(&items, &mut out, &mut scratch);

        // Then: identical output with zero reallocations — both buffers
        // kept their allocations.
        assert_eq!(out, first);
        assert_eq!(
            out.as_ptr(),
            out_ptr,
            "the output buffer must be reused, not reallocated"
        );
        assert_eq!(out.capacity(), out_capacity);
        assert_eq!(
            scratch.triangles.as_ptr(),
            triangles_ptr,
            "the pending-triangle buffer must be reused, not reallocated"
        );
        assert_eq!(scratch.triangles.capacity(), triangles_capacity);
    }

    #[test]
    fn bake_skips_triangles_with_infinite_projected_position() {
        // Given: finite inputs under a scale so large the transformed x
        // overflows to infinity while z stays finite — so `w` stays 1.0,
        // the depth guard passes, and only a lateral finiteness check can
        // catch it.
        let blown_out = RenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_scale(glam::Vec3::new(
                1e20, 1.0, 1.0,
            ))),
            vertices: vec![[1e20, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            color: [0.0, 1.0, 0.0],
        };

        // When: baked.
        let baked = bake_scene_to_vertices(std::slice::from_ref(&blown_out));

        // Then: skipped — 1e20 * 1e20 overflows f32 to infinity, and an
        // infinite NDC vertex would poison the uploaded buffer.
        assert!(
            baked.is_empty(),
            "an infinite projected position must skip, got {baked:?}"
        );
        assert!(
            baked.iter().all(|v| v.is_finite()),
            "no bake output may carry inf/NaN, got {baked:?}"
        );
    }
}
