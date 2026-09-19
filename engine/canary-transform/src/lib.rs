// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine spatial transforms.
//!
//! [`Transform`] is the single, always-3D local representation
//! (`glam::Vec3` translation, `glam::Quat` rotation, `glam::Vec3` scale)
//! shared by 2D and 3D games alike; [`GlobalTransform`] caches the composed
//! world-space matrix so readers never re-walk the parent chain. Hierarchy
//! is a plain ECS relationship ([`Parent`]/[`Children`], kept in sync via
//! [`set_parent`]/[`remove_parent`]), and [`propagate_transforms`] recomputes
//! every [`GlobalTransform`] parent-before-child through `canary-scheduler`.
//! See `docs/architecture/transform.md` and ADR 0017 for the design.
//!
//! **Not yet here**: change-detection-gated propagation (only re-propagating
//! subtrees whose `Transform` or ancestry changed) — a `v0.2.0`+ performance
//! concern. The current implementation depth-orders the hierarchy walk each
//! run, which is correct but does redundant work; see `transform.md` for why
//! that tradeoff is deliberate for now.

mod hierarchy;
mod propagation;
mod transform;

pub use hierarchy::{remove_parent, set_parent, Children, Parent};
pub use propagation::{
    propagate_transforms, register_transform_propagation, transform_propagation_access,
};
pub use transform::{GlobalTransform, Transform};
