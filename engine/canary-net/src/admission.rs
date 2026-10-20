//! Host-facing wiring for the connection admission and liveness guards.
//!
//! [`crate::policy::ConnectionPolicy`] defines *what* the two guards enforce;
//! this module is *where the server calls them*. It owns the
//! [`HandshakeGate`] and [`IdleTracker`] together with the live
//! `ClientId -> SocketAddr` map so every guard is keyed by the transport
//! peer address — never the log-only [`Hello::client_label`].
//!
//! The host drives one guard per server exactly like its
//! [`SessionTable`](crate::session::SessionTable): both are plain owned
//! state behind `&mut self`, so the compiler enforces exclusive access and
//! there is no lock ordering to deadlock. A host that must share the guard
//! across tasks wraps it in the same lock as the session table and never
//! awaits while holding that lock.
//!
//! Wiring points, in connection lifecycle order:
//!
//! 1. After [`NetTransport::accept_with_peer`] and before reading the hello,
//!    call [`ConnectionGuard::admit`]. On [`PeerAdmission::Banned`], send
//!    [`reject_banned`] and close the connection without running admission.
//! 2. On a rejected hello (decode failure, version/label mismatch, any
//!    [`Reject`]), call
//!    [`ConnectionGuard::note_reject`]. On a welcomed hello, call
//!    [`ConnectionGuard::note_accepted`] with the assigned client id and
//!    mark activity as frames arrive.
//! 3. Once per tick, call [`ConnectionGuard::maintain`]. It expires served
//!    bans and returns the clients past their idle deadline; disconnect each
//!    with [`DisconnectReason::IdleTimeout`](crate::error::DisconnectReason::IdleTimeout)
//!    and call [`ConnectionGuard::disconnect`] next to the session-table
//!    removal.
//!
//! There is deliberately no background task: the clock is caller-supplied
//! (`u64` ticks in one monotonic unit), so tests and hosts stay in control.
//!
//! [`Hello::client_label`]: crate::handshake::Hello::client_label
//! [`NetTransport::accept_with_peer`]: crate::transport::NetTransport::accept_with_peer

use std::collections::HashMap;
use std::net::SocketAddr;

use crate::error::NetError;
use crate::handshake::{encode_reject, Reject, RejectReason};
use crate::ids::{NetSequence, ProtocolVersion};
use crate::limits::NetLimits;
use crate::policy::{ConnectionPolicy, HandshakeGate, IdleTracker};
use crate::session::ClientId;
use crate::transport::NetSend;

/// The accept-path verdict for one inbound peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerAdmission {
    /// The peer may proceed to the hello exchange.
    Allowed,
    /// The peer is under an unexpired temporary ban: send a typed
    /// [`RejectReason::TemporarilyBanned`] refusal and close, without
    /// reading its hello.
    Banned,
}

/// One client past its idle deadline, reported by
/// [`ConnectionGuard::maintain`].
///
/// The peer address is the key the client was admitted under, so the host
/// can log and close the matching transport connection. The guard keeps
/// tracking the client until [`ConnectionGuard::disconnect`] is called:
/// a host that only sweeps sees the same client again next tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleClient {
    /// Connected client to disconnect.
    client: ClientId,
    /// Transport peer address recorded at admission.
    peer: Option<SocketAddr>,
}

impl IdleClient {
    /// The connected client to disconnect.
    #[must_use]
    pub fn client(&self) -> ClientId {
        self.client
    }

    /// The transport peer address recorded at admission.
    #[must_use]
    pub fn peer(&self) -> Option<SocketAddr> {
        self.peer
    }
}

/// Owns the handshake-ban and idle guards for one server and keys them by
/// transport peer address.
///
/// Construct one per server, next to the
/// [`SessionTable`](crate::session::SessionTable), and drive it from the
/// accept, handshake, and tick paths described in the module docs.
#[derive(Debug)]
pub struct ConnectionGuard {
    /// Idle and ban configuration both guards enforce.
    policy: ConnectionPolicy,
    /// Consecutive handshake rejects by peer address.
    bans: HandshakeGate,
    /// Last inbound activity by connected client.
    idle: IdleTracker,
    /// Admission key for each connected client.
    peers: HashMap<ClientId, SocketAddr>,
}

