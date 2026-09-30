//! Reconnect/resync baseline retention (WP4 hardening, ADR 0027 point 10).
//!
//! A reconnect is a *new* session (new session id, fresh sequence gates —
//! see [`crate::session`]), but it need not restart from a live snapshot
//! when the server still retains the baseline the client converged to.
//! [`BaselineRetention`] is the minimal server-side history that makes that
//! promise keepable: a bounded ring of recent authoritative snapshots
//! (full post-tick states, newest first in push order) consulted alongside
//! the [`TombstoneLog`](crate::tombstone::TombstoneLog), which already
//! outlives any one session.
//!
//! Resync planning ([`BaselineRetention::plan`]) takes the reconnecting
//! client's last-acked simulation tick plus the tombstone cursor it still
//! holds, and answers one of:
//!
//! - [`ResyncPlan::Covered`]: retention holds a snapshot at or before the
//!   acked tick (a baseline the client provably applied) *and* the tombstone
//!   log still holds every removal since the client's cursor. The server
//!   sends that snapshot, replays the retained tombstones as a catch-up
//!   delta, then streams live deltas. Incremental, no full state transfer.
//! - [`ResyncPlan::FullSnapshot`]: the client fell behind the dropped
//!   prefix (tick older than retention, cursor behind the tombstone log, or
//!   nothing retained). The server sends its live full state instead —
//!   convergence without pretending the gap is bridgeable.
//!
//! Design delta from WP3 (deliberate, documented): per-delta journal replay
//! is *not* implemented. Deltas between the shared baseline and the live
//! tip are superseded by the retained snapshots plus tombstones — a newer
//! full state plus the removal log converges the same map with less
//! bookkeeping. A workload that proves snapshot transfer too costly for its
//! resync rate may justify a delta journal as follow-up work; nothing in
//! this API precludes adding one beside the snapshot ring.
//!
//! Memory is bounded: at most `cap` snapshots, and each snapshot is still
//! gated by [`NetLimits`](crate::limits::NetLimits) at encode time, so the
//! worst case is `cap` times the frame bound. The server pushes one
//! snapshot per authoritative tick it wants resyncable; ticks it skips
//! pushing are simply not resyncable baselines.

use std::collections::VecDeque;

use crate::ids::SimTick;
use crate::replication::ReplicatedEntry;
use crate::tombstone::{Tombstone, TombstoneLog};

/// Default cap on retained baseline snapshots. Large enough that a client
/// reconnecting within a few ticks finds its baseline; small enough that a
/// vanished client cannot pin unbounded server memory. A starting point —
/// tune against measured snapshot sizes and observed reconnect delays.
pub const DEFAULT_BASELINE_CAP: usize = 16;

/// One retained authoritative state: the full live entries after `tick`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineSnapshot {
    /// Simulation tick this state was captured at.
    pub tick: SimTick,
    /// Full authoritative entries (live state, canonical order not required
    /// here — [`encode_snapshot`](crate::replication::encode_snapshot)
    /// sorts at send time).
    pub entries: Vec<ReplicatedEntry>,
}

/// Bounded ring of recent authoritative snapshots for reconnect replay.
///
/// Push order is tick order: the server pushes one entry per authoritative
/// tick it wants resyncable, with nondecreasing ticks. Pushing past `cap`
/// drops the oldest baseline, and any client acked at or before the dropped
/// tick falls back to [`ResyncPlan::FullSnapshot`] (drop→resync, the same
/// rule as the tombstone cap).
#[derive(Debug)]
pub struct BaselineRetention {
    /// Retained baselines in push (tick) order; front is oldest.
    snapshots: VecDeque<BaselineSnapshot>,
    /// Maximum retained baselines.
    cap: usize,
}

