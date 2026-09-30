//! Bounded per-client queues and backpressure policy (ADR 0027, point 8).
//!
//! Every message a peer queues — outbound deltas waiting on the socket,
//! inbound input waiting on the simulation — passes through a
//! [`BoundedQueue`]. Both the message count and the byte count are capped
//! from [`NetLimits`](crate::limits::NetLimits); a peer that overflows its
//! queue is handled per the session's [`QueuePolicy`], never by growing
//! memory without bound:
//!
//! - [`QueuePolicy::Disconnect`]: the overflow fails as
//!   [`NetError::QueueFull`] and the session layer disconnects that peer
//!   with [`DisconnectReason::QueueFull`](crate::error::DisconnectReason::QueueFull).
//!   Other peers are unaffected. Use for ingress, where accepting more
//!   would let a hostile client spend server memory.
//! - [`QueuePolicy::DropOldest`]: the oldest queued message is dropped
//!   (counted in [`BoundedQueue::dropped`]) to make room, so a slow
//!   consumer loses stale state — which the next delta supersedes anyway —
//!   instead of disconnecting. Use for egress state, where newer deltas
//!   obsolete older ones.
//!
//! A single message larger than the byte cap cannot be queued under either
//! policy: dropping the whole queue to fit one message would amplify a
//! hostile peer into a full resync storm, so it fails as
//! [`NetError::QueueFull`] and the peer is disconnected.

use std::collections::VecDeque;

use crate::error::NetError;

/// What a session does when a peer's queue overflows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueuePolicy {
    /// Fail the push as [`NetError::QueueFull`]; the session layer
    /// disconnects that peer. Default: the choice that cannot lose
    /// authoritative state silently.
    #[default]
    Disconnect,
    /// Drop the oldest queued message(s) to make room, counting them in
    /// [`BoundedQueue::dropped`]. Only for queues whose newer entries
    /// obsolete older ones (outbound state), never for ingress.
    DropOldest,
}

/// How a push resolved under [`QueuePolicy::DropOldest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// Queued without dropping anything.
    Accepted,
    /// Queued after dropping this many oldest messages.
    DroppedOldest {
        /// How many queued messages were discarded to make room.
        dropped: usize,
    },
}

/// A message-count- and byte-count-bounded FIFO for one peer.
#[derive(Debug)]
pub struct BoundedQueue<T> {
    /// Queued messages with their byte sizes, oldest first.
    items: VecDeque<(T, usize)>,
    /// Maximum messages retained.
    max_messages: usize,
    /// Maximum bytes retained.
    max_bytes: usize,
    /// Bytes currently retained.
    queued_bytes: usize,
    /// Messages dropped under [`QueuePolicy::DropOldest`] over the queue's
    /// lifetime. Monotonic: dequeuing never decrements it.
    dropped: u64,
}

impl<T> BoundedQueue<T> {
    /// An empty queue holding at most `max_messages` messages and
    /// `max_bytes` bytes. Zero caps queue nothing: every push fails.
    #[must_use]
    pub fn new(max_messages: usize, max_bytes: usize) -> Self {
        Self {
            items: VecDeque::new(),
            max_messages,
            max_bytes,
            queued_bytes: 0,
            dropped: 0,
        }
    }

    /// Builds a queue from the shared [`NetLimits`](crate::limits::NetLimits)
    /// knobs. The `u32` limits are widened to `usize` without loss on any
    /// target this engine supports; the conversion stays explicit.
    #[must_use]
    pub fn from_limits(limits: &crate::limits::NetLimits) -> Self {
        Self::new(
            usize::try_from(limits.max_queued_messages).unwrap_or(usize::MAX),
            usize::try_from(limits.max_queued_bytes).unwrap_or(usize::MAX),
        )
    }

    /// Queues `item` of `bytes` under `policy`.
    ///
    /// Under [`QueuePolicy::Disconnect`], a full queue (or an item larger
    /// than the byte cap) fails as [`NetError::QueueFull`] and the queue is
    /// unmodified. Under [`QueuePolicy::DropOldest`], oldest messages are
    /// evicted until the item fits; an item larger than the whole byte cap
    /// still fails — the queue is emptied first, then the error reports the
    /// empty queue, so the caller can see nothing was salvageable.
    pub fn push(
        &mut self,
        item: T,
        bytes: usize,
        policy: QueuePolicy,
    ) -> Result<PushOutcome, NetError> {
        if bytes > self.max_bytes {
            return Err(self.full());
        }
        match policy {
            QueuePolicy::Disconnect => {
                if self.items.len() >= self.max_messages
                    || self.queued_bytes.saturating_add(bytes) > self.max_bytes
                {
                    return Err(self.full());
                }
                self.items.push_back((item, bytes));
                self.queued_bytes = self.queued_bytes.saturating_add(bytes);
                Ok(PushOutcome::Accepted)
            }
            QueuePolicy::DropOldest => {
                let mut dropped = 0usize;
                while self.items.len() >= self.max_messages
                    || self.queued_bytes.saturating_add(bytes) > self.max_bytes
                {
                    let Some((_, evicted)) = self.items.pop_front() else {
                        break;
                    };
                    self.queued_bytes = self.queued_bytes.saturating_sub(evicted);
                    dropped += 1;
                }
                // The cap check above guarantees room unless the caps are
                // zero, in which case nothing can ever be queued.
                if self.items.len() >= self.max_messages
                    || self.queued_bytes.saturating_add(bytes) > self.max_bytes
                {
                    return Err(self.full());
                }
                self.items.push_back((item, bytes));
                self.queued_bytes = self.queued_bytes.saturating_add(bytes);
                self.dropped = self.dropped.saturating_add(dropped as u64);
                if dropped == 0 {
                    Ok(PushOutcome::Accepted)
                } else {
                    Ok(PushOutcome::DroppedOldest { dropped })
                }
            }
        }
    }

