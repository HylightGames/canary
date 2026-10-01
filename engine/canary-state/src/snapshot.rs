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
use crate::schema::{check_encoding, SchemaId, SchemaVersion, SnapshotEnvelope, SNAPSHOT_ENCODING};
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
///
/// Keys are the full `(index, generation)` tuple: generations are `u64`, so
/// no lossless `u64` packing exists and any hash would risk aliasing two
/// live entities into one canonical ID.
#[derive(Debug, Default)]
pub struct RemapTable {
    live_to_canonical: HashMap<(u32, u64), u32>,
    next: u32,
}

impl RemapTable {
    /// Returns the canonical ID for the live `(index, generation)` handle,
    /// assigning a fresh one (`0, 1, 2, …`) on first sight.
    pub fn assign(&mut self, index: u32, generation: u64) -> u32 {
        if let Some(id) = self.live_to_canonical.get(&(index, generation)) {
            return *id;
        }
        let id = self.next;
        self.next += 1;
        self.live_to_canonical.insert((index, generation), id);
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

    /// Restores a generator from previously captured [`OwnedRng::state`].
    /// The stream continues exactly where the captured one left off: the
    /// inner splitmix64 word is the whole state, so no seed expansion or
    /// OS randomness is involved.
    #[must_use]
    pub fn from_state(state: u64) -> Self {
        Self(state)
    }

    /// Captures the current stream position for a snapshot. Feed the result
    /// back through [`OwnedRng::from_state`] on restore to continue the
    /// same deterministic sequence across a save/restore boundary.
    #[must_use]
    pub fn state(&self) -> u64 {
        self.0
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

/// Renders a SHA-256 digest as 64 lowercase hex characters.
///
/// Encoded by hand rather than via a `hex` dependency — it is a few
/// lines, and a whole crate for it would be a pin to maintain against
/// the workspace's transitive-pin policy for nothing (the same
/// rationale as `canary-assets`' hand-rolled `AssetId::to_hex`).
/// `sha2` 0.11's digest output carries no `LowerHex` impl,
/// so this takes the digest as bytes.
pub(crate) fn hex_digest(digest: &[u8]) -> String {
    const ALPHABET: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(ALPHABET[(byte >> 4) as usize] as char);
        out.push(ALPHABET[(byte & 0x0F) as usize] as char);
    }
    out
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
    let digest = hex_digest(&Sha256::digest(&bytes));
    let mut snapshot: Snapshot = postcard::from_bytes(&bytes)?;
    snapshot.envelope.checksum = digest.clone();
    bytes = postcard::to_allocvec(&snapshot)?;
    Ok((bytes, SnapshotChecksum(digest)))
}

/// Recomputes the canonical checksum of an already-decoded [`Snapshot`]:
/// records sort by ID, the envelope checksum clears, the body
/// postcard-encodes, SHA-256 digests it. This is the same canonicalization
/// [`decode_snapshot`] verifies against, factored out so callers holding a
/// [`Snapshot`] (rather than bytes) can re-pin or compare checksums without
/// re-encoding through a profile.
pub fn snapshot_checksum(snapshot: &Snapshot) -> Result<SnapshotChecksum, StateError> {
    let mut canonical = snapshot.clone();
    canonical.records.sort_by_key(|record| record.id);
    canonical.envelope.checksum.clear();
    let bytes = postcard::to_allocvec(&canonical)?;
    Ok(SnapshotChecksum(hex_digest(&Sha256::digest(&bytes))))
}

/// Decodes and verifies: checksum first, then postcard, then envelope.
/// A corrupt or non-canonical payload fails before any record is trusted.
///
/// Local-file-only: `bytes` must come from [`save_snapshot`] /
/// [`load_snapshot`] (or an equivalent trusted-local write). Untrusted
/// network bytes must not reach this function until size and nesting budgets
/// land (planned envelope work): decoding runs before verification, so a
/// hostile sender can force allocation first.
///
/// The checksum gate runs before the encoding gate on purpose: a truncated
/// or hand-edited file fails as [`StateError::ChecksumMismatch`] (or a typed
/// codec error) regardless of which byte was touched, while a
/// checksum-valid payload under an encoding this build cannot read fails as
/// [`StateError::UnsupportedEncoding`].
pub fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, StateError> {
    let snapshot: Snapshot = postcard::from_bytes(bytes).map_err(StateError::from)?;
    let claimed = snapshot.envelope.checksum.clone();
    let mut canonical = snapshot.clone();
    // Canonical order is part of the contract: re-sort before verifying so
    // a hand-assembled unsorted payload cannot carry a matching checksum.
    canonical.records.sort_by_key(|record| record.id);
    canonical.envelope.checksum.clear();
    let canonical_bytes = postcard::to_allocvec(&canonical)?;
    let computed = hex_digest(&Sha256::digest(&canonical_bytes));
    if computed != claimed {
        return Err(StateError::ChecksumMismatch {
            expected: claimed,
            computed,
        });
    }
    check_encoding(snapshot.envelope.encoding, SNAPSHOT_ENCODING)?;
    Ok(canonical)
}

/// Reserved component schema for the deterministic sim-core record: the
/// simulation tick, clock, and owned RNG position that travel inside a
/// snapshot's record list. Always declared in a snapshot profile (see the
/// runtime registry), so checksums accept it; restore partitions it out
/// before entity allocation, so it never gains a runtime entity.
pub const SIM_STATE_SCHEMA: &str = "canary.sim-state";

/// Reserved snapshot-local ID for the sim-core record. It sorts after every
/// entity ID by construction, so canonical order keeps it last without a
/// special case in the sort.
pub const SIM_STATE_ID: u32 = u32::MAX;

/// Deterministic sim-core state: the three values a save/restore round-trip
/// must carry so the simulation continues deterministically on the other
/// side — the completed-step count, the simulation clock (all `Duration`s
/// as whole nanoseconds, saturating at `u64::MAX`), and the owned RNG
/// stream position (see [`OwnedRng::state`]). Wall-clock frame identity and
/// input history are not simulation state and travel nowhere here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimStateSnapshot {
    /// Completed simulation steps.
    pub tick: u64,
    /// Accumulated simulation time, whole nanoseconds.
    pub sim_time_nanos: u64,
    /// The `dt` that advanced the most recent step, whole nanoseconds.
    pub step_nanos: u64,
    /// The input frame that drove the most recent step.
    pub frame_index: u64,
    /// Owned RNG stream position at capture time.
    pub rng_state: u64,
}

impl SimStateSnapshot {
    /// Field keys of the sim-core record, in canonical order.
    const TICK_KEY: &'static str = "tick";
    const SIM_TIME_KEY: &'static str = "sim_time_nanos";
    const STEP_KEY: &'static str = "step_nanos";
    const FRAME_KEY: &'static str = "frame_index";
    const RNG_KEY: &'static str = "rng_state";

