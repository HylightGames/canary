//! Orthographic camera plus ortho bake: the 2D specialization of the bridge.
//!
//! [`OrthographicCamera`] names how much of the world fits vertically on
//! screen; the entity's [`GlobalTransform`] supplies position and rotation
//! (a 2D game conventionally lives in one plane — see ADR 0017 — so the
//! camera entity typically carries the identity rotation and a `z` that
//! puts the room inside `[near, far]`). [`ActiveCamera`] selects which
//! camera renders when several exist. [`resolve_ortho_camera`] folds all of
//! that into one [`OrthoView`]; the `bake_*_ortho*` functions project
//! through it.
//!
//! # Why the camera lives here, not in `canary-render`
//!
//! `canary-render` holds a zero-dependency invariant (checked mechanically,
//! not just claimed — see `docs/architecture/rendering.md`): giving it an
//! ECS component plus a `glam` matrix type would end the guarantee that a
//! game build compiles only the backend crates it names. The bridge already
//! couples ECS, math, and projection, so the camera component is bridge
//! surface. Rejected: a raw `Mat4` resource (no entity to attach gameplay
//! behavior to, no multi-camera selection story) and a 3D FOV camera (this
//! slice proves the 2D sample; perspective stays exactly where it was).
//!
//! # Projection and the Y-flip convention
//!
//! With `view` the inverse of the camera's world matrix, every corner goes
//! world → view exactly like the soup bake, then orthographic (no divide):
//! `ndc_x = (view_x / half_height) / aspect`,
//! `ndc_y = -(view_y / half_height)`. The negate is the same Y-flip the
//! soup bake documents (Vulkan NDC is Y-down, object space Y-up). The
//! sample's world is Y-DOWN (screen convention: `+y` is down), so it
//! negates `y` once at spawn into `Transform` — after that one negate, the
//! bake's flip renders world-down as screen-down correctly.
//!
//! # Invalid state: loud in dev, legacy in release
//!
//! A non-positive or non-finite `half_height`, non-finite or inverted
//! `near`/`far`, a non-finite view matrix, or a non-positive/non-finite
//! aspect is a *caller* bug, not game content — so the ortho bakes
//! `debug_assert` it loudly and release builds fall back to the legacy
//! fixed-perspective bake with the caller's aspect, UNCHANGED (the legacy
//! functions are not refactored by this module: the existing pixel tests
//! pass unmodified). A missing camera, a stale [`ActiveCamera`] with no
//! live fallback, or an entity missing either component is *absence*, not
//! invalidity: [`resolve_ortho_camera`] returns `None` and the scheduled
//! ortho systems early-return, leaving the legacy frames byte-identical.

use canary_ecs::{Entity, World};
use canary_transform::GlobalTransform;
use glam::{Mat4, Vec3};

use crate::extract::{
    bake_scene_to_vertices_with_aspect_into, BakeScratch, RenderItem, FLOATS_PER_VERTEX,
};
use crate::textured_renderable::{
    bake_textured_scene_to_vertices_with_aspect, TexturedRenderItem, FLOATS_PER_TEXTURED_VERTEX,
};

/// A 2D orthographic camera: how much of the world fits on screen.
///
/// Attached to an entity alongside [`GlobalTransform`] (position and
/// rotation come from the entity — the camera looks down its local `-Z`
/// with `+Y` up, the conventional 2D setup being identity rotation in
/// the room plane). All fields public: construction is infallible and
/// validation is [`OrthographicCamera::is_valid`]'s job at resolve/bake
/// time, so a half-built prefab surfaces as a skipped camera (legacy
/// fallback), never a spawn-time error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrthographicCamera {
    /// Half the visible world height, in world units: `view_y` in
    /// `[-half_height, half_height]` maps to the full NDC vertical range
    /// (before the aspect widening on `x`). Must be positive and finite.
    pub half_height: f32,
    /// Closest visible view-space depth (`-view_z`), in world units:
    /// corners nearer than this clip. Must be finite and below `far`.
    /// The default `0.0` admits geometry coplanar with the camera — the
    /// 2D sample's camera sits in the room plane — while anything
    /// strictly behind still clips.
    pub near: f32,
    /// Farthest visible view-space depth (`-view_z`), in world units.
    /// Must be finite and above `near`.
    pub far: f32,
}