impl ConnectionGuard {
    /// A guard enforcing `policy`.
    #[must_use]
    pub fn new(policy: ConnectionPolicy) -> Self {
        Self {
            policy,
            bans: HandshakeGate::new(policy.ban),
            idle: IdleTracker::new(policy.idle),
            peers: HashMap::new(),
        }
    }

    /// The enforced connection policy.
    #[must_use]
    pub fn policy(&self) -> ConnectionPolicy {
        self.policy
    }

    /// The accept-path check: whether `peer` may proceed to the hello
    /// exchange at `now`.
    ///
    /// Called before acquiring the session table (the guard is separate
    /// owned state), so a refused peer costs no session record. When the
    /// guard and table share a lock, run this check first and release the
    /// lock before the async [`reject_banned`] send — never hold it across
    /// an await.
    pub fn admit(&mut self, peer: SocketAddr, now: u64) -> PeerAdmission {
        if self.is_banned(peer, now) {
            PeerAdmission::Banned
        } else {
            PeerAdmission::Allowed
        }
    }

    /// Whether `peer` is under an unexpired ban at `now`.
    ///
    /// A served ban is forgiven on read, exactly like
    /// [`HandshakeGate::is_banned`].
    pub fn is_banned(&mut self, peer: SocketAddr, now: u64) -> bool {
        self.bans.is_banned(&peer_label(peer), now)
    }

    /// Records a rejected handshake for `peer` at `now`. Returns `true`
    /// when the peer is banned as a result.
    ///
    /// Call this for every refusal on the hello path — decode failure
    /// (including an over-long label), version mismatch, or any other
    /// [`Reject`] — not just version mismatches.
    pub fn note_reject(&mut self, peer: SocketAddr, now: u64) -> bool {
        self.bans.note_reject(&peer_label(peer), now)
    }

    /// Records a welcomed handshake: forgives the peer's reject history,
    /// maps `client` to `peer` for idle eviction, and starts its idle
    /// deadline at `now`.
    pub fn note_accepted(&mut self, peer: SocketAddr, client: ClientId, now: u64) {
        self.bans.note_accepted(&peer_label(peer));
        self.peers.insert(client, peer);
        self.idle.mark_connected(client, now);
    }

    /// Records inbound activity for an admitted `client` at `now`.
    ///
    /// Returns `false` for an unknown client (one that was never accepted
    /// through [`ConnectionGuard::note_accepted`]): the guard never tracks
    /// phantom clients, so a driver bug cannot grow the idle table.
    pub fn note_activity(&mut self, client: ClientId, now: u64) -> bool {
        if !self.peers.contains_key(&client) {
            return false;
        }
        self.idle.note_activity(client, now);
        true
    }

    /// The peer address `client` was admitted under, if still connected.
    #[must_use]
    pub fn peer(&self, client: ClientId) -> Option<SocketAddr> {
        self.peers.get(&client).copied()
    }

    /// Forgets `client`: call next to
    /// [`SessionTable::disconnect`](crate::session::SessionTable::disconnect).
    ///
    /// Returns the peer address it was admitted under. The handshake gate's
    /// reject history is intentionally left alone: a peer that fails its
    /// hello and disconnects keeps accruing toward a ban.
    pub fn disconnect(&mut self, client: ClientId) -> Option<SocketAddr> {
        self.idle.remove(client);
        self.peers.remove(&client)
    }

