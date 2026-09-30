//! Typed rejections for the network path (ADR 0027, points 6, 8, 10).
//!
//! Every untrusted input fails as a structured variant — never a bare string,
//! never a panic — so the session layer can map each failure to connection
//! policy without parsing messages. Two policy questions are answered here:
//! [`NetError::disconnect_reason`] (what to do with the peer) and
//! [`NetError::is_retryable`] (whether the failed operation may succeed if
//! retried) versus [`NetError::resync_required`] (whether the peer's baseline
//! is stale and only a fresh snapshot restores convergence — the peer stays
//! connected in that case).

use thiserror::Error;

/// Renders raw checksum bytes as lowercase hex for error messages.
fn hex_of(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Every way the network path can refuse untrusted bytes or a misbehaving peer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NetError {
    /// A frame length prefix (or an encoded envelope) exceeds the configured
    /// bound. Raised before allocation on the receive path.
    #[error("frame of {claimed} bytes exceeds limit of {max} bytes")]
    OversizeFrame {
        /// Claimed body length in bytes.
        claimed: u32,
        /// Configured bound in bytes.
        max: u32,
    },

    /// The stream ended before the claimed body arrived.
    #[error("frame truncated: claimed {claimed} bytes, received {received}")]
    TruncatedFrame {
        /// Body length the prefix promised.
        claimed: usize,
        /// Bytes actually delivered before the stream ended.
        received: usize,
    },

    /// Bytes remain after the envelope. A sender that appends smuggled data
    /// past the message boundary is a protocol violation, not a longer
    /// message.
    #[error("envelope has {trailing} trailing bytes after the message")]
    TrailingBytes {
        /// Number of unconsumed bytes.
        trailing: usize,
    },

    /// Structural `postcard` decode failure on length-bounded bytes.
    #[error("envelope codec: {0}")]
    Codec(#[from] postcard::Error),

    /// The envelope checksum does not cover its payload. The payload is
    /// untrusted and must not reach game logic.
    #[error(
        "checksum mismatch: expected {}, computed {}",
        hex_of(expected),
        hex_of(computed)
    )]
    ChecksumMismatch {
        /// Checksum the envelope claimed (raw SHA-256 digest bytes).
        expected: [u8; 32],
        /// Checksum recomputed locally.
        computed: [u8; 32],
    },

    /// A sequence value already accepted on this connection. Never applied
    /// twice (ADR 0027, point 10).
    #[error("duplicate sequence {sequence}")]
    DuplicateSequence {
        /// The repeated sequence value.
        sequence: u64,
    },

    /// A sequence value at or below the last accepted one, out of order.
    /// Never applied (ADR 0027, point 10).
    #[error("reordered sequence {sequence} at or below last accepted {last_accepted}")]
    ReorderedSequence {
        /// The out-of-order sequence value.
        sequence: u64,
        /// Highest sequence accepted so far.
        last_accepted: u64,
    },

    /// A delta names a base sequence the receiver has not applied — either a
    /// gap (deltas lost or reordered onto an unknown baseline) or a stale
    /// baseline the server has already discarded. Never partially applied:
    /// the session layer must resynchronize the peer from a fresh snapshot
    /// (ADR 0027, points 3–4). The connection itself stays up.
    #[error("unknown delta base {base}: last applied {last_applied}, resync required")]
    UnknownBaseSequence {
        /// Base sequence the delta claimed.
        base: u64,
        /// Newest sequence the receiver has fully applied.
        last_applied: u64,
    },

    /// The peer needs tombstones the log no longer retains (its cursor fell
    /// behind a dropped prefix), or entries past the retained tip it cannot
    /// yet have. Same policy as [`NetError::UnknownBaseSequence`]: fresh
    /// snapshot, connection stays up.
    #[error("tombstone cursor {cursor} outside retained [{oldest_retained}, {next_seq}): resync required")]
    TombstoneGap {
        /// Log sequence the client needs next.
        cursor: u64,
        /// Oldest log sequence still retained.
        oldest_retained: u64,
        /// Next log sequence to be assigned (exclusive tip).
        next_seq: u64,
    },

    /// A snapshot or delta carries entries outside canonical order, or the
    /// same `(entity, schema)` key twice. Canonical order is part of the
    /// wire contract (sorted by network entity, then schema), so an
    /// unsorted message is a protocol violation, not a longer message.
    #[error("entries outside canonical order at index {index}")]
    UnsortedEntries {
        /// Position of the first entry that breaks canonical order.
        index: usize,
    },

    /// Two entries claim the same `(entity, schema)` key in one message.
    /// The receiver cannot apply both without inventing precedence, so the
    /// whole message is rejected before anything is applied.
    #[error("duplicate entry for entity {entity} schema {schema:?} at index {index}")]
    DuplicateEntry {
        /// Network entity id both entries claim.
        entity: u64,
        /// Schema id both entries claim.
        schema: String,
        /// Position of the second (duplicate) entry.
        index: usize,
    },

    /// The peer speaks a wire protocol version this build does not support.
    /// Rejected before any state is applied (ADR 0027, point 7). The
    /// session layer answers with a typed [`crate::handshake::Reject`]
    /// and then closes the connection.
    #[error("unsupported protocol version {got}, this build speaks {supported}")]
    UnsupportedProtocol {
        /// Version the peer announced.
        got: u16,
        /// Version this build speaks.
        supported: u16,
    },

    /// The peer closed the stream mid-conversation.
    #[error("peer closed the stream")]
    PeerClosed,

    /// Outbound connection establishment failed (refused, timed out, TLS
    /// verification against the pinning policy). Transient network conditions
    /// and a not-yet-listening server both land here, so the caller may
    /// retry with backoff — see [`NetError::is_retryable`].
    #[error("connect failed: {detail}")]
    TransportConnect {
        /// Backend failure, as text (no backend types cross the boundary).
        detail: String,
    },

    /// Inbound connection acceptance failed. The listener itself is intact;
    /// accepting the next peer may succeed.
    #[error("accept failed: {detail}")]
    TransportAccept {
        /// Backend failure, as text (no backend types cross the boundary).
        detail: String,
    },

    /// A read on an established stream failed for a backend reason other
    /// than a clean peer close (which is [`NetError::PeerClosed`]). Fatal
    /// to the connection: re-establishing means a new session (WP3), not a
    /// resumed read.
    #[error("read failed: {detail}")]
    TransportRead {
        /// Backend failure, as text (no backend types cross the boundary).
        detail: String,
    },

    /// A write on an established stream failed for a backend reason other
    /// than a clean peer close. Fatal to the connection, like
    /// [`NetError::TransportRead`].
    #[error("write failed: {detail}")]
    TransportWrite {
        /// Backend failure, as text (no backend types cross the boundary).
        detail: String,
    },

    /// Underlying transport failure that fits none of the structured
    /// per-operation variants above. Fallback only: backends must prefer the
    /// structured variants so callers can distinguish retryable failures
    /// from fatal ones without parsing text.
    #[error("transport: {0}")]
    Transport(String),

    /// A per-peer queue overflowed under the `Disconnect` backpressure
    /// policy, or a single message exceeded the queue byte cap outright.
    /// Fatal to that peer only: the session layer disconnects it with
    /// [`DisconnectReason::QueueFull`] while other peers are unaffected.
    /// Under the `DropOldest` policy this variant never surfaces for
    /// ordinary overflow — the oldest queued message is dropped instead —
    /// so reaching it means the message cannot be queued at all.
    #[error("queue full: {queued_messages} messages / {queued_bytes} bytes queued")]
    QueueFull {
        /// Messages already queued for the peer.
        queued_messages: usize,
        /// Bytes already queued for the peer.
        queued_bytes: usize,
    },

    /// A client input message failed session-ingress validation
    /// (wrong player slot, duplicate sequence, stale or
    /// future-out-of-window target tick, oversize payload, or unknown
    /// action schema). The message is rejected and the validator keeps no
    /// trace of it — session state is unmutated and the connection stays
    /// alive (the input path never disconnects a peer for a bad message;
    /// only handshake, framing, and queue-full violations do).
    #[error("invalid client input: {detail}")]
    InvalidInput {
        /// What failed validation, as text (no untrusted bytes echoed).
        detail: String,
    },

    /// A delta or snapshot entry names a component schema with no
    /// registered payload codec. The receiver cannot interpret the payload
    /// without inventing semantics, so the whole message is rejected and
    /// the peer resynchronizes from a fresh snapshot on the live
    /// connection — the same policy as
    /// [`NetError::UnknownBaseSequence`], never a panic, never a partial
    /// apply.
    #[error("unknown payload schema {schema:?}: resync required")]
    UnknownSchema {
        /// Schema id no codec is registered for.
        schema: String,
    },
}