    /// Packs this state into the reserved sim-core record. Durations arrive
    /// as `std::time::Duration` and saturate to whole nanoseconds.
    #[must_use]
    pub fn to_record(
        tick: u64,
        sim_time: std::time::Duration,
        step: std::time::Duration,
        frame_index: u64,
        rng_state: u64,
    ) -> SnapshotRecord {
        Self {
            tick,
            sim_time_nanos: sim_time.as_nanos().min(u128::from(u64::MAX)) as u64,
            step_nanos: step.as_nanos().min(u128::from(u64::MAX)) as u64,
            frame_index,
            rng_state,
        }
        .into_record()
    }

    /// Packs already-nanos state into the reserved sim-core record.
    fn into_record(self) -> SnapshotRecord {
        SnapshotRecord {
            id: SIM_STATE_ID,
            component: SchemaId::new(SIM_STATE_SCHEMA),
            fields: BTreeMap::from([
                (Self::TICK_KEY.to_owned(), SnapshotValue::U64(self.tick)),
                (
                    Self::SIM_TIME_KEY.to_owned(),
                    SnapshotValue::U64(self.sim_time_nanos),
                ),
                (
                    Self::STEP_KEY.to_owned(),
                    SnapshotValue::U64(self.step_nanos),
                ),
                (
                    Self::FRAME_KEY.to_owned(),
                    SnapshotValue::U64(self.frame_index),
                ),
                (Self::RNG_KEY.to_owned(), SnapshotValue::U64(self.rng_state)),
            ]),
        }
    }

