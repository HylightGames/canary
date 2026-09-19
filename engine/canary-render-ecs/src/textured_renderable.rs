//! File-loaded textured renderable: [`TexturedRenderable`], textured
//! soup expansion, textured extraction, textured CPU-bake, and the
//! textured frame.
//!
//! This module is Phase 3b's bridge slice, mirroring
//! [`mesh_renderable`](crate::mesh_renderable)'s shape one level up:
//! where the mesh path expands indices to positions and bakes `x, y,
//! r, g, b`, this path additionally carries UVs through the same
//! extract → bake chain and emits `x, y, u, v` into its own
//! [`BakedTexturedFrame`] — a separate resource (not an extension of
//! [`BakedFrame`](crate::BakedFrame)) because the vertex stride,
//! attribute layout, pipeline, and bound texture all differ from the
//! soup path. Sharing one frame type across two layouts would corrupt
//! both draws; the separate resource keeps each draw's contract exact.
//!
//! # What is deferred, explicitly
//!
//! One texture per draw (the caller of
//! [`draw_textured_frame`](crate::draw_textured_frame) supplies which
//! one — multi-texture batching and per-entity material selection are
//! the general materials system's scope), no lighting (normals stay
//! ignored, as in the mesh path), no mipmaps/sRGB/sampler choice (the
//! RHI offers its one default). An entity whose mesh carries no UVs is
//! skipped like a stale handle — a *game-content* condition, not an
//! engine panic — because inventing UVs for it would be guessing at
//! author intent.

use canary_assets::{AssetHandle, AssetStore, Mesh, Texture};
use canary_ecs::World;
use canary_transform::GlobalTransform;
use glam::Vec3;

/// Distance the virtual camera sits behind the world origin, in world
/// units — reused verbatim from the soup bake (`extract.rs`), because
/// the textured bake is the same projection with UVs carried along, not
/// a second camera.
const CAMERA_DISTANCE: f32 = 3.2;

/// Focal length of the hand-rolled perspective projection — same
/// source and reason as [`CAMERA_DISTANCE`].
const FOCAL_LENGTH: f32 = 2.2;

/// Default target aspect ratio (width divided by height) — the neutral
/// square default, matching the soup bake's contract.
const DEFAULT_ASPECT_RATIO: f32 = 1.0;

/// Number of `f32`s emitted per baked textured vertex: NDC `x`, `y`
/// plus carried `u`, `v`.
///
/// Matches the textured shader contract exactly: one
/// [`Float32x2`](canary_render::VertexFormat::Float32x2) position
/// attribute followed by one [`Float32x2`](canary_render::VertexFormat::Float32x2)
/// UV attribute. [`BakedTexturedFrame::vertex_count`] derives from this
/// constant so the layout has a single source of truth.
const FLOATS_PER_TEXTURED_VERTEX: usize = 4;

/// Camera-space depths at or below this threshold are treated as
/// on-or-behind the camera plane — same guard, same value, same
/// rationale as the soup bake's `MIN_CAMERA_DEPTH`.
const MIN_CAMERA_DEPTH: f32 = 1e-6;

/// A file-loaded textured mesh attached to one entity: which geometry
/// plus which image, both by handle.
///
/// `mesh` names the indexed geometry in the [`AssetStore<Mesh>`]
/// resource; `texture` names the RGBA8 image in the
/// [`AssetStore<Texture>`] resource. No flat color: the texture *is*
/// the color. Handles stay two words to copy and the stores stay the
/// single owners, for the same reason [`MeshRenderable`](crate::MeshRenderable)
/// holds a handle rather than inline vertices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TexturedRenderable {
    /// Generational handle into the [`AssetStore<Mesh>`] resource
    /// holding this entity's geometry. Resolved per extract; stale
    /// handles skip.
    pub mesh: AssetHandle<Mesh>,
    /// Generational handle into the [`AssetStore<Texture>`] resource
    /// holding this entity's image. Resolved per extract; stale handles
    /// skip.
    pub texture: AssetHandle<Texture>,
}