    /// Removes and returns the oldest queued message, or `None` when empty.
    pub fn pop(&mut self) -> Option<T> {
        let (item, bytes) = self.items.pop_front()?;
        self.queued_bytes = self.queued_bytes.saturating_sub(bytes);
        Some(item)
    }

    /// How many messages are queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the queue holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Bytes currently retained. Always at or under the byte cap.
    #[must_use]
    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    /// Messages dropped under [`QueuePolicy::DropOldest`] over the queue's
    /// lifetime. Monotonic.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Reports the current fullness as [`NetError::QueueFull`].
    fn full(&self) -> NetError {
        NetError::QueueFull {
            queued_messages: self.items.len(),
            queued_bytes: self.queued_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_policy_fails_overflow_and_keeps_the_queued_prefix() {
        let mut queue = BoundedQueue::new(2, 1024);
        assert!(matches!(
            queue.push(vec![1u8], 1, QueuePolicy::Disconnect),
            Ok(PushOutcome::Accepted)
        ));
        assert!(matches!(
            queue.push(vec![2u8], 1, QueuePolicy::Disconnect),
            Ok(PushOutcome::Accepted)
        ));
        let error = queue
            .push(vec![3u8], 1, QueuePolicy::Disconnect)
            .expect_err("third message overflows a cap of two");
        assert!(matches!(error, NetError::QueueFull { .. }));
        // The queued prefix is untouched: overflow drops the newcomer, not
        // history.
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop(), Some(vec![1u8]));
        assert_eq!(queue.pop(), Some(vec![2u8]));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn byte_cap_binds_before_message_count() {
        let mut queue = BoundedQueue::new(64, 4);
        queue
            .push(vec![0u8; 3], 3, QueuePolicy::Disconnect)
            .expect("fits");
        assert!(matches!(
            queue.push(vec![0u8; 2], 2, QueuePolicy::Disconnect),
            Err(NetError::QueueFull { .. })
        ));
        assert_eq!(queue.queued_bytes(), 3);
    }

    #[test]
    fn drop_oldest_evicts_stale_state_and_counts_it() {
        let mut queue: BoundedQueue<Vec<u8>> = BoundedQueue::new(2, 1024);
        queue
            .push(vec![1u8], 1, QueuePolicy::DropOldest)
            .expect("first");
        queue
            .push(vec![2u8], 1, QueuePolicy::DropOldest)
            .expect("second");
        assert!(matches!(
            queue.push(vec![3u8], 1, QueuePolicy::DropOldest),
            Ok(PushOutcome::DroppedOldest { dropped: 1 })
        ));
        assert_eq!(queue.dropped(), 1);
        assert_eq!(queue.len(), 2);
        // The oldest entry is gone; the two newest survive.
        assert_eq!(queue.pop(), Some(vec![2u8]));
        assert_eq!(queue.pop(), Some(vec![3u8]));
    }

    #[test]
    fn flooding_ingress_is_memory_bounded_and_disconnects() {
        // A hostile peer floods ingress under the Disconnect policy: memory
        // stays under both caps and the policy surfaces exactly once per
        // excess message.
        let mut queue: BoundedQueue<Vec<u8>> =
            BoundedQueue::from_limits(&crate::limits::NetLimits {
                max_queued_messages: 8,
                ..crate::limits::NetLimits::default()
            });
        let mut rejected = 0usize;
        for index in 0..1024u64 {
            let body = index.to_be_bytes().to_vec();
            match queue.push(body, 8, QueuePolicy::Disconnect) {
                Ok(_) => {}
                Err(NetError::QueueFull { .. }) => rejected += 1,
                Err(other) => panic!("unexpected queue error: {other:?}"),
            }
            assert!(queue.len() <= 8, "message count escaped the cap");
            assert!(queue.queued_bytes() <= 8 * 8, "byte count escaped the cap");
        }
        assert_eq!(queue.len(), 8);
        assert_eq!(rejected, 1024 - 8);
    }

    #[test]
    fn oversize_single_message_never_clears_the_queue_to_fit() {
        let mut queue: BoundedQueue<Vec<u8>> = BoundedQueue::new(4, 8);
        queue
            .push(vec![1u8], 1, QueuePolicy::DropOldest)
            .expect("seed");
        // One 9-byte message against an 8-byte cap: rejected even under
        // DropOldest, and the queued message survives.
        assert!(matches!(
            queue.push(vec![0u8; 9], 9, QueuePolicy::DropOldest),
            Err(NetError::QueueFull { .. })
        ));
        assert_eq!(queue.len(), 1);
    }
}