    /// The tick path: expires served bans and reports every admitted client
    /// past its idle deadline at `now`, in client-id order.
    ///
    /// Disconnect each reported client with
    /// [`DisconnectReason::IdleTimeout`](crate::error::DisconnectReason::IdleTimeout)
    /// and call [`ConnectionGuard::disconnect`] beside the session-table
    /// removal. Clients stay tracked until then, so a missed disconnect is
    /// reported again on the next tick. Hosts with no tick loop must call
    /// this from their own periodic maintenance; this crate spawns nothing.
    pub fn maintain(&mut self, now: u64) -> Vec<IdleClient> {
        self.bans.evict_expired(now);
        self.idle
            .sweep_idle(now)
            .into_iter()
            .map(|client| IdleClient {
                client,
                peer: self.peers.get(&client).copied(),
            })
            .collect()
    }

    /// How many clients the guard currently tracks.
    #[must_use]
    pub fn tracked_clients(&self) -> usize {
        self.peers.len()
    }
}

impl Default for ConnectionGuard {
    /// [`ConnectionGuard::new`] with [`ConnectionPolicy::default`].
    fn default() -> Self {
        Self::new(ConnectionPolicy::default())
    }
}

/// Sends the typed temporary-ban refusal on a freshly accepted stream and
/// closes it.
///
/// Call when [`ConnectionGuard::admit`] returns [`PeerAdmission::Banned`]:
/// the host supplies the next outbound envelope `sequence` and the
/// `supported_protocol` echoed in the [`Reject`]. A send failure is local to
/// that peer; the accept loop should log it and continue.
pub async fn reject_banned<S: NetSend>(
    send: &mut S,
    sequence: NetSequence,
    supported_protocol: ProtocolVersion,
    limits: &NetLimits,
) -> Result<(), NetError> {
    let reject = Reject {
        reason: RejectReason::TemporarilyBanned,
        supported_protocol,
    };
    let body = encode_reject(&reject, limits)?;
    let envelope =
        crate::envelope::NetEnvelope::seal(supported_protocol, sequence, body).encode(limits)?;
    send.send_frame(&envelope, limits).await?;
    send.finish().await
}

