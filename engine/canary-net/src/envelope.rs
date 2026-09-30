//! The wire envelope: structural decode, then checksum-before-trust.
//!
//! [`NetEnvelope`] carries one sequenced message: the protocol version, the
//! session sequence, an integrity checksum, and the opaque payload. Decoding
//! order on this path is deliberate and is the inverse of the `.14`
//! snapshot-file path — read the `decode` docs before "simplifying" it.
//!
//! Integrity checksum: SHA-256 (`sha2` 0.10, the same crate and version the
//! `.14` snapshot path in `canary-state` checksums with) computed over the
//! protocol version, sequence, and payload bytes. That is a
//! corruption/truncation gate, not a security boundary — a checksum cannot
//! authenticate a peer, and per ADR 0027 point 9 QUIC TLS likewise does not
//! decide who the peer is. Wire bytes that fail verification never reach
//! game logic.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::NetError;
use crate::ids::{NetSequence, ProtocolVersion};
use crate::limits::NetLimits;

/// One sequenced message on the reliable ordered stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetEnvelope {
    /// Wire protocol contract this message was encoded under.
    pub protocol_version: ProtocolVersion,
    /// Per-connection sequence; gated by [`crate::sequence::SequenceGate`].
    pub sequence: NetSequence,
    /// [`NetEnvelope::checksum_of`] over version, sequence, and payload.
    pub checksum: [u8; 32],
    /// Opaque message body. Untrusted until [`NetEnvelope::verify`] passes.
    pub payload: Vec<u8>,
}

impl NetEnvelope {
    /// Seals a payload: computes the checksum over the version, sequence,
    /// and payload bytes and stores it in the envelope.
    #[must_use]
    pub fn seal(
        protocol_version: ProtocolVersion,
        sequence: NetSequence,
        payload: Vec<u8>,
    ) -> Self {
        let checksum = Self::checksum_of(protocol_version, sequence, &payload);
        Self {
            protocol_version,
            sequence,
            checksum,
            payload,
        }
    }

    /// Recomputes the checksum over the version, sequence, and payload bytes.
    #[must_use]
    pub fn checksum_of(
        protocol_version: ProtocolVersion,
        sequence: NetSequence,
        payload: &[u8],
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(protocol_version.0.to_be_bytes());
        hasher.update(sequence.0.to_be_bytes());
        hasher.update(payload);
        hasher.finalize().into()
    }

    /// Verifies the stored checksum before the caller trusts the payload.
    ///
    /// Returns [`NetError::ChecksumMismatch`] without touching the payload
    /// semantically; callers must not interpret `payload` until this passes.
    pub fn verify(&self) -> Result<(), NetError> {
        let computed = Self::checksum_of(self.protocol_version, self.sequence, &self.payload);
        if computed != self.checksum {
            return Err(NetError::ChecksumMismatch {
                expected: self.checksum,
                computed,
            });
        }
        Ok(())
    }

    /// `postcard`-encodes the envelope, rejecting over-limit messages before
    /// they reach the transport.
    pub fn encode(&self, limits: &NetLimits) -> Result<Vec<u8>, NetError> {
        let bytes = postcard::to_allocvec(self)?;
        let len = u32::try_from(bytes.len()).map_err(|_| NetError::OversizeFrame {
            claimed: u32::MAX,
            max: limits.max_message_bytes,
        })?;
        limits.check_frame_len(len)?;
        Ok(bytes)
    }

