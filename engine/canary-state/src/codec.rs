//! Serialization abstraction: the traits all formats hide behind.
//!
//! Callers program against [`AuthoredFormat`] and [`SnapshotFormat`], never
//! against `serde_json` or `postcard` directly. Today each trait has one
//! implementation (canonical JSON, postcard); the decision record in
//! ADR 0026 keeps the door open without paying for a second codec now.

use std::path::Path;

use crate::authored::AuthoredDocument;
use crate::error::StateError;
use crate::snapshot::Snapshot;

/// Read/write contract for authored project files.
pub trait AuthoredFormat {
    /// Serializes a document to canonical bytes (JSON today).
    fn to_bytes(document: &AuthoredDocument) -> Result<String, StateError>;

    /// Parses canonical bytes back into a document.
    fn from_bytes(text: &str) -> Result<AuthoredDocument, StateError>;

    /// Atomically saves a document to `path`.
    fn save(document: &AuthoredDocument, path: &Path) -> Result<(), StateError> {
        document.save(path)
    }

    /// Loads a document from `path` (staged checks still owed).
    fn load(path: &Path) -> Result<AuthoredDocument, StateError> {
        AuthoredDocument::load(path)
    }
}

/// The canonical-JSON authored format.
pub struct CanonicalJson;

impl AuthoredFormat for CanonicalJson {
    fn to_bytes(document: &AuthoredDocument) -> Result<String, StateError> {
        document.to_canonical_json()
    }

    fn from_bytes(text: &str) -> Result<AuthoredDocument, StateError> {
        AuthoredDocument::from_canonical_json(text)
    }
}

/// Read/write/verify contract for snapshot payloads.
pub trait SnapshotFormat {
    /// Encodes records (already translated to canonical form) to bytes.
    fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, StateError>;

    /// Decodes and checksum-verifies bytes back into a snapshot.
    fn decode(bytes: &[u8]) -> Result<Snapshot, StateError>;
}

/// The postcard snapshot format.
pub struct PostcardSnapshots;

impl SnapshotFormat for PostcardSnapshots {
    fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, StateError> {
        // Re-encode through the canonical path would need the profile, so
        // this trait serves payloads that already carry their checksum.
        Ok(postcard::to_allocvec(snapshot)?)
    }

    fn decode(bytes: &[u8]) -> Result<Snapshot, StateError> {
        crate::snapshot::decode_snapshot(bytes)
    }
}
