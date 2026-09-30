//! Bounds on untrusted network work and memory (ADR 0027, point 8).
//!
//! Every limit is checked before the resource it guards is spent: the frame
//! length before allocation, the encoded envelope before send, the queue
//! counters before enqueue. A peer that exceeds a bound is refused or
//! disconnected with a typed [`crate::error::DisconnectReason`]; queues never
//! grow without bound and network work never blocks the simulation loop.

use crate::error::NetError;

/// Default cap on one decoded frame body: 1 MiB.
///
/// Large enough for a bounded initial snapshot slice; small enough that a
/// hostile length prefix cannot force a large allocation. Tune per profile
/// once snapshot sizes are measured.
pub const DEFAULT_MAX_MESSAGE_BYTES: u32 = 1024 * 1024;

/// Default cap on queued outbound messages per peer.
///
/// Enforced by [`BoundedQueue`](crate::queue::BoundedQueue): session
/// records build their queue caps from these knobs via
/// [`ClientSession::caps_from_limits`](crate::session::ClientSession::caps_from_limits).
pub const DEFAULT_MAX_QUEUED_MESSAGES: u32 = 64;

/// Default cap on queued outbound bytes per peer.
///
/// Enforced by [`BoundedQueue`](crate::queue::BoundedQueue), same path as
/// [`DEFAULT_MAX_QUEUED_MESSAGES`].
pub const DEFAULT_MAX_QUEUED_BYTES: u32 = 8 * 1024 * 1024;

/// Bounds enforced on the network path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetLimits {
    /// Maximum accepted frame body length in bytes, checked against the
    /// 4-byte length prefix before any allocation.
    pub max_message_bytes: u32,
    /// Maximum queued messages per peer, enforced by the session queues.
    pub max_queued_messages: u32,
    /// Maximum queued bytes per peer, enforced by the session queues.
    pub max_queued_bytes: u32,
}

impl Default for NetLimits {
    /// Conservative defaults; see the `DEFAULT_*` constants.
    fn default() -> Self {
        Self {
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_queued_messages: DEFAULT_MAX_QUEUED_MESSAGES,
            max_queued_bytes: DEFAULT_MAX_QUEUED_BYTES,
        }
    }
}

impl NetLimits {
    /// Gates a claimed frame length before the caller allocates for it.
    ///
    /// `claimed` arrives as `u32` straight from the 4-byte big-endian length
    /// prefix, so the `u32 -> usize` widening cannot fail on any target this
    /// engine supports; the `try_from` keeps that assumption explicit rather
    /// than an `as` cast.
    pub fn check_frame_len(&self, claimed: u32) -> Result<usize, NetError> {
        if claimed > self.max_message_bytes {
            return Err(NetError::OversizeFrame {
                claimed,
                max: self.max_message_bytes,
            });
        }
        usize::try_from(claimed).map_err(|_| NetError::OversizeFrame {
            claimed,
            max: self.max_message_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_gate_at_one_mebibyte() {
        let limits = NetLimits::default();
        assert_eq!(
            limits
                .check_frame_len(DEFAULT_MAX_MESSAGE_BYTES)
                .expect("at-limit"),
            DEFAULT_MAX_MESSAGE_BYTES as usize
        );
        assert!(matches!(
            limits.check_frame_len(DEFAULT_MAX_MESSAGE_BYTES + 1),
            Err(NetError::OversizeFrame { .. })
        ));
    }

    #[test]
    fn zero_length_frame_is_allowed() {
        let limits = NetLimits::default();
        assert_eq!(limits.check_frame_len(0).expect("empty body"), 0);
    }
}
