//! Client input ingress validation (ADR 0027, point 6).
//!
//! Clients submit logical input, never state writes: every [`ClientInput`]
//! names the connection's assigned player slot, the target simulation tick,
//! and a versioned action payload. [`InputValidator`] verifies ownership,
//! schema/action compatibility, payload bounds, and the accepted input
//! window before anything is queued — validate-all-before-apply per client.
//! A malformed message fails as [`NetError::InvalidInput`]: the validator
//! keeps no trace of it (no state mutation), and the connection stays alive.
//! The input path never disconnects a peer for a bad message; only the
//! handshake, framing, and queue-full paths do.
//!
//! Accepted input becomes a [`ValidatedInput`] the server simulation may
//! queue. Server simulation alone produces authoritative replicated state
//! (ADR 0027, point 6); this module only gates what reaches it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::NetError;
use crate::limits::NetLimits;

/// One logical input message from a client: who it is for, when it is
/// for, and what versioned action it carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInput {
    /// Player slot the input claims. Must equal the slot assigned at
    /// handshake ([`crate::handshake::Welcome::assigned_slot`]).
    pub player_slot: u64,
    /// Simulation tick the input targets.
    pub target_tick: u64,
    /// Stable action schema id (e.g. `"canary.input/move@1"`).
    pub action_schema: String,
    /// Version of the action payload encoding.
    pub action_version: u32,
    /// Versioned action payload bytes. Opaque here; interpreted by the
    /// simulation after validation.
    pub payload: Vec<u8>,
    /// Per-client input sequence. Strictly increasing per connection:
    /// duplicates are rejected, never queued twice.
    pub input_seq: u64,
}

/// Input that passed every ingress check and may be queued for simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedInput {
    /// Player slot the input was accepted for.
    pub player_slot: u64,
    /// Simulation tick the input targets.
    pub target_tick: u64,
    /// Action schema id, confirmed against the allow-list.
    pub action_schema: String,
    /// Action payload version, confirmed against the schema's accepted
    /// version. Forwarded so the simulation interprets the payload bytes
    /// under the exact encoding the validator approved.
    pub action_version: u32,
    /// Action payload bytes, within bounds.
    pub payload: Vec<u8>,
    /// Input sequence accepted (advances the validator cursor).
    pub input_seq: u64,
}

/// Per-client ingress validator. One instance lives in the client's
/// session record; it is the only mutable state the input path touches.
#[derive(Debug)]
pub struct InputValidator {
    /// Slot assigned to this connection at handshake.
    assigned_slot: u64,
    /// Action schemas this client may submit, each mapped to the one
    /// `action_version` accepted for it (`None` for unversioned schema ids,
    /// which constrain the schema only). Derived from the version suffix of
    /// each allow-list entry (see [`InputValidator::new`]).
    allowed_actions: BTreeMap<String, Option<u32>>,
    /// Maximum accepted action payload in bytes.
    max_payload_bytes: usize,
    /// How many ticks ahead of the server tick an input may target.
    max_future_ticks: u64,
    /// How many ticks behind the server tick an input may target.
    max_stale_ticks: u64,
    /// Highest input sequence accepted so far. `None` before the first.
    last_input_seq: Option<u64>,
}

/// Reads the accepted `action_version` out of an allow-list entry's
/// trailing `@N` version suffix (e.g. `"canary.input/move@1"` accepts
/// version 1). Returns `None` for entries with no numeric suffix: an
/// unversioned schema id constrains the schema only, not the version.
fn expected_action_version(action_schema: &str) -> Option<u32> {
    action_schema
        .rsplit_once('@')
        .and_then(|(_, suffix)| suffix.parse::<u32>().ok())
}

impl InputValidator {
    /// Builds a validator for the connection holding `assigned_slot`.
    ///
    /// Each entry of `allowed_actions` names one submittable action schema
    /// id; a trailing `@N` suffix additionally pins the single accepted
    /// [`ClientInput::action_version`] for that schema (exact match —
    /// version 2 of a `@1` action is rejected, not coerced). Entries with
    /// no numeric suffix constrain the schema only.
    #[must_use]
    pub fn new(
        assigned_slot: u64,
        allowed_actions: &[&str],
        max_payload_bytes: usize,
        max_future_ticks: u64,
        max_stale_ticks: u64,
    ) -> Self {
        Self {
            assigned_slot,
            allowed_actions: allowed_actions
                .iter()
                .map(|action| (action.to_string(), expected_action_version(action)))
                .collect(),
            max_payload_bytes,
            max_future_ticks,
            max_stale_ticks,
            last_input_seq: None,
        }
    }

    /// Highest input sequence accepted so far, or `None` before the first.
    #[must_use]
    pub fn last_input_seq(&self) -> Option<u64> {
        self.last_input_seq
    }

