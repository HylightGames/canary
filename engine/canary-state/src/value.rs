//! Canonical field values for snapshot records.
//!
//! [`SnapshotValue`] is the only value language snapshots speak: no
//! `usize`, no NaN, maps in key order. postcard would happily encode a NaN;
//! [`SnapshotValue::validate`] refuses it, because a NaN has no canonical
//! form and would break checksum stability.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::StateError;

/// Canonical field value. Every variant postcard-encodes deterministically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SnapshotValue {
    /// Absent value.
    Null,
    /// Boolean.
    Bool(bool),
    /// Signed integer.
    I64(i64),
    /// Unsigned integer.
    U64(u64),
    /// Finite float. Non-finite values are rejected at encode time.
    F64(f64),
    /// Text.
    Str(String),
    /// Raw bytes.
    Bytes(Vec<u8>),
    /// Ordered list.
    List(Vec<SnapshotValue>),
    /// Ordered map.
    Map(BTreeMap<String, SnapshotValue>),
}

impl SnapshotValue {
    /// Checks the value and everything nested in it is canonical: all
    /// floats finite. postcard would happily encode NaN; snapshots must not.
    pub fn validate(&self) -> Result<(), StateError> {
        match self {
            Self::Null
            | Self::Bool(_)
            | Self::I64(_)
            | Self::U64(_)
            | Self::Str(_)
            | Self::Bytes(_) => Ok(()),
            Self::F64(f) if f.is_finite() => Ok(()),
            Self::F64(_) => Err(StateError::MigrationInvalid {
                schema: "<snapshot-value>".to_owned(),
                to: 0,
                reason: "non-finite float has no canonical encoding".to_owned(),
            }),
            Self::List(items) => items.iter().try_for_each(Self::validate),
            Self::Map(entries) => entries.values().try_for_each(Self::validate),
        }
    }

    /// Converts authored JSON into a snapshot value. JSON has no NaN,
    /// no bytes, and no integer-width contract, so numbers prefer the
    /// exact integer arms and fall back to float; every output passes
    /// [`SnapshotValue::validate`].
    #[must_use]
    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(flag) => Self::Bool(*flag),
            serde_json::Value::Number(number) => number
                .as_i64()
                .map(Self::I64)
                .or_else(|| number.as_u64().map(Self::U64))
                .or_else(|| number.as_f64().map(Self::F64))
                .unwrap_or(Self::Null),
            serde_json::Value::String(text) => Self::Str(text.clone()),
            serde_json::Value::Array(items) => {
                Self::List(items.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(entries) => Self::Map(
                entries
                    .iter()
                    .map(|(key, item)| (key.clone(), Self::from_json(item)))
                    .collect(),
            ),
        }
    }

    /// Converts a snapshot value back into authored JSON: the inverse of
    /// [`SnapshotValue::from_json`] for values that JSON can represent.
    /// `Bytes` (which JSON has no native arm for) become arrays of `U64`
    /// numbers; integers keep their exact arm, finite floats stay floats.
    /// The bridge layer uses this to run JSON-shaped [`MigrationChain`](crate::migration::MigrationChain)
    /// steps over record fields without a new framework.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(flag) => serde_json::Value::Bool(*flag),
            Self::I64(value) => serde_json::json!(*value),
            Self::U64(value) => serde_json::json!(*value),
            Self::F64(value) => serde_json::json!(*value),
            Self::Str(text) => serde_json::Value::String(text.clone()),
            Self::Bytes(bytes) => serde_json::Value::Array(
                bytes
                    .iter()
                    .map(|byte| serde_json::Value::from(u64::from(*byte)))
                    .collect(),
            ),
            Self::List(items) => {
                serde_json::Value::Array(items.iter().map(Self::to_json).collect())
            }
            Self::Map(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(key, item)| (key.clone(), item.to_json()))
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_preserves_integer_arms_and_nesting() {
        let fields = BTreeMap::from([
            ("hp".to_owned(), SnapshotValue::I64(-3)),
            ("seed".to_owned(), SnapshotValue::U64(u64::MAX)),
            (
                "nested".to_owned(),
                SnapshotValue::Map(BTreeMap::from([(
                    "flag".to_owned(),
                    SnapshotValue::Bool(true),
                )])),
            ),
        ]);
        for value in fields.values() {
            assert_eq!(&SnapshotValue::from_json(&value.to_json()), value);
        }
    }

    #[test]
    fn bytes_become_number_arrays() {
        let value = SnapshotValue::Bytes(vec![1, 255]);
        assert_eq!(value.to_json(), serde_json::json!([1, 255]));
    }
}