    /// Unpacks and validates the reserved sim-core record: exact reserved
    /// ID and schema, all five fields present as `U64`. Anything else is a
    /// typed error — a corrupt or hand-assembled record never seeds the RNG.
    pub fn from_record(record: &SnapshotRecord) -> Result<Self, StateError> {
        let invalid = |reason: String| StateError::MigrationInvalid {
            schema: SIM_STATE_SCHEMA.to_owned(),
            to: 0,
            reason,
        };
        if record.id != SIM_STATE_ID {
            return Err(invalid(format!(
                "sim-state record carries id {}, expected reserved {SIM_STATE_ID}",
                record.id
            )));
        }
        if record.component.as_str() != SIM_STATE_SCHEMA {
            return Err(invalid(format!(
                "sim-state record names schema '{}', expected '{SIM_STATE_SCHEMA}'",
                record.component.as_str()
            )));
        }
        let field = |key: &str| match record.fields.get(key) {
            Some(SnapshotValue::U64(value)) => Ok(*value),
            other => Err(invalid(format!(
                "sim-state field '{key}' must be a U64, got {other:?}"
            ))),
        };
        Ok(Self {
            tick: field(Self::TICK_KEY)?,
            sim_time_nanos: field(Self::SIM_TIME_KEY)?,
            step_nanos: field(Self::STEP_KEY)?,
            frame_index: field(Self::FRAME_KEY)?,
            rng_state: field(Self::RNG_KEY)?,
        })
    }

    /// Rebuilds whole-nanosecond durations, saturating at `u64::MAX` nanos.
    #[must_use]
    pub fn sim_time(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.sim_time_nanos)
    }

    /// Rebuilds the last-step duration.
    #[must_use]
    pub fn step(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.step_nanos)
    }
}

/// Atomically persists canonical snapshot bytes: complete bytes to a sibling
/// temporary file, platform flush, then rename over `path`. A crash before
/// the rename leaves the previous file untouched; a stale temporary file
/// from such a crash is fully overwritten by the next save, never merged.
/// Filesystem durability varies by platform (see the authored-save notes);
/// what this promises is atomic replacement, not a universal flush barrier.
/// Temp suffix for snapshot saves (see [`crate::authored::atomic_write`]).
/// Appending (not extension-replacing) keeps sibling temp names distinct
/// per file, so a snapshot and a project file sharing a directory never
/// collide even when their stems match.
const SNAPSHOT_TEMP_SUFFIX: &str = ".tmp";

/// Atomically persists canonical snapshot bytes: complete bytes to a sibling
/// temporary file, platform flush, then rename over `path`. A crash before
/// the rename leaves the previous file untouched; a stale temporary file
/// from such a crash is fully overwritten by the next save, never merged.
/// Filesystem durability varies by platform (see the authored-save notes);
/// what this promises is atomic replacement, not a universal flush barrier.
pub fn save_snapshot(path: &std::path::Path, bytes: &[u8]) -> Result<(), StateError> {
    crate::authored::atomic_write(path, bytes, SNAPSHOT_TEMP_SUFFIX)
}

