//! Flat-colored triangle-soup renderable component for the ECS-to-render bridge.
//!
//! See [`Renderable`] for the target-design rationale.

/// A flat-colored, object-space triangle soup attached to one entity.
///
/// `vertices` holds object-space positions as consecutive triangles: every
/// three entries form one triangle, so `vertices.len() % 3 == 0` must hold
/// (see [`Renderable::is_valid`]). `color` is a single flat RGB triple shared
/// by every vertex of this entity.
///
/// # Why this shape
///
/// - **Why flat per-entity color:** the RHI has no uniforms, no descriptor
///   sets, no materials, and no textures — [`PipelineDescriptor`] documents
///   "no descriptor sets/uniforms", and [`VertexFormat`] offers only
///   [`Float32x2`](canary_render::VertexFormat::Float32x2) and
///   [`Float32x3`](canary_render::VertexFormat::Float32x3). The only way to
///   give two entities distinguishable pixels today is to bake a constant
///   RGB triple into each vertex, which is exactly what `[f32; 3]` maps to:
///   one [`Float32x3`](canary_render::VertexFormat::Float32x3) color
///   attribute (12 bytes) per vertex. Per-entity colors additionally enable
///   the multi-entity pixel tests (Task 5) to tell entities apart.
/// - **Why triangle soup with no index buffer:** the RHI has no index-buffer
///   support — [`CommandEncoder::set_vertex_buffer`] binds exactly one vertex
///   buffer and its docs state "no index buffer yet", while
///   [`CommandEncoder::draw`] consumes a plain `vertex_count` with no
///   instancing. An indexed mesh would have nothing to bind to, so triangles
///   are stored pre-expanded.
/// - **Why no UVs, normals, or materials:** those need texture sampling,
///   lighting inputs, and descriptor-backed material data — all listed in
///   [`RenderDevice`](canary_render::RenderDevice)'s docs as expected future
///   RHI work, not present capability. Mesh-asset resources and materials are
///   deferred to v0.0.10; adding them here would be scope creep against
///   decision D2 of the v0.0.9 stage plan.
///
/// [`PipelineDescriptor`]: canary_render::PipelineDescriptor
/// [`VertexFormat`]: canary_render::VertexFormat
/// [`CommandEncoder::set_vertex_buffer`]:
///     canary_render::CommandEncoder::set_vertex_buffer
/// [`CommandEncoder::draw`]: canary_render::CommandEncoder::draw
#[derive(Debug, Clone, PartialEq)]
pub struct Renderable {
    /// Object-space vertex positions as consecutive triangles.
    ///
    /// Every three entries form one triangle, so the length must be a
    /// multiple of three (checked by [`Renderable::is_valid`]). Stored
    /// pre-expanded — no index buffer — because the RHI
    /// ([`CommandEncoder::set_vertex_buffer`]) binds a single vertex buffer
    /// with no index support.
    ///
    /// [`CommandEncoder::set_vertex_buffer`]:
    ///     canary_render::CommandEncoder::set_vertex_buffer
    pub vertices: Vec<[f32; 3]>,
    /// Flat RGB color shared by every vertex of this entity, each channel in
    /// `[0.0, 1.0]`.
    ///
    /// One color per entity — not per vertex — because the RHI has no
    /// uniforms or materials to vary color any other way: this triple is
    /// replicated into each baked vertex's
    /// [`Float32x3`](canary_render::VertexFormat::Float32x3) color
    /// attribute at bake time.
    pub color: [f32; 3],
}

impl Renderable {
    /// Stores object-space triangle vertices with a flat per-entity color.
    ///
    /// Takes the vertex list as-is: no validation, no reordering, no
    /// allocation beyond the move — checking the triangle invariant is
    /// [`Renderable::is_valid`]'s job so that construction stays infallible
    /// and invalid data surfaces at bake time, not at spawn time.
    pub fn new(vertices: Vec<[f32; 3]>, color: [f32; 3]) -> Self {
        Self { vertices, color }
    }

    /// The number of complete triangles in [`Renderable::vertices`].
    ///
    /// Integer division by three: any trailing partial triangle (see
    /// [`Renderable::is_valid`]) contributes nothing here, which is exactly
    /// why bake must reject invalid renderables instead of silently dropping
    /// their leftover vertices.
    pub fn triangle_count(&self) -> usize {
        self.vertices.len() / 3
    }

    /// Whether the vertex list partitions into complete triangles.
    ///
    /// Returns `vertices.len() % 3 == 0`. An invalid renderable (a trailing
    /// partial triangle) has no defined rasterization: the downstream bake
    /// (Task 3 of the v0.0.9 stage plan) must skip or reject it — emitting a
    /// partial triangle's vertices would feed the single-vertex-buffer draw
    /// ([`CommandEncoder::draw`](canary_render::CommandEncoder::draw))
    /// vertices that form no triangle, corrupting the frame. This predicate
    /// is the contract that lets bake enforce that.
    pub fn is_valid(&self) -> bool {
        self.vertices.len() % 3 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts two `f32` values agree within `1e-6`; exact `==` on floats
    /// is brittle, so every float assertion in this module goes through here.
    fn assert_approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn new_stores_vertices_and_color() {
        let vertices = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let color = [1.0, 0.25, 0.5];

        let renderable = Renderable::new(vertices.clone(), color);

        assert_eq!(renderable.vertices, vertices);
        for (actual, expected) in renderable.color.iter().zip(color.iter()) {
            assert_approx_eq(*actual, *expected);
        }
    }

    #[test]
    fn triangle_count_divides_by_three() {
        let vertices = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
        ];

        let renderable = Renderable::new(vertices, [1.0, 1.0, 1.0]);

        assert_eq!(renderable.triangle_count(), 2);
    }

    #[test]
    fn is_valid_rejects_non_multiple_of_three() {
        let valid = Renderable::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [1.0, 0.0, 0.0],
        );
        let invalid = Renderable::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], [1.0, 0.0, 0.0]);

        assert!(valid.is_valid());
        assert!(!invalid.is_valid());
    }

    #[test]
    fn empty_vertices_is_valid_with_zero_triangles() {
        let renderable = Renderable::new(vec![], [0.0, 0.0, 0.0]);

        assert!(renderable.is_valid());
        assert_eq!(renderable.triangle_count(), 0);
    }
}