impl BaselineRetention {
    /// An empty retention ring holding at most `cap` baselines. `cap == 0`
    /// retains nothing: every reconnect takes the full-snapshot path.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            snapshots: VecDeque::new(),
            cap,
        }
    }

    /// The baseline cap.
    #[must_use]
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// How many baselines are retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// Whether no baseline is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    /// Oldest retained tick, if any. A client acked before this tick fell
    /// behind the dropped prefix.
    #[must_use]
    pub fn oldest_tick(&self) -> Option<SimTick> {
        self.snapshots.front().map(|snapshot| snapshot.tick)
    }

    /// Newest retained baseline, if any.
    #[must_use]
    pub fn latest(&self) -> Option<&BaselineSnapshot> {
        self.snapshots.back()
    }

    /// Retains the authoritative post-`tick` state, dropping the oldest
    /// baseline past the cap. Ticks should be nondecreasing — pushing an
    /// older tick after a newer one keeps both (the ring is push-ordered,
    /// not sorted), and `plan` answers against push order, so a
    /// time-travelling push only confuses the planner, never memory safety.
    pub fn push(&mut self, tick: SimTick, entries: Vec<ReplicatedEntry>) {
        self.snapshots.push_back(BaselineSnapshot { tick, entries });
        while self.snapshots.len() > self.cap {
            self.snapshots.pop_front();
        }
    }

    /// Plans the resync for a reconnecting client that converged to
    /// `last_acked_tick` and still holds `tombstone_cursor` (the log
    /// sequence it needs next — see
    /// [`TombstoneLog::pending_since`](crate::tombstone::TombstoneLog::pending_since)).
    ///
    /// Covered requires both halves: a retained baseline at or before the
    /// acked tick (so the snapshot names state the client provably
    /// applied) and a tombstone log that still holds the client's cursor.
    /// Either half missing — or an empty ring — answers `FullSnapshot`:
    /// the server sends live full state instead of guessing across a gap.
    /// A cursor *past* the log tip (the client claims a newer log than the
    /// server has) is likewise unprovable and answers `FullSnapshot`.
    #[must_use]
    pub fn plan(
        &self,
        last_acked_tick: SimTick,
        tombstone_cursor: u64,
        tombstones: &TombstoneLog,
    ) -> ResyncPlan {
        let Some(baseline) = self.newest_at_or_before(last_acked_tick) else {
            return ResyncPlan::FullSnapshot;
        };
        let pending = match tombstones.pending_since(tombstone_cursor) {
            Ok(pending) => pending
                .into_iter()
                .map(|logged| logged.tombstone.clone())
                .collect(),
            Err(_) => return ResyncPlan::FullSnapshot,
        };
        ResyncPlan::Covered {
            tick: baseline.tick,
            entries: baseline.entries.clone(),
            tombstones: pending,
        }
    }

    /// Newest retained baseline at or before `tick`: state the client
    /// provably applied when it converged to `tick` (it applied everything
    /// up to its ack). `None` when retention starts past `tick` (fell
    /// behind) or is empty.
    fn newest_at_or_before(&self, tick: SimTick) -> Option<&BaselineSnapshot> {
        self.snapshots
            .iter()
            .rev()
            .find(|snapshot| snapshot.tick <= tick)
    }
}