impl NetError {
    /// Maps a rejection to connection policy: what the session layer should
    /// do with the peer that triggered it.
    ///
    /// [`DisconnectReason::NeedsResync`] is not a disconnect: the connection
    /// stays up and the peer is sent a fresh snapshot. See
    /// [`NetError::resync_required`].
    #[must_use]
    pub fn disconnect_reason(&self) -> DisconnectReason {
        match self {
            Self::OversizeFrame { .. } => DisconnectReason::OversizeFrame,
            Self::ChecksumMismatch { .. } => DisconnectReason::ChecksumFailure,
            Self::DuplicateSequence { .. } | Self::ReorderedSequence { .. } => {
                DisconnectReason::ProtocolViolation
            }
            Self::UnknownBaseSequence { .. } | Self::TombstoneGap { .. } => {
                DisconnectReason::NeedsResync
            }
            Self::UnsortedEntries { .. } | Self::DuplicateEntry { .. } => {
                DisconnectReason::ProtocolViolation
            }
            Self::TrailingBytes { .. }
            | Self::Codec(_)
            | Self::TruncatedFrame { .. }
            | Self::UnsupportedProtocol { .. } => DisconnectReason::ProtocolViolation,
            Self::InvalidInput { .. } => DisconnectReason::InputRejected,
            Self::QueueFull { .. } => DisconnectReason::QueueFull,
            Self::UnknownSchema { .. } => DisconnectReason::NeedsResync,
            Self::PeerClosed => DisconnectReason::PeerClosed,
            Self::TransportConnect { .. }
            | Self::TransportAccept { .. }
            | Self::TransportRead { .. }
            | Self::TransportWrite { .. }
            | Self::Transport(_) => DisconnectReason::TransportError,
        }
    }

