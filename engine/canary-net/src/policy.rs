//! Type-level replication opt-in (ADR 0027, point 3).
//!
//! [`ReplicationRegistry`] is the server's per-schema allow-list: the set of
//! stable schema ids whose components may be published to clients. It
//! composes with the entity-level [`Replicated`](https://github.com/HylightGames/canary)
//! marker in `canary-ecs` — a component replicates for an entity only when
//! the entity carries the marker *and* the component's schema is registered
//! here. Both halves are explicit because they answer different questions:
//! "which entities does the server publish" versus "which component types
//! does the session speak".
//!
//! The dirty set itself still comes from
//! `World::query_changed_since(last-acked tick)` per opted-in schema (ADR
//! 0014) — this registry only decides which schemas are consulted, and is
//! deliberately not a parallel dirty-flag system.
//!
//! ## Connection admission and liveness policy (WP4 hardening)
//!
//! [`ConnectionPolicy`] bundles the two server-side guards that keep one
//! hostile or vanished peer from spending unbounded resources:
//!
//! - Idle eviction ([`IdlePolicy`] + [`IdleTracker`]): every connected
//!   client owes inbound activity at least once per `max_idle_ticks`. The
//!   driver calls [`IdleTracker::sweep_idle`] on its tick and disconnects
//!   each returned client with
//!   [`DisconnectReason::IdleTimeout`](crate::error::DisconnectReason::IdleTimeout).
//! - Handshake abuse backoff ([`BanPolicy`] + [`HandshakeGate`]):
//!   consecutive handshake rejects from one peer label accrue toward a
//!   temporary ban. While banned the server answers
//!   [`RejectReason::TemporarilyBanned`](crate::handshake::RejectReason::TemporarilyBanned)
//!   instead of re-running admission, and a successful handshake forgives
//!   the label's history.
//!
//! Both guards run on a caller-supplied `u64` clock (server tick count,
//! millisecond counter — any monotonic unit). This crate never reads
//! wall-clock time, so tests drive policy on a fake clock with zero
//! flakiness. `saturating_*` arithmetic throughout means a clock that jumps
//! backward can delay enforcement but never false-trigger it.

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};

use crate::session::ClientId;

/// Server-side allow-list of replicable component schemas.
#[derive(Debug, Clone, Default)]
pub struct ReplicationRegistry {
    /// Opted-in schema ids. A `BTreeSet` (not `HashSet`) so iteration order
    /// is deterministic — the server walks the same schema order every tick,
    /// and two servers with the same registrations capture identical sets.
    schemas: BTreeSet<String>,
}

impl ReplicationRegistry {
    /// An empty registry: nothing replicates until registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schemas: BTreeSet::new(),
        }
    }

    /// Opts `schema_id` into replication. Returns `true` when newly added,
    /// `false` when it was already registered (idempotent).
    pub fn register(&mut self, schema_id: &str) -> bool {
        self.schemas.insert(schema_id.to_owned())
    }

    /// Removes `schema_id` from the replicated set. Returns `true` when one
    /// was removed. Entities already published under it keep their last
    /// state client-side — unpublishing stops future updates; actively
    /// clearing clients is a removal tombstone, not a registry edit.
    pub fn unregister(&mut self, schema_id: &str) -> bool {
        self.schemas.remove(schema_id)
    }

    /// Whether `schema_id` is currently opted into replication.
    #[must_use]
    pub fn is_replicated(&self, schema_id: &str) -> bool {
        self.schemas.contains(schema_id)
    }

    /// Opted-in schemas in deterministic (lexicographic) order — the order
    /// the server walks when capturing dirty sets.
    pub fn schemas(&self) -> impl Iterator<Item = &str> {
        self.schemas.iter().map(String::as_str)
    }

    /// How many schemas are opted in.
    #[must_use]
    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    /// Whether any schema is opted in.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
    }
}

/// Default idle deadline in clock ticks: a client silent this long is
/// disconnected. The unit is caller-chosen (see [`ConnectionPolicy`]);
/// 300 ticks is 5 minutes at one tick per second, 5 seconds at 60 Hz —
/// callers with faster ticks should raise it. A starting point, not a
/// measured budget: tune against observed session lifetimes.
pub const DEFAULT_MAX_IDLE_TICKS: u64 = 300;