impl OrthographicCamera {
    /// Names the vertical half-extent plus the depth range. Stores
    /// as-is: no validation, so construction stays infallible and bad
    /// values surface at resolve/bake time (loud in dev, legacy fallback
    /// in release).
    pub fn new(half_height: f32, near: f32, far: f32) -> Self {
        Self {
            half_height,
            near,
            far,
        }
    }

    /// A camera framing a square arena of `half_extent_px` world units:
    /// `half_height` equals the arena half-extent (vertical fits exactly;
    /// wider targets show more horizontally via the aspect term), `near`
    /// `0.0` (coplanar geometry stays visible), `far` generous for
    /// layered 2D (`1000.0` — ortho has no depth-precision reason to be
    /// tight).
    pub fn from_arena_half_extent(half_extent_px: f32) -> Self {
        Self {
            half_height: half_extent_px,
            near: 0.0,
            far: 1000.0,
        }
    }

    /// Whether this camera can project: positive finite `half_height`,
    /// finite `near`/`far`, `near` strictly below `far`. Everything else
    /// is invalid state (dev-loud, release-legacy — see this module's
    /// docs), never a panic.
    pub fn is_valid(&self) -> bool {
        self.half_height > 0.0
            && self.half_height.is_finite()
            && self.near.is_finite()
            && self.far.is_finite()
            && self.near < self.far
    }
}

impl Default for OrthographicCamera {
    /// A unit camera: `half_height` 1, `near` 0, `far` 1000 — valid, so a
    /// default-constructed camera renders rather than falling back.
    fn default() -> Self {
        Self::from_arena_half_extent(1.0)
    }
}

/// Selects the rendering camera: holds the entity whose
/// [`OrthographicCamera`] plus [`GlobalTransform`] project the frame.
///
/// A resource (at most one per world). Stale (despawned entity, or one
/// missing either component) is absence, not an error: resolution falls
/// through to the first valid camera in query order, then to `None`
/// (legacy bake) when no valid camera exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveCamera(pub Entity);

/// One resolved camera: the view matrix plus the validated parameters.
///
/// Produced by [`resolve_ortho_camera`], consumed by the `bake_*_ortho*`
/// functions. Plain data — no world borrow — so it crosses the
/// resolve/bake boundary (and lives across the manual extract→bake→draw
/// sequence binaries run) like [`RenderItem`] does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrthoView {
    /// Inverse of the camera entity's world matrix: world → view.
    pub view: Mat4,
    /// Validated vertical half-extent in world units (positive, finite).
    pub half_height: f32,
    /// Validated near depth (finite, below `far`).
    pub near: f32,
    /// Validated far depth (finite, above `near`).
    pub far: f32,
}

/// Resolves which camera projects the frame: the [`ActiveCamera`]
/// entity when it carries a valid [`OrthographicCamera`] plus
/// [`GlobalTransform`], else the first valid camera in query order, else
/// `None` (the caller bakes legacy).
///
/// A valid component pair with a non-invertible (or non-finite-inverse)
/// world matrix still resolves to `None`: a degenerate camera transform
/// projects nothing definable, so legacy fallback beats garbage pixels.
/// `None` is absence — the scheduled ortho systems early-return on it,
/// leaving camera-less worlds byte-identical.
pub fn resolve_ortho_camera(world: &World) -> Option<OrthoView> {
    if let Some(active) = world.resource::<ActiveCamera>() {
        let entity = active.0;
        if let (Some(global), Some(camera)) = (
            world.get::<GlobalTransform>(entity),
            world.get::<OrthographicCamera>(entity),
        ) {
            if let Some(view) = checked_view(global, camera) {
                return Some(view);
            }
        }
        // Stale or invalid active camera: fall through to the first
        // valid camera rather than blinding the frame.
    }
    for (_, global, camera) in world.query2::<GlobalTransform, OrthographicCamera>() {
        if let Some(view) = checked_view(global, camera) {
            return Some(view);
        }
    }
    None
}

