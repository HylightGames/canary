//! Schema identity and versioned envelopes.
//!
//! Every serialized payload — authored or snapshot — opens with an envelope
//! naming the [`SchemaId`], the [`SchemaVersion`], and the
//! [`EncodingVersion`]. Readers dispatch on the envelope before touching the
//! body: unknown schemas are [`StateError::UnknownSchema`](crate::StateError),
//! newer encodings are
//! [`StateError::UnsupportedEncoding`](crate::StateError), and older schema
//! versions route into [`migration`](crate::migration) chains.

use serde::{Deserialize, Serialize};

/// Opaque name for one versioned data shape, e.g. `"canary.project.v1"`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SchemaId(String);

impl SchemaId {
    /// Wraps a schema name. Names are dotted reverse-domain paths by
    /// convention; the type does not enforce it.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self(name.to_owned())
    }

    /// Wraps an owned schema name, for schemas declared at runtime.
    #[must_use]
    pub fn from_string(name: String) -> Self {
        Self(name)
    }

    /// The schema name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Monotonic version of one schema's body shape. Migrations walk `from` to
/// `to` one step at a time; skipping versions is never allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SchemaVersion(pub u32);

/// Version of the wire encoding itself, independent of any schema. Bumping
/// this means older builds can no longer read the bytes at all — unlike a
/// schema bump, there is no migration path, only rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EncodingVersion(pub u32);

/// Newest authored-JSON encoding this build reads and writes.
pub const AUTHORED_ENCODING: EncodingVersion = EncodingVersion(1);
/// Newest snapshot-postcard encoding this build reads and writes.
pub const SNAPSHOT_ENCODING: EncodingVersion = EncodingVersion(1);

/// Envelope opening every authored JSON document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoredEnvelope {
    /// Which shape the body holds.
    pub schema: SchemaId,
    /// Which shape version the body holds.
    pub version: SchemaVersion,
    /// Which JSON encoding rules produced the bytes.
    pub encoding: EncodingVersion,
}

/// Envelope opening every snapshot payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEnvelope {
    /// Which shape the body holds.
    pub schema: SchemaId,
    /// Which shape version the body holds.
    pub version: SchemaVersion,
    /// Which postcard encoding rules produced the bytes.
    pub encoding: EncodingVersion,
    /// Hex SHA-256 over the canonical encoded body.
    pub checksum: String,
}

/// Rejects envelopes no build of this crate could ever read.
pub fn check_encoding(
    found: EncodingVersion,
    supported: EncodingVersion,
) -> Result<(), crate::StateError> {
    if found.0 > supported.0 {
        return Err(crate::StateError::UnsupportedEncoding {
            found: found.0,
            supported: supported.0,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn const_new_keeps_the_name() {
        assert_eq!(
            SchemaId::new("canary.project.v1").as_str(),
            "canary.project.v1"
        );
    }

    #[test]
    fn newer_encoding_than_supported_is_rejected() {
        let err = check_encoding(EncodingVersion(2), AUTHORED_ENCODING).unwrap_err();
        assert!(matches!(
            err,
            crate::StateError::UnsupportedEncoding {
                found: 2,
                supported: 1
            }
        ));
    }

    #[test]
    fn same_or_older_encoding_passes() {
        assert!(check_encoding(AUTHORED_ENCODING, AUTHORED_ENCODING).is_ok());
        assert!(check_encoding(EncodingVersion(0), AUTHORED_ENCODING).is_ok());
    }
}