    /// Whether the failed operation may succeed if retried as-is.
    ///
    /// Only connection establishment and acceptance are retryable:
    /// a refused dial or a failed accept says nothing about the next
    /// attempt. Everything else either violates the protocol (retrying the
    /// same bytes fails the same way), breaks integrity (same), or ends the
    /// connection (a retry is really a reconnect — a new session under WP3
    /// policy, not a resumed operation). The [`NetError::Transport`]
    /// fallback is conservatively *not* retryable: an unclassified backend
    /// failure must not spin a retry loop.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::TransportConnect { .. } | Self::TransportAccept { .. }
        )
    }

    /// Whether the peer's baseline is stale and only a fresh snapshot
    /// restores convergence. When true, the session layer resynchronizes
    /// the peer instead of disconnecting it — see
    /// [`DisconnectReason::NeedsResync`].
    #[must_use]
    pub fn resync_required(&self) -> bool {
        matches!(
            self,
            Self::UnknownBaseSequence { .. }
                | Self::TombstoneGap { .. }
                | Self::UnknownSchema { .. }
        )
    }
}

/// Why a peer was (or should be) disconnected — or, for
/// [`DisconnectReason::NeedsResync`], resynchronized on the live
/// connection. Every overflow or violation applies backpressure or
/// disconnects that peer with one of these; it never grows memory without
/// bound (ADR 0027, point 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisconnectReason {
    /// A frame exceeded the size bound.
    OversizeFrame,
    /// Per-peer queued messages or bytes overflowed (WP3 session queues).
    QueueFull,
    /// Malformed, trailing, replayed, reordered, or version-incompatible
    /// data.
    ProtocolViolation,
    /// A client input message failed ingress validation. The message is
    /// rejected; the connection stays up and no resync is triggered.
    InputRejected,
    /// An envelope failed checksum verification.
    ChecksumFailure,
    /// The peer's baseline is stale (unknown delta base, tombstone gap):
    /// send a fresh snapshot on the live connection instead of
    /// disconnecting.
    NeedsResync,
    /// The peer was idle past the admission policy (WP3).
    IdleTimeout,
    /// The peer closed the connection or stream.
    PeerClosed,
    /// Underlying transport failure.
    TransportError,
    /// Orderly local shutdown.
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejections_map_to_the_documented_policy() {
        assert_eq!(
            NetError::OversizeFrame { claimed: 2, max: 1 }.disconnect_reason(),
            DisconnectReason::OversizeFrame
        );
        assert_eq!(
            NetError::ChecksumMismatch {
                expected: [1u8; 32],
                computed: [2u8; 32]
            }
            .disconnect_reason(),
            DisconnectReason::ChecksumFailure
        );
        assert_eq!(
            NetError::DuplicateSequence { sequence: 4 }.disconnect_reason(),
            DisconnectReason::ProtocolViolation
        );
        assert_eq!(
            NetError::ReorderedSequence {
                sequence: 2,
                last_accepted: 5
            }
            .disconnect_reason(),
            DisconnectReason::ProtocolViolation
        );
        assert_eq!(
            NetError::TrailingBytes { trailing: 3 }.disconnect_reason(),
            DisconnectReason::ProtocolViolation
        );
        assert_eq!(
            NetError::PeerClosed.disconnect_reason(),
            DisconnectReason::PeerClosed
        );
        assert_eq!(
            NetError::Transport("boom".to_string()).disconnect_reason(),
            DisconnectReason::TransportError
        );
    }

    #[test]
    fn only_dial_and_accept_are_retryable() {
        assert!(NetError::TransportConnect {
            detail: "refused".to_string()
        }
        .is_retryable());
        assert!(NetError::TransportAccept {
            detail: "busy".to_string()
        }
        .is_retryable());
        for error in [
            NetError::TransportRead {
                detail: "reset".to_string(),
            },
            NetError::TransportWrite {
                detail: "reset".to_string(),
            },
            NetError::Transport("unclassified".to_string()),
            NetError::PeerClosed,
            NetError::ChecksumMismatch {
                expected: [0u8; 32],
                computed: [1u8; 32],
            },
            NetError::OversizeFrame { claimed: 9, max: 1 },
        ] {
            assert!(
                !error.is_retryable(),
                "{error:?} must be fatal, not retried"
            );
        }
    }

    #[test]
    fn unknown_base_and_tombstone_gap_request_resync_not_disconnect() {
        let base = NetError::UnknownBaseSequence {
            base: 12,
            last_applied: 9,
        };
        assert!(base.resync_required());
        assert_eq!(base.disconnect_reason(), DisconnectReason::NeedsResync);
        let gap = NetError::TombstoneGap {
            cursor: 3,
            oldest_retained: 40,
            next_seq: 50,
        };
        assert!(gap.resync_required());
        assert_eq!(gap.disconnect_reason(), DisconnectReason::NeedsResync);
        assert!(!NetError::PeerClosed.resync_required());
        assert!(!NetError::UnsortedEntries { index: 1 }.resync_required());
    }

    #[test]
    fn canonical_violations_are_protocol_violations() {
        assert_eq!(
            NetError::UnsortedEntries { index: 2 }.disconnect_reason(),
            DisconnectReason::ProtocolViolation
        );
        assert_eq!(
            NetError::DuplicateEntry {
                entity: 7,
                schema: "canary.health".to_string(),
                index: 3,
            }
            .disconnect_reason(),
            DisconnectReason::ProtocolViolation
        );
    }

    #[test]
    fn checksum_mismatch_reports_hex_digests() {
        let error = NetError::ChecksumMismatch {
            expected: [0xabu8; 32],
            computed: [0u8; 32],
        };
        let text = error.to_string();
        assert!(text.contains(&"ab".repeat(32)), "{text}");
        assert!(text.contains(&"00".repeat(32)), "{text}");
    }

    #[test]
    fn session_layer_policy_for_queue_input_and_schema_errors() {
        // Backpressure overflow disconnects that peer only.
        let full = NetError::QueueFull {
            queued_messages: 64,
            queued_bytes: 1024,
        };
        assert_eq!(full.disconnect_reason(), DisconnectReason::QueueFull);
        assert!(!full.is_retryable());
        assert!(!full.resync_required());
        // Rejected input keeps the connection alive without a resync.
        let bad_input = NetError::InvalidInput {
            detail: "wrong player slot".to_string(),
        };
        assert_eq!(
            bad_input.disconnect_reason(),
            DisconnectReason::InputRejected
        );
        assert!(!bad_input.is_retryable());
        assert!(!bad_input.resync_required());
        // Unknown payload schema resynchronizes on the live connection.
        let unknown = NetError::UnknownSchema {
            schema: "canary.unregistered".to_string(),
        };
        assert!(unknown.resync_required());
        assert_eq!(unknown.disconnect_reason(), DisconnectReason::NeedsResync);
        assert!(!unknown.is_retryable());
    }
}
