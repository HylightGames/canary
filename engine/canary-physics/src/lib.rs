// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine 2D physics: components, the `PhysicsBackend` trait, and a
//! Rapier2D backend stepped at a fixed timestep.
//!
//! See `docs/architecture/physics.md` for the full design and
//! [ADR 0019](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0019-physics-backend-lineup.md)
//! for the backend lineup this crate records: **rapier2d-only 2D scope** —
//! Rapier2D is the canonical 2D backend, Jolt is the canonical 3D backend,
//! and Rapier3D is the 3D alternative. Jolt/Rapier3D land post-`v0.1.0`
//! (per the `v0.1.0` plan), so nothing 3D lives in this crate.
//!
//! # What is implemented (v0.0.11 Task 3: components + trait)
//!
//! - [`components`]: minimal bodies, colliders, velocity, gravity scale,
//!   locked axes, collider material, and the [`PhysicsConfig`] resource.
//! - [`backend`]: the object-safe, leak-free [`PhysicsBackend`] trait,
//!   opaque [`BodyHandle`] and [`ColliderHandle`] keys, the [`FIXED_DT`]
//!   timestep, and typed [`PhysicsError`] failures.
//!
//! # What is explicitly stub (later tasks own it)
//!
//! - **Backend + fixed-step** (Task 4): the private Rapier backend,
//!   `PhysicsClock` accumulator, `SimulationTime`, and 2D-only sync into
//!   `Transform`.
//! - **Wiring + game proof** (Task 5): `EcsSubsystem` registration and the
//!   z-pinned-quad scene.
//!
//! # A note on dependencies
//!
//! This crate depends on `canary-ecs`, `canary-scheduler`, and
//! `canary-transform` (the stepping/sync seams its later tasks are built
//! on), `glam` for 2D boundary math at the sync boundary, `thiserror` for
//! typed errors, and `rapier2d` — which is pure safe Rust, so no `unsafe`
//! blocks are expected anywhere in this crate.
//!
//! The `rapier2d` dependency carries no code yet: this task defines the
//! seam the backend will sit behind, and no item in [`components`] or
//! [`backend`] names a solver type. Joints, scene queries, and
//! trimesh/heightfield colliders are named deferred items (see the
//! v0.0.11 work plan), not gaps.

pub mod backend;
pub mod components;

pub use backend::{BodyHandle, ColliderHandle, PhysicsBackend, PhysicsError, FIXED_DT};
pub use components::{
    Collider, ColliderMaterial, GravityScale, LockedAxes, PhysicsBackendName, PhysicsConfig,
    PhysicsDimension, RigidBody, RigidBodyKind, Velocity,
};
