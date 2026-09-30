//! Canonical snapshots and sequenced deltas (ADR 0027, point 3).
//!
//! A [`Snapshot`] is the authoritative baseline each connection starts from;
//! a [`Delta`] advances a known baseline by naming its [`NetSequence`] base
//! plus the [`SimTick`] its changes were captured at. Three counters stay
//! distinct across these types: the wire session sequence (`sequence` /
//! `base_sequence`), the simulation step count (`sim_tick`), and the
//! scheduler tick values recorded inside tombstones (see
//! [`crate::tombstone`]) — never compare one domain against another.
//!
//! Wire discipline, mirroring the `.14` snapshot path without reusing its
//! types (authored documents, simulation snapshots, and wire messages are
//! separate formats — a `canary-state` snapshot never goes on the wire
//! verbatim; replication envelopes are always constructed around replicated
//! payloads from `(network entity, schema, bytes)` triples):
//!
//! - Entries sort by `(network entity id, schema id)` before encoding, so
//!   arrival order cannot change application order; the sort-then-checksum
//!   rule is the same one `canary-state` snapshots follow.
//! - The receiver validates the *whole* delta before applying anything: an
//!   unknown base fails as [`NetError::UnknownBaseSequence`], unsorted or
//!   duplicated entries fail as [`NetError::UnsortedEntries`] /
//!   [`NetError::DuplicateEntry`]. A gap causes a full resync, never a
//!   partial apply onto an unknown baseline.
//! - Payloads stay opaque `Vec<u8>` here: per-schema codecs that produce
//!   and interpret them ship in [`crate::codec`]. This module proves the
//!   ordering, basing, and validation representation those bytes ride in.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::NetError;
use crate::ids::{NetEntityId, NetSequence, SimTick};
use crate::limits::NetLimits;
use crate::tombstone::Tombstone;

/// One replicated component value: which entity, which schema, what bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicatedEntry {
    /// Server-scoped entity identity; never a runtime handle.
    pub entity: NetEntityId,
    /// Stable schema id naming the component (e.g. `"canary:transform/position@1"`).
    pub schema: String,
    /// Opaque component payload. Produced by per-schema codecs (WP3);
    /// untrusted until the enclosing message validates.
    pub payload: Vec<u8>,
}

impl ReplicatedEntry {
    /// Canonical ordering key: network entity first, then schema.
    #[must_use]
    pub fn sort_key(&self) -> (u64, &str) {
        (self.entity.0, self.schema.as_str())
    }
}

/// Envelope opening every wire snapshot: which simulation step it captures
/// plus the checksum binding its canonical bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEnvelope {
    /// Simulation step the snapshot was captured at.
    pub sim_tick: SimTick,
    /// SHA-256 over the canonical encoding (entries sorted, this field
    /// zeroed) — the `.14` sort-then-checksum discipline, applied to wire
    /// bytes rather than snapshot files.
    pub checksum: [u8; 32],
}

/// The canonical initial snapshot: one authoritative baseline per connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Capture step plus integrity checksum.
    pub envelope: SnapshotEnvelope,
    /// Component values in canonical `(entity, schema)` order.
    pub entries: Vec<ReplicatedEntry>,
}

/// Sorts `entries` into canonical order in place.
fn sort_entries(entries: &mut [ReplicatedEntry]) {
    entries.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
}

/// SHA-256 over the canonical encoding of `envelope` + `entries`: entries
/// sorted, checksum field zeroed — identical bytes for identical logical
/// content regardless of insertion order.
fn snapshot_checksum(sim_tick: SimTick, entries: &[ReplicatedEntry]) -> Result<[u8; 32], NetError> {
    let mut canonical = entries.to_vec();
    sort_entries(&mut canonical);
    let snapshot = Snapshot {
        envelope: SnapshotEnvelope {
            sim_tick,
            checksum: [0u8; 32],
        },
        entries: canonical,
    };
    let bytes = postcard::to_allocvec(&snapshot)?;
    Ok(Sha256::digest(&bytes).into())
}