/// Validates `camera` and inverts `global`, returning `None` for invalid
/// parameters or a non-finite inverse.
fn checked_view(global: &GlobalTransform, camera: &OrthographicCamera) -> Option<OrthoView> {
    if !camera.is_valid() {
        return None;
    }
    let view = global.matrix().inverse();
    if !view.to_cols_array().iter().all(|v| v.is_finite()) {
        return None;
    }
    Some(OrthoView {
        view,
        half_height: camera.half_height,
        near: camera.near,
        far: camera.far,
    })
}

/// Whether the resolved view plus the caller's aspect can project:
/// finite view matrix, positive finite `half_height`, positive finite
/// aspect. `near`/`far` already passed [`OrthographicCamera::is_valid`]
/// inside [`resolve_ortho_camera`]; hand-built views re-check them via
/// `near < far` here so direct bake callers get the same fallback.
fn ortho_params_valid(view: &OrthoView, aspect: f32) -> bool {
    view.half_height > 0.0
        && view.half_height.is_finite()
        && view.near.is_finite()
        && view.far.is_finite()
        && view.near < view.far
        && view.view.to_cols_array().iter().all(|v| v.is_finite())
        && aspect > 0.0
        && aspect.is_finite()
}

/// One triangle in view space awaiting the painter sort: its view-space
/// depth (the sort key — larger is farther), its three view-space
/// corners (the project inputs), and the per-vertex payload the emit
/// closure carries (color triples on the soup path, UV triples on the
/// textured path — the two ortho bakes share the sort through
/// [`sort_and_emit`] and keep their own payloads).
#[derive(Debug, Clone, PartialEq)]
struct OrthoTriangle<const N: usize> {
    depth: f32,
    corners: [Vec3; 3],
    payload: [[f32; N]; 3],
}

/// Sorts far-to-near (stable: equal depths keep extract order) and emits
/// projected vertices, skipping invalid triangles whole.
///
/// Shared by the soup and textured ortho bakes: the sort key and the
/// per-triangle validity rule (a partial triangle would misalign the
/// soup) are one concept, one function. `emit` projects one triangle's
/// corners and appends its vertices only when all three project finite —
/// returning without appending is the whole-triangle skip.
fn sort_and_emit<const N: usize>(
    triangles: &mut [OrthoTriangle<N>],
    out: &mut Vec<f32>,
    floats_per_vertex: usize,
    mut emit: impl FnMut(&OrthoTriangle<N>, &mut Vec<f32>),
) {
    // `total_cmp` gives a deterministic order for every `f32` bit
    // pattern; `sort_by` is stable, so coplanar triangles (the whole 2D
    // sample) emit in extract order.
    triangles.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    out.clear();
    out.reserve(triangles.len() * 3 * floats_per_vertex);
    for triangle in triangles.iter() {
        emit(triangle, out);
    }
}

/// Bakes soup `items` through `view` into NDC-space vertex floats
/// (`x, y, r, g, b` per vertex) for a target of `aspect` (width/height).
///
/// Orthographic: no perspective divide — `ndc_x` scales view-`x` by the
/// half-height and aspect, `ndc_y` is the negated view-`y` over the
/// half-height (the Y-flip convention). Triangles outside `[near, far]`
/// depth, with non-finite corners, or projecting non-finite skip whole.
/// Invalid camera state (`debug_assert` loudly) falls back to the legacy
/// fixed-perspective bake with the same aspect, UNCHANGED.
///
/// For non-square targets pass the real ratio; the scheduled ortho
/// systems bake square (binaries needing exact aspect bake explicitly —
/// see `collectathon`'s frame path).
pub fn bake_scene_to_vertices_ortho(
    items: &[RenderItem],
    view: OrthoView,
    aspect: f32,
) -> Vec<f32> {
    let mut out = Vec::new();
    bake_scene_to_vertices_ortho_into(items, view, aspect, &mut out);
    out
}