/// Default consecutive handshake rejects from one peer label that trigger
/// a temporary ban. Low enough to blunt hello-flooding, high enough that
/// a stale client disagreeing on versions a few times is not banned.
pub const DEFAULT_MAX_HANDSHAKE_REJECTS: u32 = 5;

/// Default ban length in clock ticks. Matches the idle deadline's unit;
/// twice the default idle deadline so a banned flooder stays out meaningfully
/// longer than one idle sweep.
pub const DEFAULT_BAN_DURATION_TICKS: u64 = 600;

/// Default cap on tracked handshake peer labels. Bounds [`HandshakeGate`]
/// memory under label churn; the least-recently-seen label is evicted past
/// it (fail-open for the evicted label only — global admission still runs).
pub const DEFAULT_MAX_TRACKED_PEERS: usize = 1024;

/// Server-side connection guards: idle eviction plus handshake abuse
/// backoff.
///
/// Both sub-policies share one clock convention: `u64` ticks in a
/// caller-chosen monotonic unit (server tick count, millisecond counter).
/// Pick one unit and use it for every `now` argument; mixing units between
/// calls silently misconfigures both deadlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionPolicy {
    /// Idle eviction config.
    pub idle: IdlePolicy,
    /// Handshake ban config.
    pub ban: BanPolicy,
}

impl ConnectionPolicy {
    /// Policies from their defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            idle: IdlePolicy::default(),
            ban: BanPolicy::default(),
        }
    }
}

impl Default for ConnectionPolicy {
    /// [`ConnectionPolicy::new`]: defaults for both sub-policies.
    fn default() -> Self {
        Self::new()
    }
}

/// How long a connected client may go without inbound activity before the
/// driver disconnects it with
/// [`DisconnectReason::IdleTimeout`](crate::error::DisconnectReason::IdleTimeout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdlePolicy {
    /// Ticks of inbound silence after which a client is idle-timed-out.
    /// Zero means any client with no activity in the current tick is idle
    /// (useful only for tests — production callers want a real deadline).
    pub max_idle_ticks: u64,
}

impl IdlePolicy {
    /// A deadline of `max_idle_ticks` silent ticks.
    #[must_use]
    pub fn new(max_idle_ticks: u64) -> Self {
        Self { max_idle_ticks }
    }
}

impl Default for IdlePolicy {
    /// [`DEFAULT_MAX_IDLE_TICKS`] silent ticks.
    fn default() -> Self {
        Self {
            max_idle_ticks: DEFAULT_MAX_IDLE_TICKS,
        }
    }
}

/// Tracks last-activity ticks per connected client and reports idle ones.
///
/// Memory is bounded by the session count: entries are inserted on connect
/// ([`IdleTracker::mark_connected`]) and removed on disconnect
/// ([`IdleTracker::remove`]), so at most one entry per live client exists.
/// A driver that forgets `remove` leaks one small entry per departed
/// client — wire `remove` next to
/// [`SessionTable::disconnect`](crate::session::SessionTable::disconnect).
#[derive(Debug, Default)]
pub struct IdleTracker {
    /// Idle deadline config.
    policy: IdlePolicy,
    /// Last tick with inbound activity per connected client.
    last_activity: HashMap<ClientId, u64>,
}

impl IdleTracker {
    /// An empty tracker enforcing `policy`.
    #[must_use]
    pub fn new(policy: IdlePolicy) -> Self {
        Self {
            policy,
            last_activity: HashMap::new(),
        }
    }

    /// The enforced idle deadline.
    #[must_use]
    pub fn policy(&self) -> IdlePolicy {
        self.policy
    }

    /// Records a fresh connection at `now`. Re-connecting an already
    /// tracked id resets its deadline (no residue from the old connection).
    pub fn mark_connected(&mut self, id: ClientId, now: u64) {
        self.last_activity.insert(id, now);
    }

    /// Records inbound activity (frame, input, ack — anything proving the
    /// peer is alive) at `now`. Unknown ids are tracked on first sight so
    /// a driver that only calls this (and never `mark_connected`) still
    /// enforces deadlines; `remove` still bounds memory.
    pub fn note_activity(&mut self, id: ClientId, now: u64) {
        self.last_activity.insert(id, now);
    }

    /// Forgets `id`: call on disconnect. Returns `true` when an entry
    /// existed.
    pub fn remove(&mut self, id: ClientId) -> bool {
        self.last_activity.remove(&id).is_some()
    }

