//! File-loaded mesh renderable: [`MeshRenderable`], index-to-soup expansion,
//! and mesh-aware scene extraction.
//!
//! See [`MeshRenderable`] for the target-design rationale.

use canary_assets::{AssetHandle, AssetStore, Mesh};
use canary_ecs::World;
use canary_transform::GlobalTransform;

use crate::RenderItem;

/// A file-loaded mesh attached to one entity, rendered in a flat color.
///
/// `mesh` is a generational handle into the [`AssetStore<Mesh>`] ECS
/// resource — never mesh data inline — plus the flat RGB triple this
/// entity's triangles carry into the bake, exactly like
/// [`Renderable`](crate::Renderable)'s per-entity color.
///
/// # Why a handle, not inline vertices
///
/// The asset owns the indexed geometry ([`Mesh`] keeps positions +
/// indices, shared vertices shared); the entity only names *which* mesh
/// plus *how it is tinted*. Copying the whole vertex list into every
/// entity would duplicate GPU-bound data per instance and freeze the
/// bridge to one layout the day meshes gain skins or morphs. Handles
/// stay two words to copy; the store stays the single owner.
///
/// # Why index-to-soup expansion happens at the bridge, not in the asset
///
/// The RHI has no index-buffer support — [`CommandEncoder::set_vertex_buffer`]
/// binds exactly one vertex buffer and [`CommandEncoder::draw`] consumes a
/// plain `vertex_count` (see [`Renderable`](crate::Renderable)'s docs).
/// Expanding indices into triangle soup at *load* time would bake one
/// consumer's layout into the asset and hide the vertex-duplication cost
/// of doing so; the asset stays honest (indexed) so a future indexed draw
/// path can consume it directly, and the bridge — the one place that
/// knows the soup layout — pays the expansion per extract via
/// [`expand_mesh_to_soup`]. Normals and UVs stored on the mesh are
/// ignored here for the same reason they are ignored everywhere this
/// release: no lighting inputs, no samplers (Phase 3b owns textures).
///
/// # Why stale handles skip instead of panicking
///
/// A stale or missing handle is a *game-content* condition (asset
/// removed between spawn and bake, handle held across a removal), not an
/// engine invariant violation — the same doctrine under which
/// [`extract_scene`](crate::extract_scene) skips invalid renderables.
/// [`extract_mesh_scene`] therefore drops entities whose handles do not
/// resolve (or when no [`AssetStore<Mesh>`] resource exists at all)
/// rather than trapping the frame.
///
/// [`CommandEncoder::set_vertex_buffer`]:
///     canary_render::CommandEncoder::set_vertex_buffer
/// [`CommandEncoder::draw`]: canary_render::CommandEncoder::draw
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshRenderable {
    /// Generational handle into the [`AssetStore<Mesh>`] resource holding
    /// this entity's geometry. Resolved per extract; stale handles skip.
    pub mesh: AssetHandle<Mesh>,
    /// Flat RGB color shared by every vertex of this entity, replicated
    /// into each baked vertex's color attribute at bake time (the RHI has
    /// no uniforms or materials to carry it any other way).
    pub color: [f32; 3],
}

impl MeshRenderable {
    /// Names the mesh asset plus the flat per-entity color.
    ///
    /// Stores the handle and color as-is: no validation, no store lookup —
    /// handle liveness is [`extract_mesh_scene`]'s job at bake time, so
    /// construction stays infallible and a removed asset surfaces as a
    /// skipped entity, not a spawn-time error.
    pub fn new(mesh: AssetHandle<Mesh>, color: [f32; 3]) -> Self {
        Self { mesh, color }
    }
}

/// Expands an indexed [`Mesh`] into object-space triangle soup.
///
/// Maps each index triple through the mesh's positions in index order, so
/// the output is `mesh.triangle_count() * 3` positions ready for
/// [`RenderItem`]'s soup contract. This is the one place the bridge knows
/// the soup layout, which is why the expansion lives here and not in
/// `canary-assets` (see [`MeshRenderable`]'s docs).
///
/// Direct indexing (not `get` + skip) is sound here, not optimistic:
/// [`Mesh::indices`] documents that every index is bounds-checked at load
/// time, so indexing with them cannot panic for any successfully loaded
/// mesh. A mesh that violates that contract cannot exist through the
/// public loader API.
pub fn expand_mesh_to_soup(mesh: &Mesh) -> Vec<[f32; 3]> {
    let positions = mesh.positions();
    mesh.indices()
        .iter()
        .map(|index| positions[*index as usize])
        .collect()
}

