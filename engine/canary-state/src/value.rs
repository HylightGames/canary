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
}