    /// Whether `id` is past its deadline at `now`: silence longer than
    /// `max_idle_ticks`. Unknown ids are never idle (nothing to evict).
    /// A `now` behind the recorded activity saturates to zero silence —
    /// a backward clock delays enforcement, never false-triggers it.
    #[must_use]
    pub fn is_idle(&self, id: ClientId, now: u64) -> bool {
        self.last_activity
            .get(&id)
            .is_some_and(|last| now.saturating_sub(*last) > self.policy.max_idle_ticks)
    }

    /// Every tracked client past its deadline at `now`, in `ClientId` order
    /// (deterministic for logs and tests — `HashMap` order is not). The
    /// caller disconnects each with
    /// [`DisconnectReason::IdleTimeout`](crate::error::DisconnectReason::IdleTimeout).
    /// Returned ids stay tracked until `remove`: a driver that sweeps
    /// without disconnecting sees the same ids again next tick.
    #[must_use]
    pub fn sweep_idle(&self, now: u64) -> Vec<ClientId> {
        let mut idle: Vec<ClientId> = self
            .last_activity
            .iter()
            .filter(|(_, last)| now.saturating_sub(**last) > self.policy.max_idle_ticks)
            .map(|(id, _)| *id)
            .collect();
        idle.sort_by_key(|id| id.0);
        idle
    }

    /// How many clients are tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.last_activity.len()
    }

    /// Whether no client is tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.last_activity.is_empty()
    }
}

/// When consecutive handshake rejects from one peer label turn into a
/// temporary ban.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BanPolicy {
    /// Consecutive rejects from one label that trigger a ban. Zero
    /// disables banning entirely (fail-open: rejects are still returned to
    /// the peer, just never escalated) — the degenerate configuration for
    /// callers that run their own abuse policy elsewhere.
    pub max_consecutive_rejects: u32,
    /// How long a ban lasts in clock ticks from the reject that triggered it.
    pub ban_duration_ticks: u64,
    /// Cap on tracked peer labels. Past it the least-recently-seen label
    /// is evicted to admit the newcomer (fail-open for the evicted label
    /// only). Zero disables tracking and therefore banning.
    pub max_tracked_peers: usize,
}

impl BanPolicy {
    /// A ban after `max_consecutive_rejects` consecutive rejects, lasting
    /// `ban_duration_ticks` and tracking at most `max_tracked_peers` labels.
    #[must_use]
    pub fn new(
        max_consecutive_rejects: u32,
        ban_duration_ticks: u64,
        max_tracked_peers: usize,
    ) -> Self {
        Self {
            max_consecutive_rejects,
            ban_duration_ticks,
            max_tracked_peers,
        }
    }
}

impl Default for BanPolicy {
    /// [`DEFAULT_MAX_HANDSHAKE_REJECTS`] rejects trigger a
    /// [`DEFAULT_BAN_DURATION_TICKS`]-tick ban across at most
    /// [`DEFAULT_MAX_TRACKED_PEERS`] labels.
    fn default() -> Self {
        Self {
            max_consecutive_rejects: DEFAULT_MAX_HANDSHAKE_REJECTS,
            ban_duration_ticks: DEFAULT_BAN_DURATION_TICKS,
            max_tracked_peers: DEFAULT_MAX_TRACKED_PEERS,
        }
    }
}

/// Per-label handshake abuse state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BanRecord {
    /// Rejects since the last accepted handshake or served ban.
    consecutive_rejects: u32,
    /// Tick at which the ban lifts, while banned.
    banned_until: Option<u64>,
    /// Last tick this label was seen (reject, ban check, or forgiveness).
    /// Eviction order under the label cap.
    last_seen: u64,
}

/// Counts consecutive handshake rejects per peer label toward temporary
/// bans.
///
/// Labels are caller-supplied opaque strings (remote address, connection
/// tag — log-only identity per ADR 0027 point 9, never authority). Memory
/// is bounded by [`BanPolicy::max_tracked_peers`]: past it the
/// least-recently-seen label is evicted. A ban is checked with
/// [`HandshakeGate::is_banned`] before running admission; a served ban
/// forgives the count; a successful handshake ([`HandshakeGate::note_accepted`])
/// erases the label entirely.
#[derive(Debug, Default)]
pub struct HandshakeGate {
    /// Ban config.
    policy: BanPolicy,
    /// Abuse state by peer label.
    peers: HashMap<String, BanRecord>,
}

