//! Deterministic simulation snapshots: postcard payloads, SHA-256 checksums.
//!
//! A [`Snapshot`] is the machine counterpart to an [`AuthoredDocument`](crate::authored::AuthoredDocument):
//! not human-readable, but canonical — the same world state always encodes
//! to the same bytes, and the [`SnapshotChecksum`] pins those bytes. The
//! checksum covers the postcard encoding of the body; envelopes carry it so
//! corruption fails at the boundary with [`StateError::ChecksumMismatch`](crate::StateError).
//!
//! Canonicality rules, all enforced here:
//!
//! - Records sort by snapshot-local ID before encoding.
//! - Snapshot-local IDs are small deterministic `u32`s assigned per encode.
//!   Live runtime handles never enter a payload; callers translate them
//!   through a [`RemapTable`] at the boundary.
//! - Component schemas must be declared in the [`SnapshotProfile`];
//!   anything else is [`StateError::UndeclaredComponent`](crate::StateError).
//! - [`SnapshotValue`] has no `usize` and no NaN: floats must be finite,
//!   checked recursively at encode time. (`serde_json` already rejects NaN,
//!   so the JSON layer needs no extra gate.)
//! - [`OwnedRng`] is a self-contained splitmix64. Snapshot code never
//!   touches OS randomness — determinism must not depend on entropy.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::StateError;
use crate::schema::{SchemaId, SchemaVersion, SnapshotEnvelope, SNAPSHOT_ENCODING};
use crate::value::SnapshotValue;

/// Declared shape of one snapshot stream: which schema, which version, and
/// exactly which component schemas may appear in records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotProfile {
    /// Schema of the snapshot body itself.
    pub schema: SchemaId,
    /// Version of the snapshot body.
    pub version: SchemaVersion,
    /// Component schemas records may reference.
    pub components: Vec<SchemaId>,
}

impl SnapshotProfile {
    /// Declares a profile. Component order is irrelevant; it is not encoded.
    #[must_use]
    pub fn new(schema: SchemaId, version: SchemaVersion, components: Vec<SchemaId>) -> Self {
        Self {
            schema,
            version,
            components,
        }
    }

    /// Whether `component` may appear in a record under this profile.
    #[must_use]
    pub fn declares(&self, component: &SchemaId) -> bool {
        self.components.iter().any(|c| c == component)
    }
}

/// One component instance on one snapshot-local entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    /// Canonical snapshot-local entity ID, assigned per encode.
    pub id: u32,
    /// Component schema, must be declared in the profile.
    pub component: SchemaId,
    /// Field values, in key order.
    pub fields: BTreeMap<String, SnapshotValue>,
}

/// Hex SHA-256 over the canonical postcard body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotChecksum(pub String);

/// The encoded unit: envelope plus canonical records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Envelope with schema, version, encoding, and checksum.
    pub envelope: SnapshotEnvelope,
    /// Records in canonical (ID-sorted) order.
    pub records: Vec<SnapshotRecord>,
}

/// Translates live runtime handles to canonical snapshot-local IDs at the
/// encode boundary. Assignment order defines the IDs, so callers that need
/// stable snapshots must feed handles in a deterministic order.
#[derive(Debug, Default)]
pub struct RemapTable {
    live_to_canonical: HashMap<u64, u32>,
    next: u32,
}

impl RemapTable {
    /// Returns the canonical ID for `live`, assigning a fresh one (`0, 1,
    /// 2, …`) on first sight.
    pub fn assign(&mut self, live: u64) -> u32 {
        if let Some(id) = self.live_to_canonical.get(&live) {
            return *id;
        }
        let id = self.next;
        self.next += 1;
        self.live_to_canonical.insert(live, id);
        id
    }
}

/// Splitmix64: self-contained deterministic RNG for snapshot code. Never
/// touches OS randomness; two instances from the same seed agree forever.
#[derive(Debug, Clone)]
pub struct OwnedRng(u64);

impl OwnedRng {
    /// Seeds the generator. Any `u64` is a valid seed, including zero.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        Self(seed)
    }

    /// Next deterministic `u64`.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Encodes `records` under `profile`: declares-check, validates values,
/// sorts by ID, postcard-encodes, checksums. Returns the bytes plus the
/// checksum that belongs in the envelope.
pub fn encode_snapshot(
    profile: &SnapshotProfile,
    mut records: Vec<SnapshotRecord>,
) -> Result<(Vec<u8>, SnapshotChecksum), StateError> {
    for record in &records {
        if !profile.declares(&record.component) {
            return Err(StateError::UndeclaredComponent(
                record.component.as_str().to_owned(),
            ));
        }
        record
            .fields
            .values()
            .try_for_each(SnapshotValue::validate)?;
    }
    records.sort_by_key(|record| record.id);
    let envelope = SnapshotEnvelope {
        schema: profile.schema.clone(),
        version: profile.version,
        encoding: SNAPSHOT_ENCODING,
        checksum: String::new(),
    };
    let snapshot = Snapshot { envelope, records };
    let mut bytes = postcard::to_allocvec(&snapshot)?;
    // Checksum covers the body with an empty checksum field, so verification
    // recomputes over identical bytes. Rewrite the envelope with the digest.
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let mut snapshot: Snapshot = postcard::from_bytes(&bytes)?;
    snapshot.envelope.checksum = digest.clone();
    bytes = postcard::to_allocvec(&snapshot)?;
    Ok((bytes, SnapshotChecksum(digest)))
}

