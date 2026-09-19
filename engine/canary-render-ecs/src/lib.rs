//! ECS-to-render bridge: extracts renderables from the [`World`](canary_ecs::World),
//! CPU-bakes them into NDC vertex buffers, and draws them through `canary-render`.
//!
//! # Target design
//!
//! The bridge is a third crate *above* `canary-ecs` and `canary-render`: it reads
//! `GlobalTransform` plus a flat-colored renderable component out of the `World`
//! via the scheduler, bakes object-space triangles to screen-space vertices on
//! the CPU each frame, and issues one buffer plus one draw per frame against the
//! existing `canary-render` device interface. `canary-render` itself stays
//! dependency-free; all ECS and math coupling lives here.
//!
//! # Current status
//!
//! [`Renderable`], extract + CPU-bake + draw, Schedule wiring with
//! propagation-before-bake ordering ([`systems::register_render_bake`]),
//! offscreen pixel tests, and the spinning-cube rewrite are all landed:
//! [`extract::extract_scene`] snapshots the [`World`](canary_ecs::World),
//! [`extract::bake_scene_to_vertices`] turns the snapshot into NDC floats,
//! [`pipeline::draw_baked_frame`] issues one buffer plus one draw per frame,
//! and `canary-runtime`'s tick drives propagation-then-bake through the
//! scheduler. File-loaded meshes ride the same path via
//! [`MeshRenderable`]; file-loaded textures ride the single-texture
//! sampled path ([`TexturedRenderable`], [`TEXTURED_WGSL`](crate::TEXTURED_WGSL),
//! [`pipeline::draw_textured_frame`]). Still deferred: any further RHI
//! upgrades — push constants, depth, `write_buffer`, multi-texture
//! materials, swapchain — all v0.0.10+.
//! Known target limits: convex-only painter sort (concave self-occlusion
//! needs a depth buffer) and a fixed camera (no camera component yet).

mod extract;
mod mesh_renderable;
mod pipeline;
mod renderable;
mod systems;
mod textured_renderable;

pub use extract::{
    bake_scene_to_vertices, bake_scene_to_vertices_with_aspect, extract_scene, BakedFrame,
    RenderItem,
};
pub use mesh_renderable::{expand_mesh_to_soup, extract_mesh_scene, MeshRenderable};
pub use pipeline::{
    draw_baked_frame, draw_textured_frame, render_vertex_attributes, render_vertex_stride,
    textured_vertex_attributes, textured_vertex_stride, DEFAULT_CLEAR_COLOR, RENDER_WGSL,
    TEXTURED_WGSL,
};
pub use renderable::Renderable;
pub use systems::{
    bake_access, bake_mesh_access, bake_mesh_scene_system, bake_scene_system, bake_textured_access,
    bake_textured_scene_system, register_mesh_render_bake, register_render_bake,
    register_textured_render_bake,
};
pub use textured_renderable::{
    bake_textured_scene_to_vertices, bake_textured_scene_to_vertices_with_aspect,
    expand_mesh_to_textured_soup, extract_textured_scene, BakedTexturedFrame, TexturedRenderItem,
    TexturedRenderable,
};
