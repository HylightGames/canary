// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine 2D physics: components, the `PhysicsBackend` trait, a
//! Rapier2D backend, and fixed-timestep stepping.
//!
//! See `docs/architecture/physics.md` for the full design and
//! [ADR 0019](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0019-physics-backend-lineup.md)
//! for the backend lineup this crate records: **rapier2d-only 2D scope** —
//! Rapier2D is the canonical 2D backend, Jolt is the canonical 3D backend,
//! and Rapier3D is the 3D alternative. Jolt/Rapier3D land post-`v0.1.0`
//! (per the `v0.1.0` plan), so nothing 3D lives in this crate.
//!
//! # What is implemented (v0.0.11 Tasks 3+4: data + trait + backend + stepping)
//!
//! - [`components`]: minimal bodies, colliders, velocity, gravity scale,
//!   locked axes, collider material, and the [`PhysicsConfig`] resource.
//! - [`backend`]: the object-safe, leak-free [`PhysicsBackend`] trait,
//!   opaque [`BodyHandle`] and [`ColliderHandle`] keys, the [`FIXED_DT`]
//!   timestep, and typed [`PhysicsError`] failures.
//! - [`RapierBackend`]: the private Rapier2D implementor (module
//!   `rapier_backend` stays private; only the type is re-exported so
//!   scheduler access declarations and resource lookups can name it).
//! - [`systems`]: [`PhysicsClock`] accumulator + [`SimulationTime`] +
//!   [`FrameDelta`], [`physics_step_system`] (fixed-steppaging with the
//!   spiral guard, 2D-only sync preserving z/off-axis/scale), and
//!   [`register_physics_step`] — which MUST run FIRST in the subsystem
//!   schedule (physics → propagation → soup → mesh → textured; see
//!   [`systems`]' module docs for the full ordering law).
//!
//! # What is explicitly stub (later tasks own it)
//!
//! - **Wiring + game proof** (Task 5): `EcsSubsystem` registration and the
//!   z-pinned-quad scene.
//!
//! # A note on dependencies
//!
//! This crate depends on `canary-ecs`, `canary-scheduler`, and
//! `canary-transform` (the stepping/sync seams the fixed-step system is
//! built on), `glam` for 2D boundary math at the sync boundary,
//! `thiserror` for typed errors, and `rapier2d` — which is pure safe
//! Rust, so no `unsafe` blocks are expected anywhere in this crate.
//! rapier2d runs on default features (`dim2`, `f32`, `std`,
//! `block-solver`): no `parallel` (it would trade the bit-identical
//! single-machine determinism Task 4 proves for throughput the v0.0.11
//! body counts do not need) and no `serde-serialize` (no persistence
//! consumer yet). Joints, scene queries, and
//! trimesh/heightfield colliders are named deferred items (see the
//! v0.0.11 work plan), not gaps.

pub mod backend;
pub mod components;
pub mod systems;

mod rapier_backend;

pub use backend::{BodyHandle, ColliderHandle, PhysicsBackend, PhysicsError, FIXED_DT};
pub use components::{
    Collider, ColliderMaterial, GravityScale, LockedAxes, PhysicsBackendName, PhysicsConfig,
    PhysicsDimension, RigidBody, RigidBodyKind, Velocity,
};
pub use rapier_backend::RapierBackend;
pub use systems::{
    physics_step_access, physics_step_system, register_physics_step, FrameDelta, PhysicsClock,
    SimulationTime, MAX_STEPS_PER_TICK,
};