impl TexturedRenderable {
    /// Names the mesh asset plus the texture asset.
    ///
    /// Stores both handles as-is: no validation, no store lookup —
    /// handle liveness is [`extract_textured_scene`]'s job at bake
    /// time, so construction stays infallible and a removed asset
    /// surfaces as a skipped entity, not a spawn-time error.
    pub fn new(mesh: AssetHandle<Mesh>, texture: AssetHandle<Texture>) -> Self {
        Self { mesh, texture }
    }
}

/// Expands an indexed [`Mesh`] into object-space position/UV pairs, or
/// `None` when the mesh carries no UVs.
///
/// Maps each index through the mesh's positions *and* UVs in index
/// order, so the output is `mesh.triangle_count() * 3` pairs ready for
/// [`TexturedRenderItem`]. `None` (rather than zero-filled UVs) when
/// [`Mesh::uvs`] is `None` because a mesh without texture coordinates
/// has no defined sampling: filling `(0, 0)` everywhere would smear
/// one texel across the entity and present it as textured rendering.
/// The caller ([`extract_textured_scene`]) skips such entities — the
/// same game-content doctrine under which stale handles skip.
///
/// Direct indexing (not `get` + skip) is sound here, not optimistic:
/// [`Mesh::indices`] documents that every index is bounds-checked at
/// load time, and positions/UVs share one length by the same load-time
/// contract, so indexing with them cannot panic for any successfully
/// loaded mesh.
pub fn expand_mesh_to_textured_soup(mesh: &Mesh) -> Option<Vec<([f32; 3], [f32; 2])>> {
    let uvs = mesh.uvs()?;
    let positions = mesh.positions();
    Some(
        mesh.indices()
            .iter()
            .map(|index| {
                let vertex = *index as usize;
                (positions[vertex], uvs[vertex])
            })
            .collect(),
    )
}

/// One textured entity's contribution to a frame: its world transform
/// plus its object-space triangle soup with per-vertex UVs, copied out
/// of the [`World`](canary_ecs::World).
///
/// A snapshot, not a borrow, for the same reason
/// [`RenderItem`](crate::RenderItem) is: extraction clones the data so
/// that baking (pure math, sortable, GPU-free) never holds a
/// [`World`](canary_ecs::World) borrow. Positions and UVs are parallel
/// arrays of equal length — enforced by construction here, which is
/// what lets bake zip them without a mismatch case.
#[derive(Debug, Clone, PartialEq)]
pub struct TexturedRenderItem {
    /// The entity's cached world-space matrix, copied from its
    /// [`GlobalTransform`] at extract time.
    ///
    /// [`GlobalTransform`]: canary_transform::GlobalTransform
    pub global: GlobalTransform,
    /// Object-space vertex positions as consecutive triangles: every
    /// three entries form one triangle.
    pub vertices: Vec<[f32; 3]>,
    /// Per-vertex texture coordinates, parallel to [`TexturedRenderItem::vertices`]:
    /// `uvs[i]` is the coordinate of `vertices[i]`.
    pub uvs: Vec<[f32; 2]>,
}

/// Reads every textured entity out of `world` into a snapshot [`Vec`].
///
/// Queries the [`World::query2`] intersection of [`GlobalTransform`]
/// and [`TexturedRenderable`], resolves each mesh handle against the
/// [`AssetStore<Mesh>`] resource and each texture handle against the
/// [`AssetStore<Texture>`] resource, expands live meshes via
/// [`expand_mesh_to_textured_soup`], and drops everything else: stale
/// handles resolve to `None` and filter out, meshes without UVs expand
/// to `None` and filter out, and a missing store resource of *either*
/// kind yields an empty snapshot (a soup-only world legitimately has
/// neither — not an error).
///
/// [`World::query2`]: canary_ecs::World::query2
/// [`GlobalTransform`]: canary_transform::GlobalTransform
pub fn extract_textured_scene(world: &World) -> Vec<TexturedRenderItem> {
    let Some(mesh_store) = world.resource::<AssetStore<Mesh>>() else {
        return Vec::new();
    };
    let Some(texture_store) = world.resource::<AssetStore<Texture>>() else {
        return Vec::new();
    };
    world
        .query2::<GlobalTransform, TexturedRenderable>()
        .filter_map(|(_, global, renderable)| {
            let mesh = mesh_store.get(renderable.mesh)?;
            let _ = texture_store.get(renderable.texture)?;
            let pairs = expand_mesh_to_textured_soup(mesh)?;
            let (vertices, uvs) = pairs.into_iter().unzip();
            Some(TexturedRenderItem {
                global: *global,
                vertices,
                uvs,
            })
        })
        .collect()
}

