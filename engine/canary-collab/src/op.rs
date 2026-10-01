//! The `.16` operation: complete local-transform replacement.
//!
//! The first (and only) operation replaces the whole local `Transform` of
//! an existing authored entity. It carries one validated
//! [`TransformPayload`] against the `canary.transform` schema version 1 —
//! no generic patches, no partial fields, no other component. Rotation is a
//! unit quaternion `[x, y, z, w]`; scale is content (zero and negative
//! values are accepted — collapsing or mirroring is the author's meaning,
//! not corruption).

use crate::error::RejectCode;

/// Tolerance on quaternion normalization: `|len - 1| <= epsilon`.
///
/// Tight enough to reject garbage, loose enough for `f32` round trips
/// through JSON authoring tools.
pub const QUAT_NORM_EPSILON: f32 = 1e-3;

/// Complete local-transform replacement payload.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TransformPayload {
    /// Local-space translation.
    pub translation: [f32; 3],
    /// Local-space rotation as a unit quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Local-space scale. Zero and negative values are valid content.
    pub scale: [f32; 3],
}

impl TransformPayload {
    /// The identity transform payload.
    #[must_use]
    pub fn identity() -> Self {
        Self {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }

    /// Numeric validation (validation stage 6): every float finite, the
    /// quaternion normalized within [`QUAT_NORM_EPSILON`]. Scale is
    /// validated only for finiteness — zero and negative scales are
    /// accepted as content.
    ///
    /// Numeric semantics come from `canary-transform`, by contract rather
    /// than by dependency (this crate must not depend on it): the shared
    /// accept/reject vectors are pinned by the parity test in
    /// `canary-runtime`, which fails if either side's verdicts drift.
    pub fn validate(&self) -> Result<(), RejectCode> {
        for value in self
            .translation
            .iter()
            .chain(self.rotation.iter())
            .chain(self.scale.iter())
        {
            if !value.is_finite() {
                return Err(RejectCode::InvalidPayload);
            }
        }
        let length_squared = self.rotation[0] * self.rotation[0]
            + self.rotation[1] * self.rotation[1]
            + self.rotation[2] * self.rotation[2]
            + self.rotation[3] * self.rotation[3];
        let length = length_squared.sqrt();
        if (length - 1.0).abs() > QUAT_NORM_EPSILON {
            return Err(RejectCode::InvalidPayload);
        }
        Ok(())
    }

    /// Converts to the canonical authored field object stored in the
    /// entity section under `canary.transform`.
    #[must_use]
    pub fn to_fields(&self) -> serde_json::Value {
        serde_json::json!({
            "translation": self.translation,
            "rotation": self.rotation,
            "scale": self.scale,
        })
    }

    /// Parses the canonical authored field object back (conflict-refresh
    /// path). Out-of-range values fail through [`Self::validate`], never
    /// silently.
    pub fn from_fields(fields: &serde_json::Value) -> Result<Self, RejectCode> {
        let object = fields.as_object().ok_or(RejectCode::InvalidPayload)?;
        let array = |key: &str| -> Result<[f32; 3], RejectCode> {
            let items = object.get(key).ok_or(RejectCode::InvalidPayload)?;
            let list = items.as_array().ok_or(RejectCode::InvalidPayload)?;
            if list.len() != 3 {
                return Err(RejectCode::InvalidPayload);
            }
            let mut out = [0.0f32; 3];
            for (index, item) in list.iter().enumerate() {
                let value = item.as_f64().ok_or(RejectCode::InvalidPayload)?;
                // JSON has no NaN arm, so `as` here only narrows range;
                // overflow lands on infinity and `validate` rejects it.
                out[index] = value as f32;
            }
            Ok(out)
        };
        let quat = |key: &str| -> Result<[f32; 4], RejectCode> {
            let items = object
                .get(key)
                .ok_or(RejectCode::InvalidPayload)?
                .as_array()
                .ok_or(RejectCode::InvalidPayload)?;
            if items.len() != 4 {
                return Err(RejectCode::InvalidPayload);
            }
            let mut out = [0.0f32; 4];
            for (index, item) in items.iter().enumerate() {
                let value = item.as_f64().ok_or(RejectCode::InvalidPayload)?;
                out[index] = value as f32;
            }
            Ok(out)
        };
        let payload = Self {
            translation: array("translation")?,
            rotation: quat("rotation")?,
            scale: array("scale")?,
        };
        payload.validate()?;
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_payload_validates() {
        TransformPayload::identity().validate().expect("identity");
    }

    #[test]
    fn non_finite_floats_are_rejected() {
        for make in [
            TransformPayload {
                translation: [f32::NAN, 0.0, 0.0],
                ..TransformPayload::identity()
            },
            TransformPayload {
                translation: [f32::INFINITY, 0.0, 0.0],
                ..TransformPayload::identity()
            },
            TransformPayload {
                rotation: [0.0, 0.0, 0.0, f32::NEG_INFINITY],
                ..TransformPayload::identity()
            },
            TransformPayload {
                scale: [1.0, f32::NAN, 1.0],
                ..TransformPayload::identity()
            },
        ] {
            assert_eq!(make.validate(), Err(RejectCode::InvalidPayload));
        }
    }

    #[test]
    fn denormalized_quaternions_are_rejected() {
        for rotation in [
            [0.0, 0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 2.0],
            [f32::NAN, 0.0, 0.0, 1.0],
        ] {
            let payload = TransformPayload {
                rotation,
                ..TransformPayload::identity()
            };
            assert_eq!(
                payload.validate(),
                Err(RejectCode::InvalidPayload),
                "rotation {rotation:?} must be rejected"
            );
        }
    }

    #[test]
    fn near_unit_quaternions_survive_f32_round_trips() {
        // `f32` JSON round trips drift the norm by ~1e-7; the epsilon must
        // tolerate that while still rejecting real garbage (above).
        let half_root = std::f32::consts::FRAC_1_SQRT_2;
        let payload = TransformPayload {
            rotation: [half_root, 0.0, 0.0, half_root],
            ..TransformPayload::identity()
        };
        payload.validate().expect("near-unit quaternion");
    }

    #[test]
    fn zero_and_negative_scales_are_content_not_corruption() {
        for scale in [[0.0, 1.0, 1.0], [-1.0, 2.0, 0.5], [0.0, 0.0, 0.0]] {
            TransformPayload {
                scale,
                ..TransformPayload::identity()
            }
            .validate()
            .expect("scale is content");
        }
    }

    #[test]
    fn fields_round_trip_through_authored_json() {
        let payload = TransformPayload {
            translation: [1.0, -2.0, 3.5],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
        };
        let fields = payload.to_fields();
        assert_eq!(
            TransformPayload::from_fields(&fields).expect("parse"),
            payload
        );
    }

    #[test]
    fn malformed_field_objects_fail_typed() {
        for fields in [
            serde_json::json!({}),
            serde_json::json!({"translation": [0, 0], "rotation": [0,0,0,1], "scale": [1,1,1]}),
            serde_json::json!({"translation": ["x", 0, 0], "rotation": [0,0,0,1], "scale": [1,1,1]}),
            serde_json::json!([1, 2, 3]),
        ] {
            assert!(
                TransformPayload::from_fields(&fields).is_err(),
                "fields {fields} must fail typed"
            );
        }
    }
}
