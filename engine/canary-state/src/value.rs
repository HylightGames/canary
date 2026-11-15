//! Canonical field values for snapshot records.
//!
//! [`SnapshotValue`] is the only value language snapshots speak: no
//! `usize`, no NaN, maps in key order. postcard would happily encode a NaN;
//! [`SnapshotValue::validate`] refuses it, because a NaN has no canonical
//! form and would break checksum stability.
//!
//! The type is recursive, and `postcard` imposes no recursion limit, so a
//! hand-crafted payload (two bytes per level) could drive the decoder into
//! an unbounded stack. Decoding therefore runs under a thread-local depth
//! guard bounded by [`MAX_SNAPSHOT_VALUE_DEPTH`]; a value past the bound
//! fails with [`StateError::NestingTooDeep`](crate::StateError), never a
//! stack overflow. Encoding is bounded by the same limit through
//! [`SnapshotValue::validate`]. Values built programmatically deeper than
//! the bound are a caller bug: they are refused at encode, but their
//! recursive `Drop` is not made iterative here (the guard is about
//! untrusted input, not in-process construction).

use std::cell::Cell;
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::StateError;

/// Maximum nesting depth of a [`SnapshotValue`], counting the value
/// itself: a flat scalar is depth 1, and a list holding a scalar is
/// depth 2. 64 levels is far past any legitimate component field
/// (canonical data is a flat field map) and far below the depth that
/// exhausts a thread stack.
pub const MAX_SNAPSHOT_VALUE_DEPTH: usize = 64;

/// Canonical field value. Every variant postcard-encodes deterministically.
#[derive(Debug, Clone, PartialEq, Serialize)]
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

/// Decode mirror of [`SnapshotValue`], variant order and names included.
///
/// postcard encodes an enum by variant index, so this must stay
/// declaration-for-declaration identical to [`SnapshotValue`]. It exists
/// only so the public type can carry a manual [`Deserialize`] that threads
/// the depth guard through every nested value; the nested payload fields
/// are the real type, so each level re-enters the guard.
#[derive(Deserialize)]
enum SnapshotValueRepr {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<SnapshotValue>),
    Map(BTreeMap<String, SnapshotValue>),
}

impl From<SnapshotValueRepr> for SnapshotValue {
    fn from(value: SnapshotValueRepr) -> Self {
        match value {
            SnapshotValueRepr::Null => Self::Null,
            SnapshotValueRepr::Bool(flag) => Self::Bool(flag),
            SnapshotValueRepr::I64(number) => Self::I64(number),
            SnapshotValueRepr::U64(number) => Self::U64(number),
            SnapshotValueRepr::F64(number) => Self::F64(number),
            SnapshotValueRepr::Str(text) => Self::Str(text),
            SnapshotValueRepr::Bytes(bytes) => Self::Bytes(bytes),
            SnapshotValueRepr::List(items) => Self::List(items),
            SnapshotValueRepr::Map(entries) => Self::Map(entries),
        }
    }
}

thread_local! {
    /// Depth of the value currently being deserialized on this thread.
    static DECODE_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// Depth recorded by the most recent refused decode, if any. Read by
    /// [`crate::decode_snapshot`] to turn a codec failure into the typed
    /// [`StateError::NestingTooDeep`] arm without inspecting `postcard`'s
    /// error surface.
    static NESTING_FAILURE: Cell<Option<usize>> = const { Cell::new(None) };
}

/// RAII depth counter: entering a value increments it, leaving decrements.
struct DepthGuard;

impl DepthGuard {
    fn enter() -> Result<Self, String> {
        DECODE_DEPTH.with(|depth| {
            let next = depth.get().saturating_add(1);
            if next > MAX_SNAPSHOT_VALUE_DEPTH {
                NESTING_FAILURE.with(|failure| failure.set(Some(next)));
                Err(format!(
                    "snapshot value nesting depth exceeds {MAX_SNAPSHOT_VALUE_DEPTH} levels"
                ))
            } else {
                depth.set(next);
                Ok(Self)
            }
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DECODE_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

impl<'de> Deserialize<'de> for SnapshotValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let guard = DepthGuard::enter().map_err(serde::de::Error::custom)?;
        let repr = SnapshotValueRepr::deserialize(deserializer)?;
        drop(guard);
        Ok(Self::from(repr))
    }
}

/// Clears the recorded nesting failure before a decode, so a stale refusal
/// from an earlier call cannot be mistaken for this one's.
pub(crate) fn reset_nesting_failure() {
    NESTING_FAILURE.with(|failure| failure.set(None));
}

/// Takes the depth recorded by the most recent refused decode, if any.
/// Taking clears it, so one refusal maps to exactly one typed error.
pub(crate) fn take_nesting_failure() -> Option<usize> {
    NESTING_FAILURE.with(Cell::take)
}

impl SnapshotValue {
    /// Checks the value and everything nested in it is canonical: all
    /// floats finite, and nesting no deeper than
    /// [`MAX_SNAPSHOT_VALUE_DEPTH`]. postcard would happily encode NaN;
    /// snapshots must not.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::MigrationInvalid`] for a non-finite float and
    /// [`StateError::NestingTooDeep`] past the depth bound.
    pub fn validate(&self) -> Result<(), StateError> {
        self.validate_at(1)
    }

    /// Depth-counting body of [`SnapshotValue::validate`]. The depth gate
    /// runs before the recursive arm, so a value built too deeply is
    /// refused without recursing past the limit.
    fn validate_at(&self, depth: usize) -> Result<(), StateError> {
        if depth > MAX_SNAPSHOT_VALUE_DEPTH {
            return Err(StateError::NestingTooDeep {
                depth,
                max: MAX_SNAPSHOT_VALUE_DEPTH,
            });
        }
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
            Self::List(items) => items
                .iter()
                .try_for_each(|item| item.validate_at(depth + 1)),
            Self::Map(entries) => entries
                .values()
                .try_for_each(|entry| entry.validate_at(depth + 1)),
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

    #[test]
    fn decode_refuses_nesting_past_the_bound_without_overflowing() {
        // 10,000 levels: two bytes (`List` + length 1) each, then `Null`.
        let mut bytes = Vec::new();
        for _ in 0..10_000 {
            bytes.extend_from_slice(&[7, 1]);
        }
        bytes.push(0);
        reset_nesting_failure();
        let result = postcard::from_bytes::<SnapshotValue>(&bytes);
        assert!(result.is_err(), "10,000 levels must be refused");
        assert_eq!(
            take_nesting_failure(),
            Some(MAX_SNAPSHOT_VALUE_DEPTH + 1),
            "the refusal must record the first over-limit depth"
        );
    }

    #[test]
    fn decode_accepts_nesting_at_the_bound() {
        let mut value = SnapshotValue::Null;
        for _ in 0..(MAX_SNAPSHOT_VALUE_DEPTH - 1) {
            value = SnapshotValue::List(vec![value]);
        }
        assert!(value.validate().is_ok());
        let bytes = postcard::to_allocvec(&value).expect("encode");
        reset_nesting_failure();
        let decoded: SnapshotValue = postcard::from_bytes(&bytes).expect("decode at the bound");
        assert_eq!(decoded, value);
        assert_eq!(take_nesting_failure(), None);
    }

    #[test]
    fn validate_refuses_a_value_built_past_the_bound() {
        let mut value = SnapshotValue::Null;
        for _ in 0..MAX_SNAPSHOT_VALUE_DEPTH {
            value = SnapshotValue::List(vec![value]);
        }
        assert!(matches!(
            value.validate(),
            Err(StateError::NestingTooDeep { .. })
        ));
    }
}