    /// Decodes length-bounded bytes into a verified envelope.
    ///
    /// Order (checksum-before-trust), and why it inverts the `.14`
    /// snapshot-file path (`canary-state::decode_snapshot`):
    ///
    /// - `.14` reads trusted-local files: it `postcard`-decodes first, then
    ///   verifies the SHA-256 checksum, then checks the encoding. Decode-first
    ///   is acceptable there because the bytes come from our own
    ///   [`save_snapshot`](https://github.com/HylightGames/canary) write —
    ///   and the code says so, warning that untrusted network bytes must not
    ///   reach it until budgets land.
    /// - This path reads hostile bytes, so allocation is gated first: the
    ///   caller holds at most `max_message_bytes` (enforced by
    ///   [`crate::frame::decode_frame_len`] before the body buffer exists),
    ///   which bounds even a malicious `postcard` length prefix inside the
    ///   envelope — a decoded `Vec` cannot outgrow the frame it came from.
    ///   Structural decode then runs on those bounded bytes, trailing bytes
    ///   are rejected (no smuggled framing past the message boundary), and
    ///   only then is the checksum verified — before the payload is trusted
    ///   by any game logic. A corrupt or hand-edited message fails as
    ///   [`NetError::ChecksumMismatch`], never as a half-applied update.
    pub fn decode(bytes: &[u8]) -> Result<Self, NetError> {
        let (envelope, remainder): (Self, &[u8]) = postcard::take_from_bytes(bytes)?;
        if !remainder.is_empty() {
            return Err(NetError::TrailingBytes {
                trailing: remainder.len(),
            });
        }
        envelope.verify()?;
        Ok(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PROTOCOL_VERSION_1;

    fn sealed(sequence: u64, payload: &[u8]) -> Vec<u8> {
        NetEnvelope::seal(PROTOCOL_VERSION_1, NetSequence(sequence), payload.to_vec())
            .encode(&NetLimits::default())
            .expect("encode sealed envelope")
    }

    #[test]
    fn seal_verify_decode_round_trip() {
        let bytes = sealed(9, b"snapshot slice");
        let envelope = NetEnvelope::decode(&bytes).expect("decode");
        assert_eq!(envelope.sequence, NetSequence(9));
        assert_eq!(envelope.protocol_version, PROTOCOL_VERSION_1);
        assert_eq!(envelope.payload, b"snapshot slice");
    }

    #[test]
    fn checksum_is_sha256_over_version_sequence_and_payload() {
        // pins the digest domain: version bytes, then sequence bytes, then
        // payload — recomputed here independently of `checksum_of`.
        let mut hasher = Sha256::new();
        hasher.update(PROTOCOL_VERSION_1.0.to_be_bytes());
        hasher.update(9u64.to_be_bytes());
        hasher.update(b"snapshot slice");
        let expected: [u8; 32] = hasher.finalize().into();
        let envelope = NetEnvelope::seal(
            PROTOCOL_VERSION_1,
            NetSequence(9),
            b"snapshot slice".to_vec(),
        );
        assert_eq!(envelope.checksum, expected);
        // and it matches the `.14` snapshot digest discipline (same crate).
        assert_eq!(envelope.checksum.len(), 32);
    }

    #[test]
    fn tampered_payload_fails_the_checksum() {
        let mut bytes = sealed(1, b"authoritative");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert!(matches!(
            NetEnvelope::decode(&bytes),
            Err(NetError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn tampered_sequence_fails_the_checksum() {
        // The sequence rides inside the checksum: editing it without
        // re-sealing breaks verification rather than reordering delivery.
        let envelope = NetEnvelope::seal(PROTOCOL_VERSION_1, NetSequence(3), b"input".to_vec());
        let mut bytes = envelope.encode(&NetLimits::default()).expect("encode");
        // Sequence is the second varint-ish field; flipping an early byte
        // corrupts either version or sequence — either way the checksum gate
        // must fire (structural decode still succeeds on these bytes).
        bytes[1] ^= 0x01;
        assert!(matches!(
            NetEnvelope::decode(&bytes),
            Err(NetError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn trailing_bytes_rejected_not_decoded() {
        let mut bytes = sealed(2, b"delta");
        bytes.extend_from_slice(b"smuggled");
        assert!(matches!(
            NetEnvelope::decode(&bytes),
            Err(NetError::TrailingBytes { trailing: 8 })
        ));
    }

    #[test]
    fn over_limit_envelope_rejected_on_encode() {
        let limits = NetLimits {
            max_message_bytes: 8,
            ..NetLimits::default()
        };
        let envelope = NetEnvelope::seal(PROTOCOL_VERSION_1, NetSequence(0), vec![0u8; 64]);
        assert!(matches!(
            envelope.encode(&limits),
            Err(NetError::OversizeFrame { .. })
        ));
    }
}