impl HandshakeGate {
    /// An empty gate enforcing `policy`.
    #[must_use]
    pub fn new(policy: BanPolicy) -> Self {
        Self {
            policy,
            peers: HashMap::new(),
        }
    }

    /// The enforced ban policy.
    #[must_use]
    pub fn policy(&self) -> BanPolicy {
        self.policy
    }

    /// Records a handshake reject for `peer` at `now`. Returns `true` when
    /// the peer is banned as a result (or already was — an in-force ban
    /// stays in force and the new reject only refreshes recency). Returns
    /// `false` when banning is disabled or the label cap forbids tracking
    /// (fail-open, documented in [`BanPolicy`]).
    pub fn note_reject(&mut self, peer: &str, now: u64) -> bool {
        if self.policy.max_consecutive_rejects == 0 || self.policy.max_tracked_peers == 0 {
            return false;
        }
        // An unexpired ban stays in force; the new reject only refreshes
        // recency so an active flooder is not evicted as "stale".
        if self.is_banned(peer, now) {
            if let Some(record) = self.peers.get_mut(peer) {
                record.last_seen = now;
            }
            return true;
        }
        if !self.peers.contains_key(peer) && self.peers.len() >= self.policy.max_tracked_peers {
            self.evict_lru();
        }
        let record = match self.peers.entry(peer.to_owned()) {
            Entry::Occupied(occupied) => occupied.into_mut(),
            Entry::Vacant(vacant) => vacant.insert(BanRecord {
                consecutive_rejects: 0,
                banned_until: None,
                last_seen: now,
            }),
        };
        record.consecutive_rejects = record.consecutive_rejects.saturating_add(1);
        record.last_seen = now;
        if record.consecutive_rejects >= self.policy.max_consecutive_rejects {
            record.banned_until = Some(now.saturating_add(self.policy.ban_duration_ticks));
            true
        } else {
            false
        }
    }

    /// Whether `peer` is banned at `now`. A served ban (`now` at or past
    /// its expiry) is forgiven on read: the count resets and this returns
    /// `false`. Unknown labels and labels with no active ban are not banned.
    pub fn is_banned(&mut self, peer: &str, now: u64) -> bool {
        let Some(record) = self.peers.get_mut(peer) else {
            return false;
        };
        match record.banned_until {
            None => false,
            Some(until) if now >= until => {
                record.banned_until = None;
                record.consecutive_rejects = 0;
                record.last_seen = now;
                false
            }
            Some(_) => true,
        }
    }

    /// Records an accepted handshake: the label's reject history is erased
    /// (a legitimate client that fixed its versions starts clean).
    pub fn note_accepted(&mut self, peer: &str) {
        self.peers.remove(peer);
    }

    /// Forgets `peer` unconditionally. Returns `true` when an entry existed.
    pub fn remove(&mut self, peer: &str) -> bool {
        self.peers.remove(peer).is_some()
    }

    /// Clears every served ban at `now` (forgives each count like
    /// [`HandshakeGate::is_banned`] does). Returns how many bans were
    /// cleared. Labels with live bans or mere reject counts are untouched.
    pub fn evict_expired(&mut self, now: u64) -> usize {
        let mut cleared = 0;
        for record in self.peers.values_mut() {
            if record.banned_until.is_some_and(|until| now >= until) {
                record.banned_until = None;
                record.consecutive_rejects = 0;
                record.last_seen = now;
                cleared += 1;
            }
        }
        cleared
    }

    /// Consecutive rejects currently held against `peer` (zero for unknown
    /// labels or forgiven ones). Drivers use this for logs, never for
    /// authority.
    #[must_use]
    pub fn consecutive_rejects(&self, peer: &str) -> u32 {
        self.peers
            .get(peer)
            .map_or(0, |record| record.consecutive_rejects)
    }