/// One frame of GPU-ready textured vertex data: NDC `x`, `y` plus
/// carried `u`, `v` per vertex (`FLOATS_PER_TEXTURED_VERTEX` floats
/// each), in painter-sorted draw order (far triangles first).
///
/// Produced by [`bake_textured_scene_to_vertices`] and consumed by
/// [`draw_textured_frame`](crate::draw_textured_frame), which uploads
/// [`BakedTexturedFrame::vertices`] as a fresh vertex buffer and binds
/// the caller-supplied texture. Plain data — no device handles, no
/// lifetimes — so it can live as an ECS resource without dragging GPU
/// types into scheduling, exactly like [`BakedFrame`](crate::BakedFrame).
#[derive(Debug, Clone, PartialEq)]
pub struct BakedTexturedFrame {
    /// Baked vertex floats: `x, y, u, v` per vertex,
    /// `4 * vertex_count()` floats total.
    pub vertices: Vec<f32>,
}

impl BakedTexturedFrame {
    /// The number of vertices in this frame: `vertices.len() / 4`.
    ///
    /// This is the `vertex_count` the RHI
    /// [`draw`](canary_render::CommandEncoder::draw) call consumes.
    /// Saturates at `u32::MAX` instead of wrapping on absurd inputs,
    /// for the same reason [`BakedFrame::vertex_count`](crate::BakedFrame::vertex_count)
    /// does.
    pub fn vertex_count(&self) -> u32 {
        u32::try_from(self.vertices.len() / FLOATS_PER_TEXTURED_VERTEX).unwrap_or(u32::MAX)
    }

