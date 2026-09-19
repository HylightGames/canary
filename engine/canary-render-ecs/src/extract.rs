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
const FLOATS_PER_VERTEX: usize = 5;

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
/// the guard.
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
    world
        .query2::<GlobalTransform, Renderable>()
        .filter(|(_, _, renderable)| renderable.is_valid())
        .map(|(_, global, renderable)| RenderItem {
            global: *global,
            vertices: renderable.vertices.clone(),
            color: renderable.color,
        })
        .collect()
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
/// [`GlobalTransform::matrix`]: canary_transform::GlobalTransform::matrix
pub fn bake_scene_to_vertices(items: &[RenderItem]) -> Vec<f32> {
    bake_scene_to_vertices_with_aspect(items, DEFAULT_ASPECT_RATIO)
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
    /// One triangle in camera space, awaiting the painter-sort: its average
    /// depth (the sort key), its three corner positions (the project inputs),
    /// and the flat color all three vertices share.
    struct PendingTriangle {
        avg_depth: f32,
        corners: [Vec3; 3],
        color: [f32; 3],
    }

    let mut triangles: Vec<PendingTriangle> = Vec::new();
    for item in items {
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
            if camera_space.iter().any(|v| v.z <= MIN_CAMERA_DEPTH) {
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
    triangles.sort_by(|a, b| b.avg_depth.total_cmp(&a.avg_depth));

    let mut vertices = Vec::with_capacity(triangles.len() * 3 * FLOATS_PER_VERTEX);
    for triangle in &triangles {
        for corner in &triangle.corners {
            // Spinning-cube's `project`, generalized to caller-supplied
            // aspect: perspective divide, with the Y-flip negate that keeps
            // object-space "up" visually up under Vulkan's Y-down NDC.
            let ndc_x = (corner.x * FOCAL_LENGTH) / (corner.z * aspect_ratio);
            let ndc_y = -(corner.y * FOCAL_LENGTH) / corner.z;
            vertices.extend_from_slice(&[ndc_x, ndc_y]);
            vertices.extend_from_slice(&triangle.color);
        }
    }
    vertices
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
}