/// Encodes a canonical snapshot: sorts entries by `(entity, schema)`,
/// checksums the canonical bytes, and rejects over-limit snapshots before
/// they reach the transport.
pub fn encode_snapshot(
    mut entries: Vec<ReplicatedEntry>,
    sim_tick: SimTick,
    limits: &NetLimits,
) -> Result<Vec<u8>, NetError> {
    sort_entries(&mut entries);
    let checksum = snapshot_checksum(sim_tick, &entries)?;
    let snapshot = Snapshot {
        envelope: SnapshotEnvelope { sim_tick, checksum },
        entries,
    };
    let bytes = postcard::to_allocvec(&snapshot)?;
    let len = u32::try_from(bytes.len()).map_err(|_| NetError::OversizeFrame {
        claimed: u32::MAX,
        max: limits.max_message_bytes,
    })?;
    limits.check_frame_len(len)?;
    Ok(bytes)
}

/// Decodes and verifies a snapshot: structural decode, trailing-byte
/// rejection, then the checksum gate over the re-sorted canonical form — so
/// a hand-assembled unsorted payload cannot carry a matching checksum, and
/// no record is trusted before the whole message verifies.
pub fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, NetError> {
    let (snapshot, remainder): (Snapshot, &[u8]) = postcard::take_from_bytes(bytes)?;
    if !remainder.is_empty() {
        return Err(NetError::TrailingBytes {
            trailing: remainder.len(),
        });
    }
    let claimed = snapshot.envelope.checksum;
    let computed = snapshot_checksum(snapshot.envelope.sim_tick, &snapshot.entries)?;
    if computed != claimed {
        return Err(NetError::ChecksumMismatch {
            expected: claimed,
            computed,
        });
    }
    let mut snapshot = snapshot;
    sort_entries(&mut snapshot.entries);
    Ok(snapshot)
}

/// An authoritative delta against a known baseline: the changes since the
/// base sequence plus the tombstones the client's cursor has not yet
/// acknowledged (see [`crate::tombstone::TombstoneLog::pending_since`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    /// Wire session sequence of this delta; gated by `SequenceGate` at the
    /// envelope layer before anything here runs.
    pub sequence: NetSequence,
    /// Newest sequence the receiver must have fully applied. Anything else
    /// fails as [`NetError::UnknownBaseSequence`] — resync, never partial
    /// apply.
    pub base_sequence: NetSequence,
    /// Simulation step the changes were captured at.
    pub sim_tick: SimTick,
    /// Changed component values, canonical `(entity, schema)` order.
    pub changes: Vec<ReplicatedEntry>,
    /// Removals and destructions, canonical tombstone order.
    pub removals: Vec<Tombstone>,
}

impl Delta {
    /// Validates the whole delta before the caller applies anything:
    /// first the base sequence against the newest fully-applied sequence,
    /// then canonical ordering and key uniqueness of both lists. Either
    /// failure rejects the entire message — the caller must never apply a
    /// prefix of it.
    pub fn validate_all(&self, last_applied: NetSequence) -> Result<(), NetError> {
        self.validate_base(last_applied)?;
        Self::validate_order(&self.changes, &self.removals)
    }

    /// The base must name exactly the newest applied sequence: older means
    /// the sender's baseline predates ours, newer means we missed deltas —
    /// both converge only through a fresh snapshot.
    pub fn validate_base(&self, last_applied: NetSequence) -> Result<(), NetError> {
        if self.base_sequence != last_applied {
            return Err(NetError::UnknownBaseSequence {
                base: self.base_sequence.0,
                last_applied: last_applied.0,
            });
        }
        Ok(())
    }

    /// Both lists must arrive in canonical order with unique keys.
    pub fn validate_order(
        changes: &[ReplicatedEntry],
        removals: &[Tombstone],
    ) -> Result<(), NetError> {
        let mut previous: Option<(u64, &str)> = None;
        for (index, entry) in changes.iter().enumerate() {
            let key = entry.sort_key();
            if let Some(previous_key) = previous {
                if key < previous_key {
                    return Err(NetError::UnsortedEntries { index });
                }
                if key == previous_key {
                    return Err(NetError::DuplicateEntry {
                        entity: entry.entity.0,
                        schema: entry.schema.clone(),
                        index,
                    });
                }
            }
            previous = Some(key);
        }
        let mut previous_removal: Option<(u64, u8, &str, u8)> = None;
        for (index, tombstone) in removals.iter().enumerate() {
            let key = tombstone.sort_key();
            if let Some(previous_key) = previous_removal {
                if key < previous_key {
                    return Err(NetError::UnsortedEntries { index });
                }
                if key == previous_key {
                    return Err(NetError::DuplicateEntry {
                        entity: tombstone.entity.0,
                        schema: tombstone.schema.clone().unwrap_or_default(),
                        index,
                    });
                }
            }
            previous_removal = Some(key);
        }
        Ok(())
    }
}