    /// Whether this frame holds no vertices.
    ///
    /// Drawing an empty textured frame still submits the clear pass,
    /// so the target shows the clear color rather than stale contents;
    /// see [`draw_textured_frame`](crate::draw_textured_frame).
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

/// Bakes `items` into NDC-space vertex floats with UVs for a square target.
///
/// The same projection as the soup bake (`CAMERA_DISTANCE`,
/// `FOCAL_LENGTH`, Y-flip negate, painter-sort far-to-near,
/// behind-camera skip) applied to positions, with each vertex's UV
/// carried through untouched: UVs are data, not geometry, so no
/// transform applies to them. Triangles on or behind the camera plane
/// are skipped with their UVs — a skipped triangle emits nothing, so
/// positions and UVs can never desynchronize here.
///
/// For non-square targets, prefer
/// [`bake_textured_scene_to_vertices_with_aspect`]: this function is
/// exactly that function with the neutral square aspect.
pub fn bake_textured_scene_to_vertices(items: &[TexturedRenderItem]) -> Vec<f32> {
    bake_textured_scene_to_vertices_with_aspect(items, DEFAULT_ASPECT_RATIO)
}

/// Bakes `items` into NDC-space vertex floats with UVs for a target of
/// the given aspect ratio (width divided by height).
///
/// Identical to [`bake_textured_scene_to_vertices`] except for the
/// horizontal scale; the aspect-fallback contract (silent square
/// fallback in release, debug loudness) matches the soup bake's
/// `bake_scene_to_vertices_with_aspect` entry-for-entry, because it is
/// the same caller obligation with the same poisoning consequence.
pub fn bake_textured_scene_to_vertices_with_aspect(
    items: &[TexturedRenderItem],
    aspect_ratio: f32,
) -> Vec<f32> {
    debug_assert!(
        aspect_ratio > 0.0 && aspect_ratio.is_finite(),
        "aspect_ratio must be a positive finite width/height; got {aspect_ratio}"
    );
    let aspect_ratio = if aspect_ratio > 0.0 && aspect_ratio.is_finite() {
        aspect_ratio
    } else {
        DEFAULT_ASPECT_RATIO
    };
    /// One triangle in camera space, awaiting the painter-sort: its
    /// average depth (the sort key), its three corner positions (the
    /// project inputs), and the three UVs those corners carry.
    struct PendingTexturedTriangle {
        avg_depth: f32,
        corners: [Vec3; 3],
        uvs: [[f32; 2]; 3],
    }

    let mut triangles: Vec<PendingTexturedTriangle> = Vec::new();
    for item in items {
        debug_assert_eq!(
            item.vertices.len(),
            item.uvs.len(),
            "extract guarantees parallel positions and UVs; a mismatch is an engine bug, not game content"
        );
        let matrix = item.global.matrix();
        let mut triangle_corners = Vec::with_capacity(3);
        let mut triangle_uvs = Vec::with_capacity(3);
        for (position, uv) in item.vertices.iter().zip(item.uvs.iter()) {
            triangle_corners.push(matrix.transform_point3(Vec3::from(*position)));
            triangle_uvs.push(*uv);
            if triangle_corners.len() == 3 {
                let corners = [
                    triangle_corners[0],
                    triangle_corners[1],
                    triangle_corners[2],
                ];
                let camera_space = [
                    Vec3::new(corners[0].x, corners[0].y, corners[0].z + CAMERA_DISTANCE),
                    Vec3::new(corners[1].x, corners[1].y, corners[1].z + CAMERA_DISTANCE),
                    Vec3::new(corners[2].x, corners[2].y, corners[2].z + CAMERA_DISTANCE),
                ];
                if camera_space.iter().all(|v| v.z > MIN_CAMERA_DEPTH) {
                    let avg_depth =
                        (camera_space[0].z + camera_space[1].z + camera_space[2].z) / 3.0;
                    triangles.push(PendingTexturedTriangle {
                        avg_depth,
                        corners: camera_space,
                        uvs: [triangle_uvs[0], triangle_uvs[1], triangle_uvs[2]],
                    });
                }
                triangle_corners.clear();
                triangle_uvs.clear();
            }
        }
        // A trailing 1–2 vertices form no triangle and are dropped, the
        // same partial-tail doctrine as the soup bake's
        // `chunks_exact(3)`: extract only produces whole triangles, so a
        // tail here is impossible through the public path.
    }
    // Painter's algorithm: farthest first so nearer triangles overdraw.
    // `total_cmp` gives a deterministic order for every `f32` bit pattern.
    triangles.sort_by(|a, b| b.avg_depth.total_cmp(&a.avg_depth));

    let mut vertices = Vec::with_capacity(triangles.len() * 3 * FLOATS_PER_TEXTURED_VERTEX);
    for triangle in &triangles {
        for (corner, uv) in triangle.corners.iter().zip(triangle.uvs.iter()) {
            let ndc_x = (corner.x * FOCAL_LENGTH) / (corner.z * aspect_ratio);
            let ndc_y = -(corner.y * FOCAL_LENGTH) / corner.z;
            vertices.extend_from_slice(&[ndc_x, ndc_y, uv[0], uv[1]]);
        }
    }
    vertices
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_assets::{load_mesh, load_texture};
    use std::path::PathBuf;

    /// Checked-in quad fixture: positions (±0.5, ±0.5, 0), `TEXCOORD_0`
    /// (0,0), (1,0), (1,1), (0,1), indices `[0, 1, 2, 0, 2, 3]`.
    /// Loaded from disk — never reconstructed by hand.
    fn quad_mesh() -> Mesh {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/quad.glb");
        load_mesh(&path)
            .expect("quad fixture must load")
            .into_iter()
            .next()
            .expect("quad fixture holds one mesh")
    }

    /// Checked-in 2×2 RGBA fixture: red, green / blue, white.
    fn quad_texture() -> Texture {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/rgba2x2.png");
        load_texture(&path).expect("RGBA fixture must load")
    }

    /// One unit right-triangle with hand-held UVs: the shared pure-bake
    /// fixture (identity global, so projection math is checkable).
    fn unit_textured_triangle() -> TexturedRenderItem {
        TexturedRenderItem {
            global: GlobalTransform::default(),
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        }
    }

    /// Asserts two `f32` values agree within `1e-5`.
    fn assert_approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn expansion_carries_positions_and_uvs_through_index_order() {
        let pairs = expand_mesh_to_textured_soup(&quad_mesh()).expect("quad carries UVs");

        assert_eq!(pairs.len(), 6, "two triangles expand to six pairs");
        assert_eq!(
            pairs,
            vec![
                ([-0.5, -0.5, 0.0], [0.0, 0.0]),
                ([0.5, -0.5, 0.0], [1.0, 0.0]),
                ([0.5, 0.5, 0.0], [1.0, 1.0]),
                ([-0.5, -0.5, 0.0], [0.0, 0.0]),
                ([0.5, 0.5, 0.0], [1.0, 1.0]),
                ([-0.5, 0.5, 0.0], [0.0, 1.0]),
            ],
            "pairs must be positions+UVs resolved through indices [0,1,2, 0,2,3] in order"
        );
    }

    #[test]
    fn expansion_is_none_for_a_mesh_without_uvs() {
        // The box fixture's first primitive carries positions + indices
        // but no TEXCOORD_0 — the loader's documented `None` path.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/box.glb");
        let meshes = load_mesh(&path).expect("box fixture must load");
        let bare = meshes
            .iter()
            .find(|mesh| mesh.uvs().is_none())
            .expect("box fixture must hold a primitive without UVs");

        assert!(
            expand_mesh_to_textured_soup(bare).is_none(),
            "a mesh without UVs has no defined sampling and must expand to None, not zero-filled UVs"
        );
    }

    #[test]
    fn extract_resolves_both_handles_through_their_stores() {
        let mut world = World::new();
        let mut mesh_store = AssetStore::new();
        let mesh_handle = mesh_store.insert(quad_mesh());
        world.insert_resource(mesh_store);
        let mut texture_store = AssetStore::new();
        let texture_handle = texture_store.insert(quad_texture());
        world.insert_resource(texture_store);
        let entity = world.spawn();
        world
            .insert(entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(entity, TexturedRenderable::new(mesh_handle, texture_handle))
            .expect("fresh entity accepts TexturedRenderable");

        let items = extract_textured_scene(&world);

        assert_eq!(items.len(), 1, "the textured entity must extract");
        assert_eq!(items[0].vertices.len(), 6);
        assert_eq!(items[0].uvs.len(), 6);
        assert_eq!(items[0].uvs[1], [1.0, 0.0]);
    }

    #[test]
    fn extract_skips_stale_texture_handles_without_trapping() {
        let mut world = World::new();
        let mut mesh_store = AssetStore::new();
        let mesh_handle = mesh_store.insert(quad_mesh());
        world.insert_resource(mesh_store);
        let mut texture_store: AssetStore<Texture> = AssetStore::new();
        let texture_handle = texture_store.insert(quad_texture());
        texture_store.remove(texture_handle);
        world.insert_resource(texture_store);
        let entity = world.spawn();
        world
            .insert(entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(entity, TexturedRenderable::new(mesh_handle, texture_handle))
            .expect("fresh entity accepts TexturedRenderable");

        let items = extract_textured_scene(&world);

        assert!(
            items.is_empty(),
            "a stale texture handle must skip, not trap the frame"
        );
    }

    #[test]
    fn extract_skips_meshes_without_uvs() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/box.glb");
        let meshes = load_mesh(&path).expect("box fixture must load");
        let bare = meshes
            .iter()
            .find(|mesh| mesh.uvs().is_none())
            .expect("box fixture must hold a primitive without UVs")
            .clone();
        let mut world = World::new();
        let mut mesh_store = AssetStore::new();
        let mesh_handle = mesh_store.insert(bare);
        world.insert_resource(mesh_store);
        let mut texture_store = AssetStore::new();
        let texture_handle = texture_store.insert(quad_texture());
        world.insert_resource(texture_store);
        let entity = world.spawn();
        world
            .insert(entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(entity, TexturedRenderable::new(mesh_handle, texture_handle))
            .expect("fresh entity accepts TexturedRenderable");

        let items = extract_textured_scene(&world);

        assert!(
            items.is_empty(),
            "a mesh without UVs must skip: inventing coordinates would fake textured rendering"
        );
    }

    #[test]
    fn extract_without_either_store_yields_empty_not_a_panic() {
        // First: mesh store present, texture store absent.
        let mut world = World::new();
        let mut mesh_store = AssetStore::new();
        let mesh_handle = mesh_store.insert(quad_mesh());
        world.insert_resource(mesh_store);
        let entity = world.spawn();
        world
            .insert(entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(
                entity,
                TexturedRenderable::new(mesh_handle, AssetHandle::from_raw_parts(0, 0)),
            )
            .expect("fresh entity accepts TexturedRenderable");
        assert!(
            extract_textured_scene(&world).is_empty(),
            "no texture store means no resolvable textured entities, not a panic"
        );

        // Then: neither store present.
        let mut bare_world = World::new();
        let bare_entity = bare_world.spawn();
        bare_world
            .insert(bare_entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        bare_world
            .insert(
                bare_entity,
                TexturedRenderable::new(
                    AssetHandle::from_raw_parts(0, 0),
                    AssetHandle::from_raw_parts(0, 0),
                ),
            )
            .expect("fresh entity accepts TexturedRenderable");
        assert!(
            extract_textured_scene(&bare_world).is_empty(),
            "no stores at all still means empty, not a panic"
        );
    }

    #[test]
    fn bake_projects_positions_and_carries_uvs_untouched() {
        let item = unit_textured_triangle();

        let baked = bake_textured_scene_to_vertices(std::slice::from_ref(&item));

        // 3 vertices × 4 floats (x, y, u, v).
        assert_eq!(baked.len(), 12);
        // Identity transform + camera shift: camera-space z is 3.2
        // everywhere, focal length 2.2, square aspect — the same
        // projection the soup bake pins.
        let focal = 2.2_f32;
        let distance = 3.2_f32;
        assert_approx_eq(baked[0], 0.0);
        assert_approx_eq(baked[1], 0.0);
        assert_approx_eq(baked[4], focal / distance);
        assert_approx_eq(baked[5], 0.0);
        // Y-flip: object +Y lands at negative NDC-Y.
        assert_approx_eq(baked[8], 0.0);
        assert_approx_eq(baked[9], -(focal / distance));
        // UVs pass through byte-identical: no transform applies to them.
        assert_eq!(
            vec![
                (baked[2], baked[3]),
                (baked[6], baked[7]),
                (baked[10], baked[11]),
            ],
            vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)],
            "UVs must survive the bake untouched"
        );
    }

    #[test]
    fn bake_skips_triangles_behind_camera_with_their_uvs() {
        let behind = TexturedRenderItem {
            global: GlobalTransform::from_matrix(glam::Mat4::from_translation(glam::Vec3::new(
                0.0, 0.0, -5.0,
            ))),
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        };

        let baked = bake_textured_scene_to_vertices(std::slice::from_ref(&behind));

        assert!(
            baked.is_empty(),
            "triangles behind the camera must be skipped with their UVs, got {baked:?}"
        );
    }

    #[test]
    fn textured_frame_counts_vertices_in_fours() {
        let frame = BakedTexturedFrame {
            vertices: bake_textured_scene_to_vertices(std::slice::from_ref(
                &unit_textured_triangle(),
            )),
        };

        assert!(!frame.is_empty());
        assert_eq!(frame.vertex_count(), 3);
        assert_eq!(
            BakedTexturedFrame { vertices: vec![] }.vertex_count(),
            0,
            "an empty textured frame draws nothing"
        );
    }
}