/// Reads every mesh-renderable entity out of `world` into a snapshot [`Vec`].
///
/// Queries the [`World::query2`] intersection of [`GlobalTransform`] and
/// [`MeshRenderable`], resolves each handle against the
/// [`AssetStore<Mesh>`] resource, expands live meshes via
/// [`expand_mesh_to_soup`], and drops everything else: entities missing
/// *either* component never surface from the query itself, stale handles
/// resolve to `None` and are filtered, and a missing store resource
/// yields an empty snapshot (no store, no meshes — not an error, since a
/// soup-only world legitimately has none).
///
/// # Why the store read is declared in `SystemAccess`
///
/// The scheduled mesh-bake system wraps this function (see
/// [`bake_mesh_scene_system`](crate::bake_mesh_scene_system)); its access
/// declaration adds `reads_resource::<AssetStore<Mesh>>()`, which keeps
/// the bake ordered after any writer of that resource by the scheduler's
/// conflict rules. The declaration is documentation-as-code: the read
/// happens here, the ordering guarantee lives there.
///
/// [`World::query2`]: canary_ecs::World::query2
/// [`GlobalTransform`]: canary_transform::GlobalTransform
pub fn extract_mesh_scene(world: &World) -> Vec<RenderItem> {
    let Some(store) = world.resource::<AssetStore<Mesh>>() else {
        return Vec::new();
    };
    world
        .query2::<GlobalTransform, MeshRenderable>()
        .filter_map(|(_, global, renderable)| {
            store.get(renderable.mesh).map(|mesh| RenderItem {
                global: *global,
                vertices: expand_mesh_to_soup(mesh),
                color: renderable.color,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_assets::load_mesh;
    use std::path::PathBuf;

    /// Checked-in quad fixture: one triangle primitive, positions
    /// (±0.5, ±0.5, 0), indices `[0, 1, 2, 0, 2, 3]`. Loaded from disk —
    /// never reconstructed by hand — so these tests prove the bridge
    /// against real loader output, not a parallel hand-built copy.
    fn quad_mesh() -> Mesh {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/quad.glb");
        load_mesh(&path)
            .expect("quad fixture must load")
            .into_iter()
            .next()
            .expect("quad fixture holds one mesh")
    }

    #[test]
    fn expansion_follows_index_order_into_positions() {
        let soup = expand_mesh_to_soup(&quad_mesh());

        assert_eq!(soup.len(), 6, "two triangles expand to six soup vertices");
        assert_eq!(
            soup,
            vec![
                [-0.5, -0.5, 0.0],
                [0.5, -0.5, 0.0],
                [0.5, 0.5, 0.0],
                [-0.5, -0.5, 0.0],
                [0.5, 0.5, 0.0],
                [-0.5, 0.5, 0.0],
            ],
            "soup must be positions resolved through indices [0,1,2, 0,2,3] in order"
        );
    }

    #[test]
    fn extract_resolves_handles_through_the_store_resource() {
        let mut world = World::new();
        let mut store = AssetStore::new();
        let handle = store.insert(quad_mesh());
        world.insert_resource(store);
        let entity = world.spawn();
        world
            .insert(entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(entity, MeshRenderable::new(handle, [1.0, 0.0, 0.0]))
            .expect("fresh entity accepts MeshRenderable");

        let items = extract_mesh_scene(&world);

        assert_eq!(items.len(), 1, "the mesh entity must extract");
        assert_eq!(items[0].color, [1.0, 0.0, 0.0]);
        assert_eq!(items[0].vertices.len(), 6);
        assert_eq!(items[0].vertices[1], [0.5, -0.5, 0.0]);
    }

    #[test]
    fn extract_skips_stale_handles_without_trapping() {
        let mut world = World::new();
        let mut store: AssetStore<Mesh> = AssetStore::new();
        let handle = store.insert(quad_mesh());
        store.remove(handle);
        world.insert_resource(store);
        let stale = world.spawn();
        world
            .insert(stale, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(stale, MeshRenderable::new(handle, [1.0, 0.0, 0.0]))
            .expect("fresh entity accepts MeshRenderable");
        let live = world.spawn();
        world
            .insert(live, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        let live_handle = world
            .resource_mut::<AssetStore<Mesh>>()
            .expect("store resource must exist")
            .insert(quad_mesh());
        world
            .insert(live, MeshRenderable::new(live_handle, [0.0, 0.0, 1.0]))
            .expect("fresh entity accepts MeshRenderable");

        let items = extract_mesh_scene(&world);

        assert_eq!(
            items.len(),
            1,
            "the stale handle must skip; the live mesh must still extract"
        );
        assert_eq!(items[0].color, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn extract_without_a_store_resource_yields_empty_not_a_panic() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(
                entity,
                MeshRenderable::new(AssetHandle::from_raw_parts(0, 0), [1.0, 0.0, 0.0]),
            )
            .expect("fresh entity accepts MeshRenderable");

        let items = extract_mesh_scene(&world);

        assert!(
            items.is_empty(),
            "no store resource means no resolvable meshes, not a panic"
        );
    }

    #[test]
    fn recycled_slot_new_handle_extracts_new_geometry_old_handle_skips() {
        let quad = quad_mesh();
        let mut store = AssetStore::new();
        let stale = store.insert(quad);
        store.remove(stale);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/box.glb");
        let box_mesh = load_mesh(&path)
            .expect("box fixture must load")
            .into_iter()
            .nth(1)
            .expect("box fixture holds two meshes");
        assert_eq!(box_mesh.triangle_count(), 6);
        let live = store.insert(box_mesh);
        assert_eq!(
            stale.index(),
            live.index(),
            "the freed slot must be recycled, so aliasing pressure is real"
        );
        assert_ne!(stale, live, "the recycled handle must differ by generation");
        let mut world = World::new();
        world.insert_resource(store);
        let stale_entity = world.spawn();
        world
            .insert(stale_entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(stale_entity, MeshRenderable::new(stale, [1.0, 0.0, 0.0]))
            .expect("fresh entity accepts MeshRenderable");
        let live_entity = world.spawn();
        world
            .insert(live_entity, GlobalTransform::default())
            .expect("fresh entity accepts GlobalTransform");
        world
            .insert(live_entity, MeshRenderable::new(live, [0.0, 0.0, 1.0]))
            .expect("fresh entity accepts MeshRenderable");

        let items = extract_mesh_scene(&world);

        assert_eq!(
            items.len(),
            1,
            "the stale quad handle must skip while the recycled box handle extracts"
        );
        assert_eq!(
            items[0].vertices.len(),
            18,
            "the extracted geometry must be the box's six triangles, not the quad's two"
        );
        assert_eq!(items[0].color, [0.0, 0.0, 1.0]);
    }
}
