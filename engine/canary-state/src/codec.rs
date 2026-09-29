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
    ///
    /// Local-file-only (see
    /// [`decode_snapshot`](crate::snapshot::decode_snapshot)): untrusted
    /// network bytes must not reach this function until size and nesting
    /// budgets land.
    fn decode(bytes: &[u8]) -> Result<Snapshot, StateError>;
}

/// The postcard snapshot format.
pub struct PostcardSnapshots;

impl SnapshotFormat for PostcardSnapshots {
    /// Re-encodes `snapshot` exactly as held: field order, record order,
    /// and checksum pass through untouched. This is the non-canonical
    /// transport form — checksummed payloads are produced only by
    /// [`encode_snapshot`](crate::snapshot::encode_snapshot), which sorts
    /// records and pins the digest. (A signature change is deliberately out
    /// of scope: the trait cannot take the profile `encode_snapshot` needs,
    /// so canonicalization stays with the free function.)
    fn encode(snapshot: &Snapshot) -> Result<Vec<u8>, StateError> {
        // Re-encode through the canonical path would need the profile, so
        // this trait serves payloads that already carry their checksum.
        Ok(postcard::to_allocvec(snapshot)?)
    }

    fn decode(bytes: &[u8]) -> Result<Snapshot, StateError> {
        crate::snapshot::decode_snapshot(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{encode_snapshot, snapshot_checksum};
    use crate::value::SnapshotValue;
    use std::collections::BTreeMap;

    fn profile() -> crate::snapshot::SnapshotProfile {
        crate::snapshot::SnapshotProfile::new(
            crate::schema::SchemaId::new("canary.snapshot"),
            crate::schema::SchemaVersion(1),
            vec![crate::schema::SchemaId::new("canary.health")],
        )
    }

    fn record(id: u32, hp: i64) -> crate::snapshot::SnapshotRecord {
        crate::snapshot::SnapshotRecord {
            id,
            component: crate::schema::SchemaId::new("canary.health"),
            fields: BTreeMap::from([("hp".to_owned(), SnapshotValue::I64(hp))]),
        }
    }

    #[test]
    fn trait_encode_is_non_canonical_and_decodes_through_the_checksum_gate() {
        // Canonical bytes for two records, in ID order.
        let (canonical_bytes, canonical_sum) =
            encode_snapshot(&profile(), vec![record(2, 20), record(0, 10)]).expect("encode");
        let canonical = PostcardSnapshots::decode(&canonical_bytes).expect("decode");
        assert_eq!(
            canonical
                .records
                .iter()
                .map(|record| record.id)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );

        // The trait encode does not canonicalize: an unsorted hand-built
        // snapshot passes through with its record order (and its checksum
        // field) untouched, byte-identical to a raw postcard encoding.
        let mut unsorted = canonical.clone();
        unsorted.records.reverse();
        // Carry the sorted body's digest so the bytes stay checksum-valid:
        // decode re-sorts before verifying, which is exactly the gate under
        // test. (`decode` clears the envelope checksum it just verified, so
        // re-stamp the digest `encode_snapshot` pinned.)
        unsorted.envelope.checksum = canonical_sum.0.clone();
        let trait_bytes = PostcardSnapshots::encode(&unsorted).expect("trait encode");
        let raw_bytes = postcard::to_allocvec(&unsorted).expect("raw postcard");
        assert_eq!(trait_bytes, raw_bytes);
        let raw: crate::snapshot::Snapshot =
            postcard::from_bytes(&trait_bytes).expect("raw decode");
        assert_eq!(
            raw.records
                .iter()
                .map(|record| record.id)
                .collect::<Vec<_>>(),
            vec![2, 0],
            "trait encode must not sort records"
        );

        // And the checksum gate still applies on the way back in: the
        // unsorted bytes carry the sorted body's digest, so decoding them
        // re-sorts before verifying and succeeds with canonical order —
        // while a checksum edited to match the unsorted body out of band
        // would be the only way to smuggle non-canonical order past decode.
        let round_tripped = PostcardSnapshots::decode(&trait_bytes).expect("decode");
        assert_eq!(
            round_tripped
                .records
                .iter()
                .map(|record| record.id)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
        // The trait never mints checksums: re-pinning needs the profile path.
        let repinned = snapshot_checksum(&round_tripped).expect("re-pin");
        assert_eq!(repinned, canonical_sum);
    }
}
