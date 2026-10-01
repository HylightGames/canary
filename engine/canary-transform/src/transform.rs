// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Local and world-space transform components.

use thiserror::Error;

/// Tolerance on quaternion normalization: `|len - 1| <= epsilon`.
///
/// Tight enough to reject garbage, loose enough for `f32` round trips
/// through JSON authoring tools. This mirrors `canary-collab`'s
/// `QUAT_NORM_EPSILON` by contract, not by dependency — neither crate
/// may depend on the other, so the value and the accept/reject vectors
/// are pinned by the parity test in `canary-runtime` instead of shared
/// code. Change one side and the parity test fails until both agree.
pub const QUAT_NORM_EPSILON: f32 = 1e-3;

/// Why a transform failed numeric validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TransformError {
    /// A translation, rotation, or scale component is NaN or infinite.
    #[error("transform has a non-finite component")]
    NonFinite,
    /// The rotation quaternion is not unit length within
    /// [`QUAT_NORM_EPSILON`].
    #[error("transform rotation is not a unit quaternion")]
    DenormalizedRotation,
}

/// Local space position, rotation, and scale of an entity.
///
/// The single, always-3D representation shared by 2D and 3D games alike (see
/// ADR 0017): a 2D game conventionally lives in one plane (typically
/// `translation.z` fixed, rotation constrained to the `z` axis) rather than
/// through a distinct `Transform2D` type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Local-space translation.
    pub translation: glam::Vec3,
    /// Local-space rotation.
    pub rotation: glam::Quat,
    /// Local-space scale.
    pub scale: glam::Vec3,
}

impl Transform {
    /// The identity transform: no translation, no rotation, unit scale.
    pub fn identity() -> Self {
        Self {
            translation: glam::Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        }
    }

    /// A transform holding only `translation` (identity rotation, unit scale).
    pub fn from_translation(translation: glam::Vec3) -> Self {
        Self {
            translation,
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        }
    }

    /// Numeric validation: every translation, rotation, and scale
    /// component finite, and the rotation a unit quaternion within
    /// [`QUAT_NORM_EPSILON`]. Scale is content — zero and negative
    /// values are accepted — matching the `canary-collab` stage-6 rule
    /// exactly (same verdicts on the same values; see the
    /// `canary-runtime` parity test). Finiteness is checked first so a
    /// NaN rotation reports [`TransformError::NonFinite`], never a
    /// normalization verdict computed from NaN.
    pub fn validate(&self) -> Result<(), TransformError> {
        for value in self
            .translation
            .to_array()
            .iter()
            .chain(self.rotation.to_array().iter())
            .chain(self.scale.to_array().iter())
        {
            if !value.is_finite() {
                return Err(TransformError::NonFinite);
            }
        }
        if (self.rotation.length() - 1.0).abs() > QUAT_NORM_EPSILON {
            return Err(TransformError::DenormalizedRotation);
        }
        Ok(())
    }

    /// Composes the local matrix as `T * R * S` (translation, then rotation,
    /// then scale).
    pub fn to_matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_translation(self.translation)
            * glam::Mat4::from_quat(self.rotation)
            * glam::Mat4::from_scale(self.scale)
    }
}

impl Default for Transform {
    /// Same as [`Transform::identity`].
    fn default() -> Self {
        Self::identity()
    }
}

/// Cached world-space matrix of an entity.
///
/// Recomputed by [`crate::propagate_transforms`]: roots copy their local
/// [`Transform`], children compose onto their parent's already-computed
/// [`GlobalTransform`]. Readers use this directly so nobody re-walks the
/// parent chain per query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalTransform(pub glam::Mat4);

impl GlobalTransform {
    /// Returns the cached world-space matrix.
    pub fn matrix(&self) -> glam::Mat4 {
        self.0
    }

    /// Wraps an already-composed world-space matrix.
    pub fn from_matrix(matrix: glam::Mat4) -> Self {
        Self(matrix)
    }
}