    /// Validates `input` against every ingress rule for `server_tick`.
    ///
    /// Checks ownership, sequence freshness, payload bounds, action-schema
    /// compatibility (the schema must be allow-listed *and* the payload's
    /// `action_version` must equal the version pinned by the schema's `@N`
    /// suffix), and the accepted input window — in that order — and only
    /// advances the sequence cursor when all of them pass. A failure
    /// leaves the validator exactly as it was: a rejected message can
    /// neither poison later messages nor disconnect the peer.
    pub fn validate(
        &mut self,
        input: &ClientInput,
        server_tick: u64,
    ) -> Result<ValidatedInput, NetError> {
        if input.player_slot != self.assigned_slot {
            return Err(NetError::InvalidInput {
                detail: "input names a player slot this connection was not assigned".to_string(),
            });
        }
        if let Some(last) = self.last_input_seq {
            if input.input_seq <= last {
                return Err(NetError::InvalidInput {
                    detail: "duplicate or stale input sequence".to_string(),
                });
            }
        }
        if input.payload.len() > self.max_payload_bytes {
            return Err(NetError::InvalidInput {
                detail: "input payload exceeds the per-message bound".to_string(),
            });
        }
        let expected_version = self
            .allowed_actions
            .get(&input.action_schema)
            .copied()
            .ok_or_else(|| NetError::InvalidInput {
                detail: "unknown action schema for this session".to_string(),
            })?;
        if let Some(expected) = expected_version {
            if input.action_version != expected {
                return Err(NetError::InvalidInput {
                    detail: "unsupported action version for this schema".to_string(),
                });
            }
        }
        if input.target_tick > server_tick.saturating_add(self.max_future_ticks) {
            return Err(NetError::InvalidInput {
                detail: "input targets a tick past the accepted window".to_string(),
            });
        }
        if server_tick.saturating_sub(input.target_tick) > self.max_stale_ticks {
            return Err(NetError::InvalidInput {
                detail: "input targets a tick before the accepted window".to_string(),
            });
        }
        self.last_input_seq = Some(input.input_seq);
        Ok(ValidatedInput {
            player_slot: input.player_slot,
            target_tick: input.target_tick,
            action_schema: input.action_schema.clone(),
            action_version: input.action_version,
            payload: input.payload.clone(),
            input_seq: input.input_seq,
        })
    }
}

/// Encodes a [`ClientInput`] for the wire, gated by `limits` before it
/// reaches the transport.
pub fn encode_input(input: &ClientInput, limits: &NetLimits) -> Result<Vec<u8>, NetError> {
    let bytes = postcard::to_allocvec(input)?;
    let len = u32::try_from(bytes.len()).map_err(|_| NetError::OversizeFrame {
        claimed: u32::MAX,
        max: limits.max_message_bytes,
    })?;
    limits.check_frame_len(len)?;
    Ok(bytes)
}

/// Decodes a [`ClientInput`], rejecting trailing bytes. Decoding is not
/// validation: the result is still untrusted until an [`InputValidator`]
/// accepts it.
pub fn decode_input(bytes: &[u8]) -> Result<ClientInput, NetError> {
    let (input, remainder): (ClientInput, &[u8]) = postcard::take_from_bytes(bytes)?;
    if !remainder.is_empty() {
        return Err(NetError::TrailingBytes {
            trailing: remainder.len(),
        });
    }
    Ok(input)
}

/// Server acknowledgement that a validated input was queued for the
/// simulation: which input sequence was accepted and the server tick it
/// was applied at. The ack is the round-trip proof for an input — the
/// client advances its input cursor only when the ack's `input_seq`
/// matches what it sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputAck {
    /// Input sequence accepted (echoes [`ClientInput::input_seq`]).
    pub input_seq: u64,
    /// Server tick the input was applied at.
    pub applied_tick: u64,
}

/// Encodes an [`InputAck`] for the wire, gated by `limits`.
pub fn encode_ack(ack: &InputAck, limits: &NetLimits) -> Result<Vec<u8>, NetError> {
    let bytes = postcard::to_allocvec(ack)?;
    let len = u32::try_from(bytes.len()).map_err(|_| NetError::OversizeFrame {
        claimed: u32::MAX,
        max: limits.max_message_bytes,
    })?;
    limits.check_frame_len(len)?;
    Ok(bytes)
}

