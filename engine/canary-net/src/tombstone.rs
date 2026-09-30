//! Durable removal/destruction log (ADR 0027, point 4; R-33).
//!
//! `World::query_changed_since` reports component *mutation* but not
//! component *removal* or entity *destruction*: a removed row is simply gone,
//! with nothing left to query (ADR 0014, "Known gaps"). [`TombstoneLog`] is
//! the complementary mechanism — owned by `canary-net`, consulted alongside
//! `query_changed_since`, never folded into it. Callers record a
//! [`Tombstone`] at the moment they remove a component or despawn an entity
//! (carrying the current scheduler-tick value so the server can correlate
//! tombstones with per-client last-acknowledged ticks); each delta ships the
//! tombstones the client's cursor has not yet acknowledged.
//!
//! Retention is bounded and ack-gated: [`TombstoneLog::reclaim_through`]
//! drops the prefix every connected client has acknowledged, and the cap
//! drops older entries regardless. A client whose cursor falls behind a
//! dropped prefix cannot be caught up incrementally — it resynchronizes
//! from a fresh snapshot under the gap→resync rule, never from a partial
//! apply ([`NetError::TombstoneGap`](crate::error::NetError::TombstoneGap)).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::error::NetError;
use crate::ids::NetEntityId;

/// Default cap on retained tombstones per log. Large enough that a healthy
/// client never outruns retention between acknowledgements; small enough
/// that a dead client cannot grow the server without bound. Tune with
/// measured snapshot sizes (WP3).
pub const DEFAULT_TOMBSTONE_CAP: usize = 1024;

/// Which removal a tombstone records. Component removals and entity
/// destructions are distinct operations: a removed component leaves the
/// entity (and its other components) intact, while a destroyed entity ends
/// every replicated component it carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TombstoneKind {
    /// One replicated component was removed; the entity survives.
    ComponentRemoved,
    /// The entity was despawned; all of its replicated state ends.
    EntityDestroyed,
}

impl TombstoneKind {
    /// Canonical rank for ordering tombstones within a delta: removals sort
    /// before destructions so a client applies the narrower op first.
    fn rank(self) -> u8 {
        match self {
            Self::ComponentRemoved => 0,
            Self::EntityDestroyed => 1,
        }
    }
}

/// A durable record that replicated state ended.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tombstone {
    /// Which kind of removal this records.
    pub kind: TombstoneKind,
    /// The network entity the removal applies to.
    pub entity: NetEntityId,
    /// Stable schema id of the removed component. `Some` for
    /// [`TombstoneKind::ComponentRemoved`], `None` for
    /// [`TombstoneKind::EntityDestroyed`].
    pub schema: Option<String>,
    /// Scheduler-tick value (`Tick::get`) in effect when the removal was
    /// recorded — the same timeline `query_changed_since` cursors live on,
    /// so the server can correlate tombstones with per-client
    /// last-acknowledged ticks.
    pub tick: u64,
}

impl Tombstone {
    /// Records a replicated component removal on `entity`.
    #[must_use]
    pub fn component_removed(entity: NetEntityId, schema: &str, tick: u64) -> Self {
        Self {
            kind: TombstoneKind::ComponentRemoved,
            entity,
            schema: Some(schema.to_owned()),
            tick,
        }
    }

    /// Records the destruction of `entity`.
    #[must_use]
    pub fn entity_destroyed(entity: NetEntityId, tick: u64) -> Self {
        Self {
            kind: TombstoneKind::EntityDestroyed,
            entity,
            schema: None,
            tick,
        }
    }

    /// Canonical ordering key within a delta: entity, then removals before
    /// destructions (a narrower op applies before the wider one that
    /// subsumes it), then schema name, then kind rank.
    #[must_use]
    pub fn sort_key(&self) -> (u64, u8, &str, u8) {
        let (is_destruction, schema) = match self.schema.as_deref() {
            Some(name) => (0u8, name),
            None => (1u8, ""),
        };
        (self.entity.0, is_destruction, schema, self.kind.rank())
    }
}

/// A tombstone with its position in the log. `log_seq` values are dense and
/// increasing from 0; a client's cursor is the `log_seq` it needs next
/// (i.e. one past the last sequence it acknowledged).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedTombstone {
    /// Position of this tombstone in the log.
    pub log_seq: u64,
    /// The removal record.
    pub tombstone: Tombstone,
}

/// Bounded, ack-gated retention for removal/destruction records.
#[derive(Debug)]
pub struct TombstoneLog {
    /// Retained records in `log_seq` order. `base_seq` is the `log_seq` of
    /// `entries[0]` (or of the next record if empty): anything below it was
    /// reclaimed or dropped, and a cursor down there forces a resync.
    entries: VecDeque<LoggedTombstone>,
    /// `log_seq` to assign the next recorded tombstone; also the exclusive
    /// tip of the retained range.
    next_seq: u64,
    /// Maximum retained records; recording past it drops the oldest.
    cap: usize,
}

