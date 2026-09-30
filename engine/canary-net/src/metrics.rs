//! Session counters with bounded label sets (WP4 hardening, ADR 0027 point 8).
//!
//! [`NetMetrics`] counts what the session layer moves — frames and bytes in
//! both directions, resyncs, handshake rejects, disconnects — as global
//! aggregates plus an optional per-client breakdown. The per-client table
//! is the cardinality risk: one entry per tracked client, capped by
//! `max_tracked_clients`, evicted on disconnect
//! ([`NetMetrics::record_disconnect`]). Past the cap, new clients still
//! count toward the globals (which are always exact) but get no per-client
//! row — aggregates stay available while label churn cannot grow memory.
//!
//! Drivers wire three calls: `record_*` on every send/receive/resync,
//! `record_disconnect` next to
//! [`SessionTable::disconnect`](crate::session::SessionTable::disconnect),
//! and `record_handshake_reject` on every typed refusal (rejects have no
//! client record by construction, so they are global-only).

use std::collections::HashMap;

use crate::session::ClientId;

/// Default cap on tracked per-client rows. Large enough for a healthy
/// server's concurrent sessions; small enough that churn cannot grow the
/// table. Tune against observed session counts.
pub const DEFAULT_MAX_TRACKED_CLIENTS: usize = 256;

/// Global aggregate counters. Always exact: every event increments these
/// regardless of the per-client cap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalCounters {
    /// Frames handed to the transport.
    pub frames_sent: u64,
    /// Frames received from the transport.
    pub frames_received: u64,
    /// Payload bytes handed to the transport.
    pub bytes_sent: u64,
    /// Payload bytes received from the transport.
    pub bytes_received: u64,
    /// Resync baselines served (incremental replays and full snapshots).
    pub resyncs: u64,
    /// Handshake refusals sent (global-only: rejects have no client row).
    pub handshake_rejects: u64,
    /// Disconnects served, idle or otherwise.
    pub disconnects: u64,
}

/// Per-client counters. Best-effort under churn: a row exists only while
/// the client is tracked (see [`NetMetrics`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientCounters {
    /// Frames handed to the transport for this client.
    pub frames_sent: u64,
    /// Frames received from this client.
    pub frames_received: u64,
    /// Payload bytes handed to the transport for this client.
    pub bytes_sent: u64,
    /// Payload bytes received from this client.
    pub bytes_received: u64,
    /// Resync baselines served to this client.
    pub resyncs: u64,
}

/// Session counters: exact globals plus a capped per-client breakdown.
///
/// Insert policy: `record_*` with `Some(id)` updates the client's row when
/// one exists, inserts one when under cap, and counts global-only when the
/// table is full. [`NetMetrics::record_disconnect`] evicts the row, so a
/// steady connect/disconnect cycle holds at most one row per live client.
/// All counters saturate (never wrap): a counter that saturates has still
/// proven the event happened unboundedly often, which is the load-bearing
/// signal for abuse detection.
#[derive(Debug)]
pub struct NetMetrics {
    /// Exact aggregates over every event.
    global: GlobalCounters,
    /// Per-client breakdown, capped at `max_tracked_clients` rows.
    per_client: HashMap<ClientId, ClientCounters>,
    /// Maximum per-client rows retained.
    max_tracked_clients: usize,
}

impl NetMetrics {
    /// Empty metrics tracking at most `max_tracked_clients` per-client rows.
    /// Zero disables per-client rows entirely (global-only mode).
    #[must_use]
    pub fn new(max_tracked_clients: usize) -> Self {
        Self {
            global: GlobalCounters::default(),
            per_client: HashMap::new(),
            max_tracked_clients,
        }
    }

    /// The per-client row cap.
    #[must_use]
    pub fn max_tracked_clients(&self) -> usize {
        self.max_tracked_clients
    }

    /// Exact global aggregates.
    #[must_use]
    pub fn global(&self) -> &GlobalCounters {
        &self.global
    }