/// Canonical ban key: the transport peer address, never the client label.
fn peer_label(peer: SocketAddr) -> String {
    peer.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::NetEnvelope;
    use crate::handshake::{decode_reject, RejectReason};
    use crate::ids::PROTOCOL_VERSION_1;
    use crate::loopback::LoopbackTransport;
    use crate::policy::{BanPolicy, IdlePolicy};
    use crate::transport::{NetRecv, NetTransport};

    fn peer(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn guard(rejects: u32, ban_ticks: u64, idle_ticks: u64) -> ConnectionGuard {
        ConnectionGuard::new(ConnectionPolicy {
            idle: IdlePolicy::new(idle_ticks),
            ban: BanPolicy::new(rejects, ban_ticks, 16),
        })
    }

    #[test]
    fn banned_peer_is_refused_at_accept() {
        let mut guard = guard(2, 100, 10);
        let banned = peer(40_001);
        assert_eq!(guard.admit(banned, 0), PeerAdmission::Allowed);
        assert!(!guard.note_reject(banned, 0));
        // One reject is below the threshold: admission still proceeds.
        assert_eq!(guard.admit(banned, 0), PeerAdmission::Allowed);
        assert!(guard.note_reject(banned, 1));
        assert_eq!(guard.admit(banned, 1), PeerAdmission::Banned);
        // Banning is per peer address: another connection is unaffected.
        assert_eq!(guard.admit(peer(40_002), 1), PeerAdmission::Allowed);
        // The served ban expires on the injected clock.
        assert_eq!(guard.admit(banned, 101), PeerAdmission::Allowed);
    }

    #[test]
    fn configured_reject_count_triggers_the_ban() {
        let mut guard = guard(3, 100, 10);
        let flooder = peer(40_010);
        assert!(!guard.note_reject(flooder, 0));
        assert!(!guard.note_reject(flooder, 1));
        assert!(!guard.is_banned(flooder, 2));
        assert!(guard.note_reject(flooder, 2));
        assert!(guard.is_banned(flooder, 2));
        // The ban lasts exactly the configured duration from the reject.
        assert!(guard.is_banned(flooder, 101));
        assert!(!guard.is_banned(flooder, 102));
    }

    #[test]
    fn accepted_handshake_clears_reject_history() {
        let mut guard = guard(2, 100, 10);
        let flaky = peer(40_020);
        assert!(!guard.note_reject(flaky, 0));
        guard.note_accepted(flaky, ClientId(1), 0);
        assert!(!guard.is_banned(flaky, 0));
        // The history restarted clean: a single new reject does not ban.
        assert!(!guard.note_reject(flaky, 1));
        assert!(!guard.is_banned(flaky, 1));
        assert_eq!(guard.peer(ClientId(1)), Some(flaky));
    }

    #[test]
    fn maintain_evicts_idle_clients_and_keeps_active_ones() {
        let mut guard = guard(3, 100, 10);
        let silent_peer = peer(40_030);
        let active_peer = peer(40_031);
        guard.note_accepted(silent_peer, ClientId(1), 0);
        guard.note_accepted(active_peer, ClientId(2), 0);
        // The active client keeps heartbeating; the silent one does not.
        assert!(guard.note_activity(ClientId(2), 9));
        // Exactly at the deadline is not yet idle (strictly greater).
        assert!(guard.maintain(10).is_empty());
        let idle = guard.maintain(11);
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].client(), ClientId(1));
        assert_eq!(idle[0].peer(), Some(silent_peer));
        // Sweeping alone does not forget: the same client is reported until
        // the host disconnects it.
        assert_eq!(guard.maintain(11).len(), 1);
        assert_eq!(guard.disconnect(ClientId(1)), Some(silent_peer));
        assert!(guard.maintain(11).is_empty());
        // The active client's deadline moved with its heartbeat.
        assert!(guard.maintain(19).is_empty());
        assert_eq!(guard.maintain(20)[0].client(), ClientId(2));
        // Unknown clients are never tracked as phantoms.
        assert!(!guard.note_activity(ClientId(99), 20));
        assert_eq!(guard.tracked_clients(), 1);
    }

    #[test]
    fn maintain_expires_served_bans() {
        let mut guard = guard(1, 10, 10);
        let flooder = peer(40_040);
        assert!(guard.note_reject(flooder, 0));
        assert!(guard.is_banned(flooder, 9));
        assert!(guard.maintain(10).is_empty());
        assert!(!guard.is_banned(flooder, 10));
        assert_eq!(guard.admit(flooder, 10), PeerAdmission::Allowed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn banned_peer_receives_a_typed_refusal_and_closed_stream() {
        let limits = NetLimits::default();
        let server = LoopbackTransport::bind().expect("bind loopback listener");
        let server_addr = server.local_addr().expect("server address");
        let client = LoopbackTransport::ephemeral();
        let client_addr = client.local_addr().expect("client address");
        let (mut client_send, mut client_recv) = client
            .connect(server_addr, "loopback")
            .await
            .expect("loopback dial");
        let (admitted_peer, mut server_send, _server_recv) =
            server.accept_with_peer().await.expect("accept with peer");
        // The ban key is the transport peer address, not the client label.
        assert_eq!(admitted_peer, client_addr);

        let mut guard = guard(1, 100, 10);
        assert!(guard.note_reject(admitted_peer, 0));
        assert_eq!(guard.admit(admitted_peer, 0), PeerAdmission::Banned);
        reject_banned(
            &mut server_send,
            NetSequence(1),
            PROTOCOL_VERSION_1,
            &limits,
        )
        .await
        .expect("ban refusal sends");

        let body = client_recv.recv_frame(&limits).await.expect("refusal recv");
        let envelope = NetEnvelope::decode(&body).expect("envelope verifies");
        let reject = decode_reject(&envelope.payload).expect("reject decodes");
        assert_eq!(reject.reason, RejectReason::TemporarilyBanned);
        assert_eq!(reject.supported_protocol, PROTOCOL_VERSION_1);
        assert!(matches!(
            client_recv.recv_frame(&limits).await,
            Err(NetError::PeerClosed)
        ));
        client_send.finish().await.ok();
    }
}