/// Decodes and verifies: checksum first, then postcard, then envelope.
/// A corrupt or non-canonical payload fails before any record is trusted.
pub fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, StateError> {
    let snapshot: Snapshot = postcard::from_bytes(bytes).map_err(StateError::from)?;
    let claimed = snapshot.envelope.checksum.clone();
    let mut canonical = snapshot.clone();
    // Canonical order is part of the contract: re-sort before verifying so
    // a hand-assembled unsorted payload cannot carry a matching checksum.
    canonical.records.sort_by_key(|record| record.id);
    canonical.envelope.checksum.clear();
    let canonical_bytes = postcard::to_allocvec(&canonical)?;
    let computed = format!("{:x}", Sha256::digest(&canonical_bytes));
    if computed != claimed {
        return Err(StateError::ChecksumMismatch {
            expected: claimed,
            computed,
        });
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> SnapshotProfile {
        SnapshotProfile::new(
            SchemaId::new("canary.snapshot"),
            SchemaVersion(1),
            vec![SchemaId::new("canary.health")],
        )
    }

    fn record(id: u32) -> SnapshotRecord {
        SnapshotRecord {
            id,
            component: SchemaId::new("canary.health"),
            fields: BTreeMap::from([("hp".to_owned(), SnapshotValue::I64(10))]),
        }
    }

    #[test]
    fn encode_is_canonical_regardless_of_input_order() {
        let (first, first_sum) =
            encode_snapshot(&profile(), vec![record(2), record(0), record(1)]).expect("encode");
        let (second, second_sum) =
            encode_snapshot(&profile(), vec![record(0), record(1), record(2)]).expect("encode");
        assert_eq!(first, second);
        assert_eq!(first_sum, second_sum);
    }

    #[test]
    fn decode_verifies_checksum_and_round_trips() {
        let (bytes, _) = encode_snapshot(&profile(), vec![record(0)]).expect("encode");
        let snapshot = decode_snapshot(&bytes).expect("decode");
        assert_eq!(snapshot.records, vec![record(0)]);
    }

    #[test]
    fn nested_component_data_round_trips() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "transform".to_owned(),
            SnapshotValue::Map(BTreeMap::from([
                (
                    "pos".to_owned(),
                    SnapshotValue::List(vec![
                        SnapshotValue::F64(1.5),
                        SnapshotValue::F64(-2.25),
                        SnapshotValue::F64(0.0),
                    ]),
                ),
                ("parent".to_owned(), SnapshotValue::Null),
            ])),
        );
        fields.insert("name".to_owned(), SnapshotValue::Str("player".to_owned()));
        let record = SnapshotRecord {
            id: 0,
            component: SchemaId::new("canary.health"),
            fields,
        };
        let (bytes, _) = encode_snapshot(&profile(), vec![record.clone()]).expect("encode");
        let snapshot = decode_snapshot(&bytes).expect("decode");
        assert_eq!(snapshot.records, vec![record]);
    }

    #[test]
    fn golden_bytes_are_platform_stable() {
        // Exact postcard bytes for a minimal snapshot. If this changes, the
        // wire format changed: say so in ADR 0026, do not just re-bless.
        // postcard varints are endianness-independent by construction.
        const GOLDEN: &[u8] = &[
            15, 99, 97, 110, 97, 114, 121, 46, 115, 110, 97, 112, 115, 104, 111, 116, 1, 1, 64, 57,
            53, 97, 51, 99, 48, 102, 54, 97, 99, 49, 97, 102, 48, 101, 49, 100, 57, 54, 54, 97, 55,
            97, 102, 52, 55, 51, 51, 57, 50, 101, 55, 54, 99, 51, 48, 48, 55, 56, 48, 53, 101, 100,
            51, 51, 50, 100, 102, 56, 48, 102, 99, 97, 101, 98, 50, 54, 55, 53, 98, 56, 50, 97, 48,
            1, 0, 13, 99, 97, 110, 97, 114, 121, 46, 104, 101, 97, 108, 116, 104, 1, 2, 104, 112,
            2, 20,
        ];
        let (bytes, checksum) = encode_snapshot(&profile(), vec![record(0)]).expect("encode");
        assert_eq!(bytes, GOLDEN, "wire format changed");
        assert_eq!(checksum.0.len(), 64);
        let again = decode_snapshot(&bytes).expect("decode");
        assert_eq!(again.records, vec![record(0)]);
    }

    #[test]
    fn tampered_bytes_fail_the_checksum() {
        let (mut bytes, _) = encode_snapshot(&profile(), vec![record(0)]).expect("encode");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert!(matches!(
            decode_snapshot(&bytes).unwrap_err(),
            StateError::ChecksumMismatch { .. } | StateError::SnapshotCodec(_)
        ));
    }

    #[test]
    fn undeclared_components_are_refused() {
        let mut bad = record(0);
        bad.component = SchemaId::new("canary.unknown");
        let err = encode_snapshot(&profile(), vec![bad]).unwrap_err();
        assert!(matches!(err, StateError::UndeclaredComponent(_)));
    }

    #[test]
    fn nan_has_no_canonical_encoding() {
        let mut bad = record(0);
        bad.fields
            .insert("x".to_owned(), SnapshotValue::F64(f64::NAN));
        assert!(encode_snapshot(&profile(), vec![bad]).is_err());
    }

    #[test]
    fn rng_is_deterministic_per_seed() {
        let mut first = OwnedRng::from_seed(42);
        let mut second = OwnedRng::from_seed(42);
        for _ in 0..16 {
            assert_eq!(first.next_u64(), second.next_u64());
        }
        let mut third = OwnedRng::from_seed(43);
        assert_ne!(first.next_u64(), third.next_u64());
    }

    #[test]
    fn remap_assigns_stable_ids() {
        let mut table = RemapTable::default();
        assert_eq!(table.assign(99), 0);
        assert_eq!(table.assign(7), 1);
        assert_eq!(table.assign(99), 0);
    }
}