/// Encodes a delta: sorts both lists into canonical order, then
/// `postcard`-encodes. The result still travels inside a [`NetEnvelope`](crate::envelope::NetEnvelope)
/// (which carries the protocol version and checksum); limits are checked
/// there and at framing, so this function checks them too before handing
/// bytes anywhere.
pub fn encode_delta(
    sequence: NetSequence,
    base_sequence: NetSequence,
    sim_tick: SimTick,
    mut changes: Vec<ReplicatedEntry>,
    mut removals: Vec<Tombstone>,
    limits: &NetLimits,
) -> Result<Vec<u8>, NetError> {
    sort_entries(&mut changes);
    removals.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    let delta = Delta {
        sequence,
        base_sequence,
        sim_tick,
        changes,
        removals,
    };
    let bytes = postcard::to_allocvec(&delta)?;
    let len = u32::try_from(bytes.len()).map_err(|_| NetError::OversizeFrame {
        claimed: u32::MAX,
        max: limits.max_message_bytes,
    })?;
    limits.check_frame_len(len)?;
    Ok(bytes)
}

/// Decodes a delta and validates canonical order before returning it —
/// unsorted or duplicated entries fail here, before the caller checks the
/// base or applies anything. Base validation stays explicit via
/// [`Delta::validate_base`] (or [`Delta::validate_all`]) so the session
/// layer supplies its own `last_applied` cursor.
pub fn decode_delta(bytes: &[u8]) -> Result<Delta, NetError> {
    let (delta, remainder): (Delta, &[u8]) = postcard::take_from_bytes(bytes)?;
    if !remainder.is_empty() {
        return Err(NetError::TrailingBytes {
            trailing: remainder.len(),
        });
    }
    Delta::validate_order(&delta.changes, &delta.removals)?;
    Ok(delta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PROTOCOL_VERSION_1;

    fn entry(entity: u64, schema: &str, payload: &[u8]) -> ReplicatedEntry {
        ReplicatedEntry {
            entity: NetEntityId(entity),
            schema: schema.to_owned(),
            payload: payload.to_vec(),
        }
    }

    fn shuffled_entries() -> Vec<ReplicatedEntry> {
        vec![
            entry(9, "canary.health", b"h9"),
            entry(2, "canary.velocity", b"v2"),
            entry(2, "canary.health", b"h2"),
        ]
    }

    #[test]
    fn canonical_order_is_byte_stable_regardless_of_insertion_order() {
        let limits = NetLimits::default();
        let tick = SimTick(42);
        let forward = encode_snapshot(shuffled_entries(), tick, &limits).expect("encode");
        let mut reversed = shuffled_entries();
        reversed.reverse();
        let backward = encode_snapshot(reversed, tick, &limits).expect("encode");
        assert_eq!(forward, backward);

        let snapshot = decode_snapshot(&forward).expect("decode");
        let keys: Vec<(u64, &str)> = snapshot
            .entries
            .iter()
            .map(ReplicatedEntry::sort_key)
            .collect();
        assert_eq!(
            keys,
            vec![
                (2, "canary.health"),
                (2, "canary.velocity"),
                (9, "canary.health"),
            ]
        );
    }

    #[test]
    fn snapshot_checksum_covers_content_not_insertion_order() {
        let limits = NetLimits::default();
        let tick = SimTick(7);
        let bytes = encode_snapshot(shuffled_entries(), tick, &limits).expect("encode");
        let snapshot = decode_snapshot(&bytes).expect("decode");
        assert_eq!(snapshot.envelope.sim_tick, tick);
        // A tampered payload breaks the checksum gate, never a record.
        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        assert!(matches!(
            decode_snapshot(&tampered),
            Err(NetError::ChecksumMismatch { .. } | NetError::Codec(_))
        ));
    }

    #[test]
    fn delta_round_trip_sorts_both_lists() {
        let limits = NetLimits::default();
        let bytes = encode_delta(
            NetSequence(11),
            NetSequence(10),
            SimTick(5),
            vec![
                entry(9, "canary.health", b"h"),
                entry(2, "canary.health", b"h"),
            ],
            vec![
                Tombstone::entity_destroyed(NetEntityId(8), 4),
                Tombstone::component_removed(NetEntityId(3), "canary.health", 4),
            ],
            &limits,
        )
        .expect("encode");
        let delta = decode_delta(&bytes).expect("decode");
        assert_eq!(delta.sequence, NetSequence(11));
        assert_eq!(delta.base_sequence, NetSequence(10));
        assert_eq!(delta.sim_tick, SimTick(5));
        assert_eq!(delta.changes[0].entity, NetEntityId(2));
        assert_eq!(delta.removals[0].entity, NetEntityId(3));
        delta.validate_all(NetSequence(10)).expect("valid base");
    }

    #[test]
    fn unknown_base_rejects_the_whole_delta_never_partially() {
        let limits = NetLimits::default();
        let bytes = encode_delta(
            NetSequence(12),
            NetSequence(11),
            SimTick(6),
            vec![entry(1, "canary.health", b"h")],
            Vec::new(),
            &limits,
        )
        .expect("encode");
        let delta = decode_delta(&bytes).expect("decode");
        // Gap: we applied through 9, the delta needs 11.
        let error = delta
            .validate_all(NetSequence(9))
            .expect_err("unknown base");
        assert!(error.resync_required());
        assert!(matches!(
            error,
            NetError::UnknownBaseSequence {
                base: 11,
                last_applied: 9
            }
        ));
        // Stale: we already applied past its base — still a resync, the
        // receiver cannot prove the delta adds nothing without the baseline.
        let stale = delta.validate_all(NetSequence(14)).expect_err("stale base");
        assert!(stale.resync_required());
    }

    #[test]
    fn hand_assembled_unsorted_delta_fails_before_base_check() {
        let delta = Delta {
            sequence: NetSequence(3),
            base_sequence: NetSequence(2),
            sim_tick: SimTick(1),
            changes: vec![
                entry(9, "canary.health", b"h"),
                entry(2, "canary.health", b"h"),
            ],
            removals: Vec::new(),
        };
        let bytes = postcard::to_allocvec(&delta).expect("encode unsorted");
        assert!(matches!(
            decode_delta(&bytes),
            Err(NetError::UnsortedEntries { index: 1 })
        ));
    }

    #[test]
    fn duplicate_keys_rejected_not_merged() {
        let delta = Delta {
            sequence: NetSequence(3),
            base_sequence: NetSequence(2),
            sim_tick: SimTick(1),
            changes: vec![
                entry(2, "canary.health", b"a"),
                entry(2, "canary.health", b"b"),
            ],
            removals: Vec::new(),
        };
        let bytes = postcard::to_allocvec(&delta).expect("encode duplicated");
        let error = decode_delta(&bytes).expect_err("duplicate keys");
        assert!(matches!(error, NetError::DuplicateEntry { entity: 2, .. }));
    }

    #[test]
    fn envelope_protocol_version_rides_outside_the_delta() {
        // The delta carries no protocol version of its own: versioning is
        // the envelope's job, so a delta cannot disagree with it.
        let limits = NetLimits::default();
        let bytes = encode_delta(
            NetSequence(1),
            NetSequence(0),
            SimTick(1),
            vec![entry(1, "canary.health", b"h")],
            Vec::new(),
            &limits,
        )
        .expect("encode");
        let sealed = crate::envelope::NetEnvelope::seal(PROTOCOL_VERSION_1, NetSequence(1), bytes)
            .encode(&limits)
            .expect("seal");
        let envelope = crate::envelope::NetEnvelope::decode(&sealed).expect("decode");
        let delta = decode_delta(&envelope.payload).expect("delta");
        assert_eq!(delta.changes.len(), 1);
    }
}