impl Default for GlobalTransform {
    /// The identity matrix (world origin, no rotation, unit scale).
    fn default() -> Self {
        Self(glam::Mat4::IDENTITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compares two matrices element-wise with an epsilon, since `glam`
    /// math is `f32` and exact equality is too strict after composition.
    fn assert_mat4_approx_eq(a: glam::Mat4, b: glam::Mat4) {
        for (x, y) in a.to_cols_array().iter().zip(b.to_cols_array().iter()) {
            assert!(
                (x - y).abs() < 1e-5,
                "matrices differ: {a:?} vs {b:?} (elements {x} vs {y})"
            );
        }
    }

    #[test]
    fn identity_has_zero_translation_identity_rotation_and_unit_scale() {
        let transform = Transform::identity();

        assert_eq!(transform.translation, glam::Vec3::ZERO);
        assert_eq!(transform.rotation, glam::Quat::IDENTITY);
        assert_eq!(transform.scale, glam::Vec3::ONE);
    }

    #[test]
    fn default_is_the_identity_transform() {
        let transform = Transform::default();

        assert_eq!(transform, Transform::identity());
    }

    #[test]
    fn from_translation_keeps_identity_rotation_and_unit_scale() {
        let translation = glam::Vec3::new(1.0, 2.0, 3.0);

        let transform = Transform::from_translation(translation);

        assert_eq!(transform.translation, translation);
        assert_eq!(transform.rotation, glam::Quat::IDENTITY);
        assert_eq!(transform.scale, glam::Vec3::ONE);
    }

    #[test]
    fn to_matrix_composes_translation_times_rotation_times_scale() {
        let transform = Transform {
            translation: glam::Vec3::new(1.0, 2.0, 3.0),
            rotation: glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            scale: glam::Vec3::new(2.0, 2.0, 2.0),
        };

        let expected = glam::Mat4::from_translation(transform.translation)
            * glam::Mat4::from_quat(transform.rotation)
            * glam::Mat4::from_scale(transform.scale);

        assert_mat4_approx_eq(transform.to_matrix(), expected);
    }

    #[test]
    fn identity_to_matrix_is_the_identity_matrix() {
        assert_mat4_approx_eq(Transform::identity().to_matrix(), glam::Mat4::IDENTITY);
    }

    #[test]
    fn translation_only_to_matrix_moves_points_by_that_translation() {
        let transform = Transform::from_translation(glam::Vec3::new(4.0, -2.0, 7.0));

        let moved = transform.to_matrix().transform_point3(glam::Vec3::ZERO);

        assert_mat4_approx_eq(
            transform.to_matrix(),
            glam::Mat4::from_translation(glam::Vec3::new(4.0, -2.0, 7.0)),
        );
        assert!((moved - glam::Vec3::new(4.0, -2.0, 7.0)).length() < 1e-5);
    }

    #[test]
    fn global_transform_matrix_and_from_matrix_round_trip() {
        let matrix = glam::Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0));

        let global = GlobalTransform::from_matrix(matrix);

        assert_mat4_approx_eq(global.matrix(), matrix);
    }

    #[test]
    fn global_transform_default_is_the_identity_matrix() {
        assert_mat4_approx_eq(GlobalTransform::default().matrix(), glam::Mat4::IDENTITY);
    }

    #[test]
    fn validate_accepts_identity_and_content_scales() {
        Transform::identity().validate().expect("identity");
        for scale in [
            glam::Vec3::new(0.0, 1.0, 1.0),
            glam::Vec3::new(-1.0, 2.0, 0.5),
            glam::Vec3::ZERO,
        ] {
            Transform {
                scale,
                ..Transform::identity()
            }
            .validate()
            .expect("scale is content, not corruption");
        }
    }

    #[test]
    fn validate_rejects_non_finite_components_before_any_quaternion_math() {
        for transform in [
            Transform {
                translation: glam::Vec3::new(f32::NAN, 0.0, 0.0),
                ..Transform::identity()
            },
            Transform {
                scale: glam::Vec3::new(1.0, f32::INFINITY, 1.0),
                ..Transform::identity()
            },
            // NaN rotation must report NonFinite, never a normalization
            // verdict computed from NaN.
            Transform {
                rotation: glam::Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0),
                ..Transform::identity()
            },
        ] {
            assert_eq!(transform.validate(), Err(TransformError::NonFinite));
        }
    }

    #[test]
    fn validate_rejects_denormalized_rotations() {
        for rotation in [
            glam::Quat::from_xyzw(0.0, 0.0, 0.0, 0.0),
            glam::Quat::from_xyzw(1.0, 1.0, 0.0, 0.0),
            glam::Quat::from_xyzw(0.0, 0.0, 0.0, 2.0),
        ] {
            assert_eq!(
                Transform {
                    rotation,
                    ..Transform::identity()
                }
                .validate(),
                Err(TransformError::DenormalizedRotation),
                "rotation {rotation:?} must be rejected"
            );
        }
        let half_root = std::f32::consts::FRAC_1_SQRT_2;
        Transform {
            rotation: glam::Quat::from_xyzw(half_root, 0.0, 0.0, half_root),
            ..Transform::identity()
        }
        .validate()
        .expect("near-unit quaternion survives f32 round trips");
    }
}