/// Decodes an [`InputAck`], rejecting trailing bytes.
pub fn decode_ack(bytes: &[u8]) -> Result<InputAck, NetError> {
    let (ack, remainder): (InputAck, &[u8]) = postcard::take_from_bytes(bytes)?;
    if !remainder.is_empty() {
        return Err(NetError::TrailingBytes {
            trailing: remainder.len(),
        });
    }
    Ok(ack)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validator() -> InputValidator {
        InputValidator::new(7, &["canary.input/move@1"], 64, 4, 8)
    }

    fn input(slot: u64, seq: u64, target: u64) -> ClientInput {
        ClientInput {
            player_slot: slot,
            target_tick: target,
            action_schema: "canary.input/move@1".to_string(),
            action_version: 1,
            payload: vec![1, 2, 3],
            input_seq: seq,
        }
    }

    #[test]
    fn valid_input_passes_and_advances_the_cursor() {
        let mut validator = validator();
        let accepted = validator.validate(&input(7, 1, 100), 100).expect("valid");
        assert_eq!(accepted.player_slot, 7);
        assert_eq!(accepted.input_seq, 1);
        // The accepted payload version rides along so the simulation
        // interprets the bytes under the exact encoding approved here.
        assert_eq!(accepted.action_version, 1);
        assert_eq!(accepted.action_schema, "canary.input/move@1");
        assert_eq!(validator.last_input_seq(), Some(1));
    }

    #[test]
    fn action_version_must_match_the_schema_suffix_exactly() {
        let mut validator = validator();
        // Accepted: the schema's `@1` pins version 1.
        validator
            .validate(&input(7, 1, 100), 100)
            .expect("version 1");
        // Rejected: one below and one above the pinned version, plus the
        // u32 boundary values — exact match, no coercion, no range.
        for (seq, version) in [(2, 0), (3, 2), (4, u32::MAX)] {
            let mut bad = input(7, seq, 100);
            bad.action_version = version;
            let error = validator
                .validate(&bad, 100)
                .expect_err("wrong action version must fail");
            assert!(
                matches!(error, NetError::InvalidInput { .. }),
                "unexpected error: {error:?}"
            );
            assert_eq!(
                error.disconnect_reason(),
                crate::error::DisconnectReason::InputRejected
            );
            assert!(!error.resync_required());
        }
        // Every version rejection left no trace: the next valid sequence
        // still lands exactly after the seed.
        assert_eq!(validator.last_input_seq(), Some(1));
        let accepted = validator
            .validate(&input(7, 2, 100), 100)
            .expect("still usable");
        assert_eq!(accepted.action_version, 1);
    }

    #[test]
    fn unversioned_schema_ids_constrain_the_schema_only() {
        let mut validator = InputValidator::new(7, &["canary.input/jump"], 64, 4, 8);
        for (seq, version) in [(1, 0), (2, 7), (3, u32::MAX)] {
            let mut accepted_input = input(7, seq, 100);
            accepted_input.action_schema = "canary.input/jump".to_string();
            accepted_input.action_version = version;
            let accepted = validator
                .validate(&accepted_input, 100)
                .expect("unversioned schema accepts any version");
            assert_eq!(accepted.action_version, version);
        }
    }

    #[test]
    fn each_rejection_leaves_no_trace_and_keeps_the_session_usable() {
        let mut validator = validator();
        // Seed one accepted input so later sequence checks have a cursor.
        validator.validate(&input(7, 5, 100), 100).expect("seed");
        let cases = [
            input(3, 6, 100), // wrong player slot
            input(7, 5, 100), // duplicate sequence
            input(7, 4, 100), // stale sequence
            {
                let mut oversized = input(7, 6, 100);
                oversized.payload = vec![0u8; 65];
                oversized
            },
            {
                let mut unknown = input(7, 6, 100);
                unknown.action_schema = "canary.input/noclip@9".to_string();
                unknown
            },
            input(7, 6, 105), // future out of window (server 100 + 4)
            input(7, 6, 91),  // stale tick (server 100 - 8 - 1)
        ];
        for bad in &cases {
            let error = validator.validate(bad, 100).expect_err("must reject");
            assert!(
                matches!(error, NetError::InvalidInput { .. }),
                "unexpected error: {error:?}"
            );
            // The rejection is never a disconnect and never a resync: the
            // connection stays alive on the current baseline.
            assert_eq!(
                error.disconnect_reason(),
                crate::error::DisconnectReason::InputRejected
            );
            assert!(!error.resync_required());
        }
        // Cursor unmoved by every rejection: the next valid sequence still
        // lands exactly after the seed.
        assert_eq!(validator.last_input_seq(), Some(5));
        let accepted = validator
            .validate(&input(7, 6, 100), 100)
            .expect("still usable");
        assert_eq!(accepted.input_seq, 6);
    }

    #[test]
    fn window_edges_are_inclusive() {
        let mut validator = validator();
        validator
            .validate(&input(7, 1, 104), 100)
            .expect("future edge");
        validator
            .validate(&input(7, 2, 92), 100)
            .expect("stale edge");
    }

    #[test]
    fn input_codec_round_trip_rejects_trailing_bytes() {
        let limits = NetLimits::default();
        let bytes = encode_input(&input(7, 1, 100), &limits).expect("encode");
        assert_eq!(decode_input(&bytes).expect("decode"), input(7, 1, 100));
        let mut smuggled = bytes;
        smuggled.push(0);
        assert!(matches!(
            decode_input(&smuggled),
            Err(NetError::TrailingBytes { .. })
        ));

        let ack = InputAck {
            input_seq: 6,
            applied_tick: 101,
        };
        let bytes = encode_ack(&ack, &limits).expect("encode ack");
        assert_eq!(decode_ack(&bytes).expect("decode ack"), ack);
    }
}