/// Loads snapshot bytes previously written by [`save_snapshot`]. The payload
/// still owes its checksum gate ([`decode_snapshot`]) before any caller
/// trusts or restores it; a truncated or hand-edited file fails there with
/// a typed error instead of yielding partial state.
///
/// Local-file-only, like [`decode_snapshot`]: the path must be a
/// trusted-local save file. Bytes fetched from an untrusted network peer
/// must not be written here and then loaded as if they were a save file
/// until size and nesting budgets land.
pub fn load_snapshot(path: &std::path::Path) -> Result<Vec<u8>, StateError> {
    std::fs::read(path).map_err(|error| StateError::File {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
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
        let (bytes, _) = encode_snapshot(&profile(), vec![record(0)]).expect("encode");
        // One flipped bit anywhere — envelope head, middle, tail — must
        // fail typed, so no tamper position slips through a partial gate.
        for position in [0, bytes.len() / 2, bytes.len() - 1] {
            let mut tampered = bytes.clone();
            tampered[position] ^= 0x01;
            assert!(
                matches!(
                    decode_snapshot(&tampered).unwrap_err(),
                    StateError::ChecksumMismatch { .. } | StateError::SnapshotCodec(_)
                ),
                "tamper at byte {position} passed verification"
            );
        }
    }

    #[test]
    fn checksum_valid_payload_under_newer_encoding_is_rejected() {
        use crate::schema::EncodingVersion;
        let (bytes, _) = encode_snapshot(&profile(), vec![record(0)]).expect("encode");
        let mut snapshot = decode_snapshot(&bytes).expect("decode");
        // A checksum-valid payload under an encoding this build cannot
        // read: re-stamp the checksum over the bumped envelope so the
        // failure is the encoding gate, not the checksum gate.
        snapshot.envelope.encoding = EncodingVersion(SNAPSHOT_ENCODING.0 + 1);
        let sum = snapshot_checksum(&snapshot).expect("re-stamp");
        snapshot.envelope.checksum = sum.0;
        let bumped = postcard::to_allocvec(&snapshot).expect("re-encode");
        assert!(matches!(
            decode_snapshot(&bumped).unwrap_err(),
            StateError::UnsupportedEncoding { .. }
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
    fn checksum_helper_agrees_with_encode_and_notices_tampering() {
        let (bytes, encoded_sum) =
            encode_snapshot(&profile(), vec![record(2), record(0)]).expect("encode");
        let snapshot = decode_snapshot(&bytes).expect("decode");
        let recomputed = snapshot_checksum(&snapshot).expect("checksum");
        assert_eq!(recomputed, encoded_sum);
        let mut tampered = snapshot.clone();
        tampered.records[0]
            .fields
            .insert("hp".to_owned(), SnapshotValue::I64(999));
        let tampered_sum = snapshot_checksum(&tampered).expect("checksum");
        assert_ne!(tampered_sum, encoded_sum);
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
        assert_eq!(table.assign(9, 0), 0);
        assert_eq!(table.assign(0, 7), 1);
        assert_eq!(table.assign(9, 0), 0);
    }

    #[test]
    fn remap_keys_the_full_index_generation_tuple() {
        let mut table = RemapTable::default();
        // Same slot recycled across generations: every handle is distinct.
        // A `u64` hash of the pair could alias two of these into one
        // canonical ID; the tuple key cannot.
        let first = table.assign(3, 0);
        let second = table.assign(3, 1);
        let third = table.assign(4, 0);
        let extreme = table.assign(u32::MAX, u64::MAX);
        assert_ne!(first, second, "generation distinguishes handles");
        assert_ne!(first, third, "index distinguishes handles");
        assert_ne!(second, extreme, "extremes assign distinctly");
        assert_eq!(table.assign(3, 0), first, "repeat sightings reuse the ID");
        assert_eq!(
            table.assign(u32::MAX, u64::MAX),
            extreme,
            "repeat extremes reuse the ID"
        );
    }

    #[test]
    fn remap_survives_slot_recycling_storms() {
        // Hundreds of generations churn through one slot while neighboring
        // slots interleave: every `(index, generation)` sighting must keep
        // its own canonical ID, and repeat sightings must reuse it.
        let mut table = RemapTable::default();
        let mut first_sight: Vec<u32> = Vec::new();
        for generation in 0..500u64 {
            first_sight.push(table.assign(7, generation));
            // Neighboring slots churn alongside so no two tuples alias.
            let _ = table.assign(generation as u32, 0);
        }
        let distinct: std::collections::HashSet<u32> = first_sight.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            first_sight.len(),
            "recycled generations must never alias"
        );
        for (generation, expected) in first_sight.iter().enumerate() {
            assert_eq!(
                table.assign(7, generation as u64),
                *expected,
                "repeat sightings reuse the ID"
            );
        }
        assert_eq!(table.assign(7, 0), first_sight[0]);
        let extreme = table.assign(u32::MAX, u64::MAX);
        assert!(
            !first_sight.contains(&extreme),
            "extreme handles assign outside the storm range"
        );
        assert_eq!(table.assign(u32::MAX, u64::MAX), extreme);
    }

    #[test]
    fn missing_snapshot_file_is_a_typed_error() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "canary-snapshot-missing-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let err = load_snapshot(&dir.join("never-saved.bin")).unwrap_err();
        assert!(
            matches!(err, StateError::File { .. }),
            "missing file must fail typed, got {err:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rng_state_continues_the_same_stream() {
        let mut rng = OwnedRng::from_seed(7);
        let _ = rng.next_u64();
        let _ = rng.next_u64();
        let resumed = OwnedRng::from_state(rng.state());
        let mut resumed = resumed;
        assert_eq!(resumed.next_u64(), rng.next_u64());
    }

    #[test]
    fn sim_state_record_round_trips_through_canonical_bytes() {
        use std::time::Duration;
        let sim_record = SimStateSnapshot::to_record(
            41,
            Duration::from_millis(656),
            Duration::from_millis(16),
            40,
            0xDEAD_BEEF,
        );
        assert_eq!(sim_record.id, SIM_STATE_ID);
        assert_eq!(sim_record.component.as_str(), SIM_STATE_SCHEMA);
        let state = SimStateSnapshot::from_record(&sim_record).expect("unpack");
        assert_eq!(state.tick, 41);
        assert_eq!(state.sim_time(), Duration::from_millis(656));
        assert_eq!(state.step(), Duration::from_millis(16));
        assert_eq!(state.frame_index, 40);
        assert_eq!(OwnedRng::from_state(state.rng_state).next_u64(), {
            let mut rng = OwnedRng::from_state(0xDEAD_BEEF);
            rng.next_u64()
        });

        // The reserved record rides inside an ordinary snapshot and still
        // verifies: profiles always declare the reserved schema.
        let profile = SnapshotProfile::new(
            SchemaId::new("canary.snapshot"),
            SchemaVersion(1),
            vec![
                SchemaId::new("canary.health"),
                SchemaId::new(SIM_STATE_SCHEMA),
            ],
        );
        let health = record(0);
        let (bytes, _) =
            encode_snapshot(&profile, vec![health, sim_record]).expect("encode with sim-state");
        let snapshot = decode_snapshot(&bytes).expect("decode");
        assert_eq!(snapshot.records.len(), 2);
        assert_eq!(
            snapshot.records.last().expect("sim-state sorts last").id,
            SIM_STATE_ID
        );
    }

    #[test]
    fn sim_state_rejects_malformed_records() {
        let mut record = SimStateSnapshot::to_record(
            1,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            0,
            0,
        );
        record.fields.remove("tick");
        assert!(matches!(
            SimStateSnapshot::from_record(&record).unwrap_err(),
            StateError::MigrationInvalid { .. }
        ));
        record = SimStateSnapshot::to_record(
            1,
            std::time::Duration::ZERO,
            std::time::Duration::ZERO,
            0,
            0,
        );
        record.id = 3;
        assert!(matches!(
            SimStateSnapshot::from_record(&record).unwrap_err(),
            StateError::MigrationInvalid { .. }
        ));
    }

    #[test]
    fn interrupted_snapshot_save_leaves_the_last_good_file_recoverable() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "canary-snapshot-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("sim.bin");

        let (good_bytes, _) = encode_snapshot(&profile(), vec![record(0)]).expect("encode");
        save_snapshot(&path, &good_bytes).expect("first save");

        // Crashed writer: partial sibling temp, no rename. The committed
        // file still loads and verifies. The temp name appends the suffix
        // (`sim.bin.tmp`), never replacing the extension — see
        // `crate::authored::atomic_write`.
        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, &good_bytes[..good_bytes.len() / 2]).expect("plant partial");
        let recovered = load_snapshot(&path).expect("recover");
        decode_snapshot(&recovered).expect("good file verifies");

        // Next save overwrites the stale temp and renames; a truncated
        // final file then fails typed at the checksum gate.
        let (next_bytes, _) =
            encode_snapshot(&profile(), vec![record(0), record(1)]).expect("encode");
        save_snapshot(&path, &next_bytes).expect("second save");
        assert!(!tmp.exists(), "rename consumes the temp file");
        std::fs::write(&path, &next_bytes[..next_bytes.len() / 2]).expect("truncate");
        let broken = load_snapshot(&path).expect("truncated file still reads");
        assert!(matches!(
            decode_snapshot(&broken).unwrap_err(),
            StateError::ChecksumMismatch { .. } | StateError::SnapshotCodec(_)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