/// Bakes soup `items` through `view` into `out`, reusing its buffer.
///
/// Value-identical to [`bake_scene_to_vertices_ortho`] on every call:
/// `out` is fully cleared and refilled, so last tick's frame cannot leak
/// in; growth only `reserve`s the delta. Invalid state falls back to the
/// legacy bake exactly like the fresh path (same `debug_assert`, same
/// square-on-bad-aspect contract — the legacy function owns that guard).
pub fn bake_scene_to_vertices_ortho_into(
    items: &[RenderItem],
    view: OrthoView,
    aspect: f32,
    out: &mut Vec<f32>,
) {
    debug_assert!(
        ortho_params_valid(&view, aspect),
        "invalid ortho camera state (half_height {}, near {}, far {}, aspect {aspect}): \
         falling back to the legacy perspective bake",
        view.half_height,
        view.near,
        view.far,
    );
    if !ortho_params_valid(&view, aspect) {
        let mut scratch = BakeScratch::default();
        bake_scene_to_vertices_with_aspect_into(items, aspect, out, &mut scratch);
        return;
    }
    let mut triangles: Vec<OrthoTriangle<3>> = Vec::new();
    for item in items {
        debug_assert!(
            item.color.iter().all(|c| c.is_finite()),
            "non-finite Renderable color uploads as undefined pixels"
        );
        let world_matrix = item.global.matrix();
        for chunk in item.vertices.chunks_exact(3) {
            let corners = [
                view.view
                    .transform_point3(world_matrix.transform_point3(Vec3::from(chunk[0]))),
                view.view
                    .transform_point3(world_matrix.transform_point3(Vec3::from(chunk[1]))),
                view.view
                    .transform_point3(world_matrix.transform_point3(Vec3::from(chunk[2]))),
            ];
            if corners.iter().any(|v| !v.is_finite()) {
                continue;
            }
            let depths = [-corners[0].z, -corners[1].z, -corners[2].z];
            if depths
                .iter()
                .any(|d| !d.is_finite() || *d < view.near || *d > view.far)
            {
                continue;
            }
            let depth = (depths[0] + depths[1] + depths[2]) / 3.0;
            triangles.push(OrthoTriangle {
                depth,
                corners,
                payload: [item.color, item.color, item.color],
            });
        }
    }
    let half_height = view.half_height;
    sort_and_emit(&mut triangles, out, FLOATS_PER_VERTEX, |triangle, out| {
        let mut projected = [(0.0f32, 0.0f32); 3];
        for (corner, slot) in triangle.corners.iter().zip(projected.iter_mut()) {
            let ndc_x = (corner.x / half_height) / aspect;
            let ndc_y = -(corner.y / half_height);
            if !ndc_x.is_finite() || !ndc_y.is_finite() {
                return;
            }
            *slot = (ndc_x, ndc_y);
        }
        for ((ndc_x, ndc_y), color) in projected.iter().zip(triangle.payload.iter()) {
            out.extend_from_slice(&[*ndc_x, *ndc_y]);
            out.extend_from_slice(color);
        }
    });
}

/// Bakes textured `items` through `view` into NDC-space vertex floats
/// (`x, y, u, v` per vertex) for a target of `aspect`.
///
/// The same ortho projection as [`bake_scene_to_vertices_ortho`] with
/// each vertex's UV carried through untouched (UVs are data, not
/// geometry). Same skip rules, same invalid-state fallback — to the
/// legacy TEXTURED bake, UNCHANGED.
pub fn bake_textured_scene_to_vertices_ortho(
    items: &[TexturedRenderItem],
    view: OrthoView,
    aspect: f32,
) -> Vec<f32> {
    let mut out = Vec::new();
    bake_textured_scene_to_vertices_ortho_into(items, view, aspect, &mut out);
    out
}