/// What the server sends a reconnecting client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResyncPlan {
    /// Retention covers the client's baseline: converge to `entries`
    /// (authoritative state at `tick`), apply `tombstones` as a catch-up
    /// delta, then stream live deltas. The caller still encodes through
    /// [`encode_snapshot`](crate::replication::encode_snapshot) and
    /// [`encode_delta`](crate::replication::encode_delta), so canonical
    /// order and frame bounds hold on the replay exactly as on live traffic.
    Covered {
        /// Tick of the shared baseline snapshot.
        tick: SimTick,
        /// Authoritative entries at that tick.
        entries: Vec<ReplicatedEntry>,
        /// Removals since the client's cursor, in log order.
        tombstones: Vec<Tombstone>,
    },
    /// The client's baseline is unprovable from retention: send live full
    /// state. Convergence without incremental replay.
    FullSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::NetEntityId;
    use crate::tombstone::TombstoneLog;

    fn entry(entity: u64, payload: &[u8]) -> ReplicatedEntry {
        ReplicatedEntry {
            entity: NetEntityId(entity),
            schema: "canary.health".to_string(),
            payload: payload.to_vec(),
        }
    }

    fn retention_with_ticks(ticks: &[u64]) -> BaselineRetention {
        let mut retention = BaselineRetention::new(DEFAULT_BASELINE_CAP);
        for tick in ticks {
            retention.push(SimTick(*tick), vec![entry(1, b"hp")]);
        }
        retention
    }

    #[test]
    fn push_evicts_oldest_past_the_cap() {
        let mut retention = BaselineRetention::new(2);
        retention.push(SimTick(41), vec![entry(1, b"a")]);
        retention.push(SimTick(42), vec![entry(1, b"b")]);
        retention.push(SimTick(43), vec![entry(1, b"c")]);
        assert_eq!(retention.len(), 2);
        assert!(!retention.is_empty());
        assert_eq!(retention.oldest_tick(), Some(SimTick(42)));
        assert_eq!(retention.latest().expect("newest").tick, SimTick(43));
        assert_eq!(retention.cap(), 2);
    }

    #[test]
    fn covered_replays_baseline_plus_tombstones_since_cursor() {
        let retention = retention_with_ticks(&[41, 42, 43]);
        let mut tombstones = TombstoneLog::new(64);
        tombstones.record(Tombstone::component_removed(
            NetEntityId(2),
            "canary.health",
            41,
        ));
        tombstones.record(Tombstone::component_removed(
            NetEntityId(3),
            "canary.health",
            42,
        ));
        // Client converged to tick 43 having applied tombstone 0: the
        // baseline is the newest snapshot at or before 43, and only the
        // unacknowledged removal replays.
        let plan = retention.plan(SimTick(43), 1, &tombstones);
        match plan {
            ResyncPlan::Covered {
                tick,
                entries,
                tombstones: replay,
            } => {
                assert_eq!(tick, SimTick(43));
                assert_eq!(entries.len(), 1);
                assert_eq!(replay.len(), 1);
                assert_eq!(replay[0].entity, NetEntityId(3));
            }
            ResyncPlan::FullSnapshot => panic!("baseline and cursor retained: must be covered"),
        }
    }

    #[test]
    fn baseline_older_than_acked_tick_is_shared_history() {
        // Ticks the server skipped pushing are not baselines, but a client
        // acked past them still shares the newest retained baseline at or
        // before its ack.
        let retention = retention_with_ticks(&[41, 43]);
        let tombstones = TombstoneLog::new(64);
        let plan = retention.plan(SimTick(42), 0, &tombstones);
        match plan {
            ResyncPlan::Covered { tick, .. } => assert_eq!(tick, SimTick(41)),
            ResyncPlan::FullSnapshot => panic!("tick 41 baseline covers ack 42"),
        }
    }

    #[test]
    fn client_behind_the_dropped_prefix_takes_full_snapshot() {
        let mut retention = BaselineRetention::new(2);
        retention.push(SimTick(41), vec![entry(1, b"a")]);
        retention.push(SimTick(42), vec![entry(1, b"b")]);
        retention.push(SimTick(43), vec![entry(1, b"c")]);
        let tombstones = TombstoneLog::new(64);
        // Acked tick 41 fell off the ring with the evicted baseline.
        assert_eq!(
            retention.plan(SimTick(41), 0, &tombstones),
            ResyncPlan::FullSnapshot
        );
        // So does an empty ring, regardless of the ack.
        let empty = BaselineRetention::new(4);
        assert!(empty.is_empty());
        assert_eq!(
            empty.plan(SimTick(43), 0, &tombstones),
            ResyncPlan::FullSnapshot
        );
    }

    #[test]
    fn tombstone_gap_forces_full_snapshot_despite_retained_baseline() {
        let retention = retention_with_ticks(&[41, 42]);
        let mut tombstones = TombstoneLog::new(1);
        tombstones.record(Tombstone::component_removed(
            NetEntityId(2),
            "canary.health",
            41,
        ));
        tombstones.record(Tombstone::component_removed(
            NetEntityId(3),
            "canary.health",
            42,
        ));
        // Cursor 0 points at the dropped log prefix: unprovable.
        assert_eq!(
            retention.plan(SimTick(42), 0, &tombstones),
            ResyncPlan::FullSnapshot
        );
        // Cursor past the tip claims history the server never had: ditto.
        assert_eq!(
            retention.plan(SimTick(42), 99, &tombstones),
            ResyncPlan::FullSnapshot
        );
    }
}