    /// Tracked row for `id`, if one exists. `None` means never seen,
    /// evicted on disconnect, or dropped past the cap — consult
    /// [`NetMetrics::global`] for the always-available aggregates.
    #[must_use]
    pub fn client(&self, id: ClientId) -> Option<&ClientCounters> {
        self.per_client.get(&id)
    }

    /// How many per-client rows are retained (always at or under the cap).
    #[must_use]
    pub fn tracked_clients(&self) -> usize {
        self.per_client.len()
    }

    /// Records one outbound frame of `bytes` payload bytes, attributed to
    /// `client` when known (`None` for pre-session traffic like handshake
    /// refusals, which still count globally).
    pub fn record_send(&mut self, client: Option<ClientId>, bytes: usize) {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        self.global.frames_sent = self.global.frames_sent.saturating_add(1);
        self.global.bytes_sent = self.global.bytes_sent.saturating_add(bytes);
        if let Some(id) = client {
            if let Some(row) = self.row_for(id) {
                row.frames_sent = row.frames_sent.saturating_add(1);
                row.bytes_sent = row.bytes_sent.saturating_add(bytes);
            }
        }
    }

    /// Records one inbound frame of `bytes` payload bytes, attributed to
    /// `client` when known.
    pub fn record_recv(&mut self, client: Option<ClientId>, bytes: usize) {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        self.global.frames_received = self.global.frames_received.saturating_add(1);
        self.global.bytes_received = self.global.bytes_received.saturating_add(bytes);
        if let Some(id) = client {
            if let Some(row) = self.row_for(id) {
                row.frames_received = row.frames_received.saturating_add(1);
                row.bytes_received = row.bytes_received.saturating_add(bytes);
            }
        }
    }

    /// Records one served resync baseline, attributed to `client` when the
    /// resync ran on a live session (`None` for reconnect replays issued
    /// before the new session row exists).
    pub fn record_resync(&mut self, client: Option<ClientId>) {
        self.global.resyncs = self.global.resyncs.saturating_add(1);
        if let Some(id) = client {
            if let Some(row) = self.row_for(id) {
                row.resyncs = row.resyncs.saturating_add(1);
            }
        }
    }

    /// Records one handshake refusal. Global-only by construction: a
    /// rejected hello never earns a client record.
    pub fn record_handshake_reject(&mut self) {
        self.global.handshake_rejects = self.global.handshake_rejects.saturating_add(1);
    }

    /// Records one disconnect and evicts the client's row. Call next to
    /// [`SessionTable::disconnect`](crate::session::SessionTable::disconnect):
    /// eviction is what keeps the table bounded across churn, and the
    /// globals already hold everything aggregates need.
    pub fn record_disconnect(&mut self, client: Option<ClientId>) {
        self.global.disconnects = self.global.disconnects.saturating_add(1);
        if let Some(id) = client {
            self.per_client.remove(&id);
        }
    }

    /// Mutable row for `id`: the existing one, a fresh one when under cap,
    /// or `None` past the cap (the event still counted globally).
    fn row_for(&mut self, id: ClientId) -> Option<&mut ClientCounters> {
        if !self.per_client.contains_key(&id) {
            if self.per_client.len() >= self.max_tracked_clients {
                return None;
            }
            self.per_client.insert(id, ClientCounters::default());
        }
        self.per_client.get_mut(&id)
    }
}

