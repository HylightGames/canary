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
//! # Current status: stub
//!
//! This crate is a scaffold only — no `Renderable` component, extract, bake, or
//! draw logic exists yet (Tasks 2-3 of the v0.0.9 stage plan). Known target
//! limits carried forward from the plan: convex-only painter sort (concave
//! self-occlusion needs a depth buffer), a fixed camera (no camera component),
//! and any RHI upgrades (push constants, depth, `write_buffer`, textures,
//! materials, swapchain) deferred to v0.0.10+.