impl TombstoneLog {
    /// An empty log retaining at most `cap` tombstones. `cap == 0` is a
    /// degenerate but coherent configuration: nothing is ever retained, so
    /// every client with an outstanding removal resynchronizes.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            next_seq: 0,
            cap,
        }
    }

    /// Records `tombstone`, returning its assigned log sequence. Drops the
    /// oldest retained record if the log is over capacity — any cursor
    /// still needing the dropped prefix resynchronizes (drop→resync).
    pub fn record(&mut self, tombstone: Tombstone) -> u64 {
        let log_seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.entries
            .push_back(LoggedTombstone { log_seq, tombstone });
        while self.entries.len() > self.cap {
            self.entries.pop_front();
        }
        log_seq
    }

    /// Oldest retained log sequence (inclusive). Cursors below this point
    /// at dropped history.
    #[must_use]
    pub fn oldest_retained(&self) -> u64 {
        self.entries
            .front()
            .map_or(self.next_seq, |entry| entry.log_seq)
    }

    /// Next log sequence to be assigned (exclusive tip of the retained
    /// range). A cursor equal to this is fully caught up.
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// How many tombstones are currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether any tombstone is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every tombstone at or past `cursor` (the log sequence the client
    /// needs next), in log order.
    ///
    /// Fails as [`NetError::TombstoneGap`] when `cursor` points at dropped
    /// history (`cursor < oldest_retained`) or past the tip (`cursor >
    /// next_seq`): in either case the client's baseline matches nothing
    /// retained, so the caller resynchronizes it from a fresh snapshot
    /// rather than guessing.
    pub fn pending_since(&self, cursor: u64) -> Result<Vec<&LoggedTombstone>, NetError> {
        if cursor < self.oldest_retained() || cursor > self.next_seq {
            return Err(NetError::TombstoneGap {
                cursor,
                oldest_retained: self.oldest_retained(),
                next_seq: self.next_seq,
            });
        }
        Ok(self
            .entries
            .iter()
            .filter(|entry| entry.log_seq >= cursor)
            .collect())
    }

    /// Drops every retained record at or below `acked_through` — the minimum
    /// log sequence acknowledged across all connected clients. Returns how
    /// many records were reclaimed. This is the ack gate: history no client
    /// still needs is freed, while anything an unacknowledged client may
    /// still need is kept (up to the cap).
    pub fn reclaim_through(&mut self, acked_through: u64) -> usize {
        let mut reclaimed = 0;
        while self
            .entries
            .front()
            .is_some_and(|entry| entry.log_seq <= acked_through)
        {
            self.entries.pop_front();
            reclaimed += 1;
        }
        reclaimed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn removed(entity: u64, tick: u64) -> Tombstone {
        Tombstone::component_removed(NetEntityId(entity), "canary.health", tick)
    }

    #[test]
    fn pending_returns_only_unacknowledged_tombstones() {
        let mut log = TombstoneLog::new(DEFAULT_TOMBSTONE_CAP);
        let first = log.record(removed(1, 10));
        let second = log.record(removed(2, 11));
        assert_eq!(first, 0);
        assert_eq!(second, 1);

        let pending = log.pending_since(0).expect("from tip start");
        assert_eq!(pending.len(), 2);
        let pending = log.pending_since(1).expect("acked first");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].tombstone.entity, NetEntityId(2));
        assert!(log.pending_since(2).expect("fully acked").is_empty());
    }

    #[test]
    fn ack_gated_reclamation_frees_only_acknowledged_prefix() {
        let mut log = TombstoneLog::new(DEFAULT_TOMBSTONE_CAP);
        log.record(removed(1, 10));
        log.record(removed(2, 11));
        log.record(removed(3, 12));

        assert_eq!(log.reclaim_through(0), 1);
        assert_eq!(log.len(), 2);
        // The acknowledged prefix is gone, but the rest is retained: the
        // client that acked through 0 can still read from 1.
        let pending = log.pending_since(1).expect("retained");
        assert_eq!(pending.len(), 2);
        // A reclaimed cursor now points at dropped history: resync.
        assert!(matches!(
            log.pending_since(0),
            Err(NetError::TombstoneGap { .. })
        ));
    }

    #[test]
    fn cap_overflow_drops_oldest_and_forces_resync() {
        let mut log = TombstoneLog::new(2);
        log.record(removed(1, 10));
        log.record(removed(2, 11));
        log.record(removed(3, 12));
        assert_eq!(log.len(), 2);
        assert_eq!(log.oldest_retained(), 1);

        // The caught-up client still converges from its cursor.
        let pending = log.pending_since(1).expect("retained suffix");
        assert_eq!(pending.len(), 2);
        // The client that fell behind the dropped prefix resynchronizes.
        let gap = log.pending_since(0).expect_err("dropped prefix");
        assert!(gap.resync_required());
        assert!(matches!(
            gap,
            NetError::TombstoneGap {
                cursor: 0,
                oldest_retained: 1,
                next_seq: 3
            }
        ));
    }

    #[test]
    fn cursor_past_the_tip_is_a_gap_not_history() {
        let mut log = TombstoneLog::new(DEFAULT_TOMBSTONE_CAP);
        log.record(removed(1, 10));
        assert!(matches!(
            log.pending_since(7),
            Err(NetError::TombstoneGap { .. })
        ));
    }

    #[test]
    fn entity_destruction_and_removal_are_distinct_kinds() {
        let destroy = Tombstone::entity_destroyed(NetEntityId(4), 20);
        assert_eq!(destroy.schema, None);
        let remove = removed(4, 20);
        assert_ne!(destroy.kind, remove.kind);
        assert!(destroy.sort_key() > remove.sort_key());
    }
}