    /// How many peer labels are tracked (always at or under the cap).
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// Whether no peer label is tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Evicts the least-recently-seen label to admit a newcomer under the
    /// cap. Ties break arbitrarily (whichever the map yields first) — only
    /// recency rank is contractual, never which tied label goes.
    fn evict_lru(&mut self) {
        let oldest = self
            .peers
            .iter()
            .min_by_key(|(_, record)| record.last_seen)
            .map(|(peer, _)| peer.clone());
        if let Some(peer) = oldest {
            self.peers.remove(&peer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_idempotent_and_deterministic() {
        let mut registry = ReplicationRegistry::new();
        assert!(registry.is_empty());
        assert!(registry.register("canary.velocity"));
        assert!(!registry.register("canary.velocity"));
        registry.register("canary.health");
        assert!(registry.is_replicated("canary.health"));
        assert!(!registry.is_replicated("canary.transform"));
        // Deterministic walk order regardless of insertion order.
        let walked: Vec<&str> = registry.schemas().collect();
        assert_eq!(walked, vec!["canary.health", "canary.velocity"]);
    }

    #[test]
    fn unregister_stops_future_updates() {
        let mut registry = ReplicationRegistry::new();
        registry.register("canary.health");
        assert!(registry.unregister("canary.health"));
        assert!(!registry.unregister("canary.health"));
        assert!(!registry.is_replicated("canary.health"));
    }

    fn idle_tracker(deadline: u64) -> IdleTracker {
        IdleTracker::new(IdlePolicy::new(deadline))
    }

    #[test]
    fn idle_deadline_enforced_on_tick_with_fake_clock() {
        let mut tracker = idle_tracker(10);
        let slow = ClientId(1);
        let live = ClientId(2);
        tracker.mark_connected(slow, 0);
        tracker.mark_connected(live, 0);
        // Heartbeats keep the live client fresh; the slow one goes silent.
        tracker.note_activity(live, 5);
        tracker.note_activity(live, 10);
        assert!(!tracker.is_idle(slow, 10));
        assert!(!tracker.is_idle(live, 10));
        // Silence past the deadline (strictly greater) is idle.
        assert!(tracker.is_idle(slow, 11));
        assert!(!tracker.is_idle(live, 11));
        // The sweep reports exactly the idle set, in id order — the driver
        // disconnects each with `DisconnectReason::IdleTimeout`.
        let idle = tracker.sweep_idle(11);
        assert_eq!(idle, vec![slow]);
        let reason = crate::error::DisconnectReason::IdleTimeout;
        assert_eq!(reason, crate::error::DisconnectReason::IdleTimeout);
        // Activity after the sweep (a late heartbeat) forgives before the
        // driver acts; disconnecting removes tracking entirely.
        tracker.note_activity(slow, 11);
        assert!(tracker.sweep_idle(11).is_empty());
        assert!(tracker.remove(slow));
        assert!(!tracker.remove(slow));
        assert_eq!(tracker.len(), 1);
    }

    #[test]
    fn backward_clock_never_false_times_out() {
        let mut tracker = idle_tracker(10);
        let id = ClientId(3);
        tracker.mark_connected(id, 100);
        // `now` behind the recorded activity saturates to zero silence.
        assert!(!tracker.is_idle(id, 50));
        assert!(tracker.sweep_idle(50).is_empty());
        // Unknown ids are never idle: nothing to evict.
        assert!(!tracker.is_idle(ClientId(99), u64::MAX));
    }

    #[test]
    fn disconnect_keeps_idle_tracking_bounded_by_live_sessions() {
        let mut tracker = idle_tracker(10);
        for index in 0..64u64 {
            tracker.mark_connected(ClientId(index), 0);
        }
        assert_eq!(tracker.len(), 64);
        assert!(!tracker.is_empty());
        for index in 0..64u64 {
            assert!(tracker.remove(ClientId(index)));
        }
        assert!(tracker.is_empty());
        assert!(tracker.sweep_idle(1_000_000).is_empty());
    }

    fn ban_gate(rejects: u32, duration: u64, cap: usize) -> HandshakeGate {
        HandshakeGate::new(BanPolicy::new(rejects, duration, cap))
    }

    #[test]
    fn rejects_below_threshold_do_not_ban() {
        let mut gate = ban_gate(3, 100, 16);
        assert!(!gate.note_reject("10.0.0.9", 0));
        assert!(!gate.note_reject("10.0.0.9", 1));
        assert_eq!(gate.consecutive_rejects("10.0.0.9"), 2);
        assert!(!gate.is_banned("10.0.0.9", 2));
        assert_eq!(gate.len(), 1);
    }

    #[test]
    fn ban_triggers_at_threshold_and_expires_on_fake_clock() {
        let mut gate = ban_gate(3, 100, 16);
        assert!(!gate.note_reject("flooder", 0));
        assert!(!gate.note_reject("flooder", 10));
        // The third consecutive reject bans; the driver answers
        // `RejectReason::TemporarilyBanned` while `is_banned` holds.
        assert!(gate.note_reject("flooder", 20));
        assert!(gate.is_banned("flooder", 20));
        assert!(gate.is_banned("flooder", 119));
        // Further rejects while banned keep the ban in force.
        assert!(gate.note_reject("flooder", 50));
        assert!(gate.is_banned("flooder", 50));
        // A served ban forgives the count: the label starts clean.
        assert!(!gate.is_banned("flooder", 120));
        assert_eq!(gate.consecutive_rejects("flooder"), 0);
        assert!(!gate.note_reject("flooder", 120));
        assert_eq!(gate.consecutive_rejects("flooder"), 1);
    }

    #[test]
    fn accepted_handshake_forgives_reject_history() {
        let mut gate = ban_gate(2, 100, 16);
        assert!(!gate.note_reject("flaky", 0));
        gate.note_accepted("flaky");
        assert_eq!(gate.consecutive_rejects("flaky"), 0);
        assert!(gate.is_empty());
        // Post-fix rejects accrue from zero again.
        assert!(!gate.note_reject("flaky", 1));
        assert!(gate.note_reject("flaky", 2));
    }

    #[test]
    fn bans_are_per_peer_and_others_are_unaffected() {
        let mut gate = ban_gate(2, 100, 16);
        assert!(!gate.note_reject("good", 0));
        assert!(!gate.note_reject("bad", 0));
        assert!(gate.note_reject("bad", 1));
        assert!(gate.is_banned("bad", 1));
        assert!(!gate.is_banned("good", 1));
        assert_eq!(gate.consecutive_rejects("good"), 1);
        assert!(gate.remove("bad"));
        assert!(!gate.is_banned("bad", 1));
    }

    #[test]
    fn evict_expired_clears_only_served_bans() {
        let mut gate = ban_gate(1, 10, 16);
        assert!(gate.note_reject("old", 0));
        assert!(gate.note_reject("fresh", 9));
        assert_eq!(gate.evict_expired(10), 1);
        assert!(!gate.is_banned("old", 10));
        assert!(gate.is_banned("fresh", 10));
        assert_eq!(gate.evict_expired(10), 0);
    }

    #[test]
    fn peer_table_stays_bounded_under_label_churn() {
        let mut gate = ban_gate(1_000, 10, 8);
        for index in 0..256u64 {
            let label = format!("peer-{index}");
            assert!(!gate.note_reject(&label, index));
            assert!(
                gate.len() <= 8,
                "label table escaped its cap at {index}: {}",
                gate.len()
            );
        }
        assert_eq!(gate.len(), 8);
        // The survivors are the most recently seen: oldest churn evicted.
        assert!(gate.consecutive_rejects("peer-255") > 0);
        assert_eq!(gate.consecutive_rejects("peer-0"), 0);
    }

    #[test]
    fn zero_threshold_disables_banning_but_keeps_rejects_visible() {
        let mut gate = ban_gate(0, 100, 16);
        assert!(!gate.note_reject("flooder", 0));
        assert!(!gate.is_banned("flooder", 0));
        assert!(gate.is_empty());
    }

    #[test]
    fn connection_policy_defaults_compose_both_guards() {
        let policy = ConnectionPolicy::new();
        assert_eq!(policy, ConnectionPolicy::default());
        assert_eq!(policy.idle.max_idle_ticks, DEFAULT_MAX_IDLE_TICKS);
        assert_eq!(
            policy.ban.max_consecutive_rejects,
            DEFAULT_MAX_HANDSHAKE_REJECTS
        );
        assert_eq!(policy.ban.ban_duration_ticks, DEFAULT_BAN_DURATION_TICKS);
        assert_eq!(policy.ban.max_tracked_peers, DEFAULT_MAX_TRACKED_PEERS);
        let tracker = IdleTracker::new(policy.idle);
        assert_eq!(tracker.policy(), policy.idle);
        let gate = HandshakeGate::new(policy.ban);
        assert_eq!(gate.policy(), policy.ban);
        assert!(tracker.is_empty());
        assert!(gate.is_empty());
    }
}