impl Default for NetMetrics {
    /// [`DEFAULT_MAX_TRACKED_CLIENTS`] per-client rows.
    fn default() -> Self {
        Self::new(DEFAULT_MAX_TRACKED_CLIENTS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_recv_resync_count_globally_and_per_client() {
        let mut metrics = NetMetrics::new(8);
        let id = ClientId(1);
        metrics.record_send(Some(id), 100);
        metrics.record_recv(Some(id), 50);
        metrics.record_resync(Some(id));
        metrics.record_handshake_reject();

        let row = metrics.client(id).expect("tracked");
        assert_eq!(row.frames_sent, 1);
        assert_eq!(row.bytes_sent, 100);
        assert_eq!(row.frames_received, 1);
        assert_eq!(row.bytes_received, 50);
        assert_eq!(row.resyncs, 1);
        let global = metrics.global();
        assert_eq!(global.frames_sent, 1);
        assert_eq!(global.bytes_sent, 100);
        assert_eq!(global.handshake_rejects, 1);
        assert_eq!(metrics.tracked_clients(), 1);
        assert_eq!(metrics.max_tracked_clients(), 8);
    }

    #[test]
    fn disconnect_evicts_the_row_but_keeps_the_aggregates() {
        let mut metrics = NetMetrics::new(8);
        let id = ClientId(7);
        metrics.record_send(Some(id), 64);
        metrics.record_disconnect(Some(id));
        assert!(metrics.client(id).is_none());
        assert_eq!(metrics.tracked_clients(), 0);
        assert_eq!(metrics.global().frames_sent, 1);
        assert_eq!(metrics.global().bytes_sent, 64);
        assert_eq!(metrics.global().disconnects, 1);
    }

    #[test]
    fn metric_state_stays_bounded_across_connect_disconnect_churn() {
        // 1024 sessions churn through a cap-8 table with no disconnect-time
        // leak: every departure evicts its row.
        let mut metrics = NetMetrics::new(8);
        for index in 0..1024u64 {
            let id = ClientId(index);
            metrics.record_send(Some(id), 16);
            metrics.record_recv(Some(id), 8);
            metrics.record_disconnect(Some(id));
            assert!(
                metrics.tracked_clients() <= 8,
                "metric table escaped its cap at session {index}"
            );
        }
        assert_eq!(metrics.tracked_clients(), 0);
        // The globals saw everything: nothing was lost to eviction.
        assert_eq!(metrics.global().frames_sent, 1024);
        assert_eq!(metrics.global().frames_received, 1024);
        assert_eq!(metrics.global().bytes_sent, 1024 * 16);
        assert_eq!(metrics.global().bytes_received, 1024 * 8);
        assert_eq!(metrics.global().disconnects, 1024);
    }

    #[test]
    fn live_overflow_counts_global_only_and_evict_frees_a_slot() {
        let mut metrics = NetMetrics::new(2);
        metrics.record_send(Some(ClientId(1)), 1);
        metrics.record_send(Some(ClientId(2)), 1);
        // Full table: the newcomer counts globally but earns no row.
        metrics.record_send(Some(ClientId(3)), 1);
        assert_eq!(metrics.tracked_clients(), 2);
        assert!(metrics.client(ClientId(3)).is_none());
        assert_eq!(metrics.global().frames_sent, 3);
        // A departure frees its slot: the next newcomer is tracked again.
        metrics.record_disconnect(Some(ClientId(1)));
        metrics.record_send(Some(ClientId(3)), 1);
        assert!(metrics.client(ClientId(3)).is_some());
        assert_eq!(metrics.global().frames_sent, 4);
    }

    #[test]
    fn anonymous_traffic_counts_globally_without_a_row() {
        let mut metrics = NetMetrics::default();
        assert_eq!(metrics.max_tracked_clients(), DEFAULT_MAX_TRACKED_CLIENTS);
        metrics.record_send(None, 32);
        metrics.record_resync(None);
        metrics.record_disconnect(None);
        assert_eq!(metrics.tracked_clients(), 0);
        assert_eq!(metrics.global().frames_sent, 1);
        assert_eq!(metrics.global().resyncs, 1);
        assert_eq!(metrics.global().disconnects, 1);
    }

    #[test]
    fn zero_cap_is_global_only_mode() {
        let mut metrics = NetMetrics::new(0);
        metrics.record_send(Some(ClientId(1)), 10);
        assert!(metrics.client(ClientId(1)).is_none());
        assert_eq!(metrics.tracked_clients(), 0);
        assert_eq!(metrics.global().frames_sent, 1);
    }
}
