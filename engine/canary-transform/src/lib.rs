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
//! **Quiet-tick skip**: [`propagate_transforms`] returns early when no
//! `Transform`, `Parent`, `Children`, or `GlobalTransform` was written
//! since its last recompute and membership counts are unchanged (change
//! detection plus a structural fingerprint, with a same-tick guard and a
//! one-tick follow-up pass keeping the probe exact) — static ticks cost a
//! few linear tick scans instead of the full snapshot/depth/compose pass,
//! and any real change still recomputes exactly as before. No depth
//! state is cached across runs, so hierarchy edits need no invalidation.

mod hierarchy;
mod propagation;
mod transform;

pub use hierarchy::{despawn_subtree, remove_parent, set_parent, Children, Parent};
pub use propagation::{
    propagate_transforms, register_transform_propagation, transform_propagation_access,
};
pub use transform::{GlobalTransform, Transform};
