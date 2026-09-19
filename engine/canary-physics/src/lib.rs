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
//! # What is implemented (v0.0.11 Task 1: scaffold only)
//!
//! Nothing yet beyond the crate itself and its verified `rapier2d` pin:
//! components, the backend trait, the Rapier backend, fixed-stepping, and
//! scheduler registration arrive in Tasks 3–5.
//!
//! # What is explicitly stub (later tasks own it)
//!
//! - **Components + trait** (Task 3): bodies, colliders, velocity,
//!   gravity scale, locked axes, collider material, and the object-safe,
//!   leak-free `PhysicsBackend` trait.
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