/// Bakes textured `items` through `view` into `out`, reusing its buffer.
///
/// Value-identical to [`bake_textured_scene_to_vertices_ortho`] on every
/// call; invalid state falls back to the legacy textured bake exactly
/// like the fresh path.
pub fn bake_textured_scene_to_vertices_ortho_into(
    items: &[TexturedRenderItem],
    view: OrthoView,
    aspect: f32,
    out: &mut Vec<f32>,
) {
    debug_assert!(
        ortho_params_valid(&view, aspect),
        "invalid ortho camera state (half_height {}, near {}, far {}, aspect {aspect}): \
         falling back to the legacy textured bake",
        view.half_height,
        view.near,
        view.far,
    );
    if !ortho_params_valid(&view, aspect) {
        *out = bake_textured_scene_to_vertices_with_aspect(items, aspect);
        return;
    }
    let mut triangles: Vec<OrthoTriangle<2>> = Vec::new();
    for item in items {
        debug_assert_eq!(
            item.vertices.len(),
            item.uvs.len(),
            "extract guarantees parallel positions and UVs; a mismatch is an engine bug, not game content"
        );
        debug_assert!(
            item.uvs.iter().all(|uv| uv.iter().all(|c| c.is_finite())),
            "non-finite UV samples undefined texels"
        );
        let world_matrix = item.global.matrix();
        for (position_chunk, uv_chunk) in
            item.vertices.chunks_exact(3).zip(item.uvs.chunks_exact(3))
        {
            let corners = [
                view.view
                    .transform_point3(world_matrix.transform_point3(Vec3::from(position_chunk[0]))),
                view.view
                    .transform_point3(world_matrix.transform_point3(Vec3::from(position_chunk[1]))),
                view.view
                    .transform_point3(world_matrix.transform_point3(Vec3::from(position_chunk[2]))),
            ];
            if corners.iter().any(|v| !v.is_finite()) {
                continue;
            }
            let depths = [-corners[0].z, -corners[1].z, -corners[2].z];
            if depths
                .iter()
                .any(|d| !d.is_finite() || *d < view.near || *d > view.far)
            {
                continue;
            }
            let depth = (depths[0] + depths[1] + depths[2]) / 3.0;
            triangles.push(OrthoTriangle {
                depth,
                corners,
                payload: [uv_chunk[0], uv_chunk[1], uv_chunk[2]],
            });
        }
    }
    let half_height = view.half_height;
    sort_and_emit(
        &mut triangles,
        out,
        FLOATS_PER_TEXTURED_VERTEX,
        |triangle, out| {
            let mut projected = [((0.0f32, 0.0f32), [0.0f32; 2]); 3];
            for ((corner, uv), slot) in triangle
                .corners
                .iter()
                .zip(triangle.payload.iter())
                .zip(projected.iter_mut())
            {
                let ndc_x = (corner.x / half_height) / aspect;
                let ndc_y = -(corner.y / half_height);
                if !ndc_x.is_finite() || !ndc_y.is_finite() {
                    return;
                }
                *slot = ((ndc_x, ndc_y), *uv);
            }
            for ((ndc_x, ndc_y), uv) in &projected {
                out.extend_from_slice(&[*ndc_x, *ndc_y, uv[0], uv[1]]);
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_transform::Transform;

    /// Asserts two `f32` values agree within `1e-5`.
    fn assert_approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "expected {expected}, got {actual}"
        );
    }

    /// One unit right-triangle at identity with a flat color.
    fn unit_triangle(color: [f32; 3]) -> RenderItem {
        RenderItem {
            global: GlobalTransform::default(),
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            color,
        }
    }

    /// Identity view, `half_height` 2, square aspect: the shared ortho
    /// fixture (object unit = half an NDC unit).
    fn identity_view() -> OrthoView {
        OrthoView {
            view: Mat4::IDENTITY,
            half_height: 2.0,
            near: 0.0,
            far: 1000.0,
        }
    }

    #[test]
    fn ortho_projects_with_y_flip_and_no_divide() {
        let item = unit_triangle([1.0, 0.0, 0.0]);

        let baked = bake_scene_to_vertices_ortho(std::slice::from_ref(&item), identity_view(), 1.0);

        assert_eq!(baked.len(), 15);
        assert_approx_eq(baked[0], 0.0);
        assert_approx_eq(baked[1], 0.0);
        assert_approx_eq(baked[5], 0.5);
        assert_approx_eq(baked[6], 0.0);
        assert_approx_eq(baked[10], 0.0);
        assert_approx_eq(baked[11], -0.5);
        for vertex in baked.chunks_exact(5) {
            assert_approx_eq(vertex[2], 1.0);
            assert_approx_eq(vertex[3], 0.0);
            assert_approx_eq(vertex[4], 0.0);
        }
    }

    #[test]
    fn ortho_scales_x_by_aspect() {
        let item = unit_triangle([1.0, 0.0, 0.0]);

        let baked = bake_scene_to_vertices_ortho(std::slice::from_ref(&item), identity_view(), 2.0);

        assert_approx_eq(baked[5], 0.25);
        assert_approx_eq(baked[11], -0.5);
    }

    #[test]
    fn ortho_view_follows_camera_translation() {
        let mut world = World::new();
        let camera_entity = world.spawn();
        world
            .insert(
                camera_entity,
                Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            )
            .expect("fresh entity accepts Transform");
        world
            .insert(
                camera_entity,
                GlobalTransform::from_matrix(Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0))),
            )
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(camera_entity, OrthographicCamera::new(2.0, 0.0, 1000.0))
            .expect("fresh entity accepts OrthographicCamera");
        world.insert_resource(ActiveCamera(camera_entity));

        let view = resolve_ortho_camera(&world).expect("one valid camera resolves");
        let item = unit_triangle([1.0, 0.0, 0.0]);
        let baked = bake_scene_to_vertices_ortho(std::slice::from_ref(&item), view, 1.0);

        assert_approx_eq(baked[0], -0.5);
        assert_approx_eq(baked[1], 0.0);
    }

    #[test]
    fn resolve_prefers_active_over_first_in_query_order() {
        let mut world = World::new();
        for _ in 0..2 {
            let entity = world.spawn();
            world
                .insert(entity, GlobalTransform::from_matrix(Mat4::IDENTITY))
                .expect("fresh entity accepts GlobalTransform");
            world
                .insert(entity, OrthographicCamera::new(2.0, 0.0, 1000.0))
                .expect("fresh entity accepts OrthographicCamera");
        }
        let first = resolve_ortho_camera(&world).expect("a camera exists");
        assert_approx_eq(first.half_height, 2.0);

        let second = world
            .query::<OrthographicCamera>()
            .nth(1)
            .map(|(entity, _)| entity)
            .expect("two cameras exist");
        world
            .get_mut::<OrthographicCamera>(second)
            .expect("second camera exists")
            .half_height = 7.0;
        world.insert_resource(ActiveCamera(second));
        let resolved = resolve_ortho_camera(&world).expect("active camera resolves");
        assert_approx_eq(resolved.half_height, 7.0);
    }

    #[test]
    fn stale_active_falls_through_to_first_valid_camera() {
        let mut world = World::new();
        let live = world.spawn();
        world
            .insert(live, GlobalTransform::from_matrix(Mat4::IDENTITY))
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(live, OrthographicCamera::new(3.0, 0.0, 1000.0))
            .expect("fresh entity accepts OrthographicCamera");
        let stale = world.spawn();
        world.insert_resource(ActiveCamera(stale));
        let resolved = resolve_ortho_camera(&world).expect("live camera covers stale active");
        assert_approx_eq(resolved.half_height, 3.0);
    }

    #[test]
    fn no_camera_resolves_to_none() {
        let world = World::new();
        assert!(
            resolve_ortho_camera(&world).is_none(),
            "a camera-less world resolves to None (legacy bake)"
        );
    }

    #[test]
    fn invalid_camera_resolves_to_none() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, GlobalTransform::from_matrix(Mat4::IDENTITY))
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(entity, OrthographicCamera::new(0.0, 0.0, 1000.0))
            .expect("fresh entity accepts OrthographicCamera");
        assert!(
            resolve_ortho_camera(&world).is_none(),
            "a zero half_height is invalid state, not a camera"
        );
    }

    #[test]
    #[should_panic(expected = "invalid ortho camera state")]
    #[cfg_attr(
        not(debug_assertions),
        ignore = "release builds fall back to legacy instead of panicking -- see below"
    )]
    fn invalid_half_height_fails_loudly_in_debug() {
        let item = unit_triangle([1.0, 0.0, 0.0]);
        let bad = OrthoView {
            half_height: 0.0,
            ..identity_view()
        };
        let _ = bake_scene_to_vertices_ortho(std::slice::from_ref(&item), bad, 1.0);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn invalid_ortho_state_falls_back_to_legacy_in_release() {
        use crate::extract::bake_scene_to_vertices_with_aspect;

        let item = unit_triangle([1.0, 0.0, 0.0]);
        let items = std::slice::from_ref(&item);
        let expected = bake_scene_to_vertices_with_aspect(items, 1.0);
        let bad_views = [
            OrthoView {
                half_height: 0.0,
                ..identity_view()
            },
            OrthoView {
                half_height: -2.0,
                ..identity_view()
            },
            OrthoView {
                half_height: f32::NAN,
                ..identity_view()
            },
            OrthoView {
                view: Mat4::NAN,
                ..identity_view()
            },
        ];
        for bad in bad_views {
            let baked = bake_scene_to_vertices_ortho(items, bad, 1.0);
            assert_eq!(
                baked, expected,
                "invalid ortho state must bake exactly like legacy"
            );
            assert!(
                baked.iter().all(|v| v.is_finite()),
                "fallback must not leak inf/NaN into the buffer"
            );
        }
        let expected_square = bake_scene_to_vertices_with_aspect(items, 1.0);
        for bad_aspect in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let baked = bake_scene_to_vertices_ortho(items, identity_view(), bad_aspect);
            assert_eq!(
                baked, expected_square,
                "aspect {bad_aspect} must bake exactly like the square legacy default"
            );
        }
    }

    #[test]
    fn ortho_into_reused_buffer_matches_fresh_bake() {
        let item = unit_triangle([1.0, 0.0, 0.0]);
        let items = std::slice::from_ref(&item);
        let mut out = vec![f32::NAN; 1024];
        bake_scene_to_vertices_ortho_into(items, identity_view(), 1.0, &mut out);

        assert_eq!(
            out,
            bake_scene_to_vertices_ortho(items, identity_view(), 1.0)
        );
        assert_eq!(out.len(), 3 * FLOATS_PER_VERTEX);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn ortho_clips_outside_near_far_and_sorts_far_first() {
        let near_item = unit_triangle([1.0, 0.0, 0.0]);
        let mut far_item = unit_triangle([0.0, 0.0, 1.0]);
        far_item.global =
            GlobalTransform::from_matrix(Mat4::from_translation(Vec3::new(0.0, 0.0, -4.0)));
        let baked = bake_scene_to_vertices_ortho(&[near_item, far_item], identity_view(), 1.0);
        assert_eq!(baked.len(), 30);
        for vertex in baked[..15].chunks_exact(5) {
            assert_approx_eq(vertex[4], 1.0);
        }
        let mut beyond = unit_triangle([1.0, 0.0, 0.0]);
        beyond.global =
            GlobalTransform::from_matrix(Mat4::from_translation(Vec3::new(0.0, 0.0, -2000.0)));
        let clipped =
            bake_scene_to_vertices_ortho(std::slice::from_ref(&beyond), identity_view(), 1.0);
        assert!(
            clipped.is_empty(),
            "geometry beyond far must clip, got {clipped:?}"
        );
        let mut behind = unit_triangle([1.0, 0.0, 0.0]);
        behind.global =
            GlobalTransform::from_matrix(Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0)));
        let clipped =
            bake_scene_to_vertices_ortho(std::slice::from_ref(&behind), identity_view(), 1.0);
        assert!(
            clipped.is_empty(),
            "geometry behind the camera must clip, got {clipped:?}"
        );
    }

    #[test]
    fn textured_ortho_projects_and_carries_uvs() {
        let item = TexturedRenderItem {
            global: GlobalTransform::default(),
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        };

        let baked = bake_textured_scene_to_vertices_ortho(
            std::slice::from_ref(&item),
            identity_view(),
            1.0,
        );

        assert_eq!(baked.len(), 12);
        assert_approx_eq(baked[0], 0.0);
        assert_approx_eq(baked[1], 0.0);
        assert_approx_eq(baked[4], 0.5);
        assert_approx_eq(baked[5], 0.0);
        assert_approx_eq(baked[8], 0.0);
        assert_approx_eq(baked[9], -0.5);
        assert_eq!(
            vec![
                (baked[2], baked[3]),
                (baked[6], baked[7]),
                (baked[10], baked[11])
            ],
            vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)],
            "UVs must survive the ortho bake untouched"
        );
    }
}
