//! In-process loopback transport and deterministic fault injection (WP4).
//!
//! [`LoopbackTransport`] implements [`NetTransport`](crate::transport::NetTransport)
//! over in-process channels: same trait, same framing gates
//! ([`encode_frame`](crate::frame::encode_frame) on send,
//! [`decode_frame_len`](crate::frame::decode_frame_len) on receive), real
//! connect/accept rendezvous through a registry — but no UDP sockets, no
//! TLS, no pinning. That path stays covered by the Quinn tests in
//! [`crate::transport`] and `tests/session_roundtrip.rs`; this module
//! exists so resync and fault-matrix tests run deterministically without
//! wall-clock timing or process harnesses.
//!
//! What loopback does and does not prove, stated plainly:
//!
//! - It proves the layers where faults actually surface: envelope
//!   checksums, [`SequenceGate`](crate::sequence::SequenceGate) replay
//!   rejection, delta base validation with gap→resync, tombstone replay,
//!   and baseline planning. A dropped frame here is a sequence gap there,
//!   exactly as on a real stream.
//! - It does *not* prove QUIC/TLS behavior (loss recovery, congestion,
//!   handshake crypto): the injected faults model what the session layer
//!   observes *above* a reliable stream when messages go missing, repeat,
//!   or arrive out of order across reconnects and lanes — the contract the
//!   session layer owns.
//!
//! [`FaultProfile`] + [`FaultySend`] wrap any
//! [`NetSend`](crate::transport::NetSend) with deterministic, counter-based
//! faults: every n-th frame drops, duplicates, or inverts order with its
//! successor (hold-one). No RNG, no wall clock (except the latency cell's
//! fixed sleep, asserted with generous margins): every matrix cell is
//! reproducible frame-for-frame, and each fault cell asserts the fault
//! actually fired (a gap, a duplicate rejection, a reorder rejection) so a
//! silently-clean run cannot pass as fault tolerance.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::error::NetError;
use crate::frame::{decode_frame_len, encode_frame, FRAME_HEADER_LEN};
use crate::limits::NetLimits;
use crate::transport::{NetRecv, NetSend, NetTransport};

/// One in-flight frame on a loopback connection (already length-prefixed).
type WireFrame = Vec<u8>;

/// Server halves waiting on a bound listener for the next `accept`.
struct PendingSocket {
    /// Server's sending half (server→client direction).
    send: LoopbackSend,
    /// Server's receiving half (client→server direction).
    recv: LoopbackRecv,
}

/// Bound loopback listeners by address. Guarded by a plain mutex held only
/// for map operations, never across `.await`.
fn listeners(
) -> &'static std::sync::Mutex<HashMap<SocketAddr, mpsc::UnboundedSender<PendingSocket>>> {
    static LISTENERS: OnceLock<
        std::sync::Mutex<HashMap<SocketAddr, mpsc::UnboundedSender<PendingSocket>>>,
    > = OnceLock::new();
    LISTENERS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Loopback port allocator. Loopback addresses never touch the network, so
/// colliding with a real listener is impossible; colliding with another
/// loopback listener is prevented by the atomic counter (20_000 bindings
/// per process before wrap — tests bind a handful).
static NEXT_PORT: AtomicU32 = AtomicU32::new(0);

/// A fresh 127.0.0.1 address with an atomically allocated port.
fn next_addr() -> SocketAddr {
    let claimed = NEXT_PORT.fetch_add(1, Ordering::SeqCst);
    let port = u16::try_from(41_000 + (claimed % 20_000)).unwrap_or(41_000);
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Sending half of one loopback stream: framed bytes into an unbounded
/// channel. Unbounded because bounds belong to the session queues
/// ([`crate::queue`]), not the transport — the transport never invents
/// backpressure policy.
#[derive(Debug)]
pub struct LoopbackSend {
    /// Outbound frames, `None` after [`NetSend::finish`].
    tx: Option<mpsc::UnboundedSender<WireFrame>>,
}

/// Receiving half of one loopback stream.
#[derive(Debug)]
pub struct LoopbackRecv {
    /// Inbound frames; closed when the peer's send half is finished.
    rx: mpsc::UnboundedReceiver<WireFrame>,
}

impl NetSend for LoopbackSend {
    fn send_frame(
        &mut self,
        body: &[u8],
        limits: &NetLimits,
    ) -> impl std::future::Future<Output = Result<(), NetError>> + Send + '_ {
        // Framing and the limit gate run eagerly at call time (mirroring
        // the Quinn adapter), so the future borrows only `self`.
        let framed = encode_frame(body, limits);
        async move {
            let frame = framed?;
            let Some(tx) = self.tx.as_ref() else {
                return Err(NetError::PeerClosed);
            };
            tx.send(frame).map_err(|_| NetError::PeerClosed)?;
            Ok(())
        }
    }

    async fn finish(&mut self) -> Result<(), NetError> {
        // Dropping the sender closes the receiver: the peer's next read
        // resolves as `PeerClosed`, the loopback analogue of QUIC's
        // send-finish EOF.
        self.tx = None;
        Ok(())
    }
}

impl NetRecv for LoopbackRecv {
    fn recv_frame(
        &mut self,
        limits: &NetLimits,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, NetError>> + Send + '_ {
        let limits = *limits;
        async move {
            let frame = self.rx.recv().await.ok_or(NetError::PeerClosed)?;
            if frame.len() < FRAME_HEADER_LEN {
                return Err(NetError::TruncatedFrame {
                    claimed: FRAME_HEADER_LEN,
                    received: frame.len(),
                });
            }
            let mut header = [0u8; FRAME_HEADER_LEN];
            header.copy_from_slice(&frame[..FRAME_HEADER_LEN]);
            // The gate runs on the bare prefix before the body is touched —
            // the same order the Quinn adapter enforces on socket bytes.
            let len = decode_frame_len(header, &limits)?;
            let body = &frame[FRAME_HEADER_LEN..];
            if body.len() != len {
                return Err(NetError::TruncatedFrame {
                    claimed: len,
                    received: body.len(),
                });
            }
            Ok(body.to_vec())
        }
    }
}

/// In-process [`NetTransport`](crate::transport::NetTransport): real
/// connect/accept rendezvous, real framing gates, no sockets or TLS.
///
/// `bind` registers a listener (accept side);
/// [`LoopbackTransport::ephemeral`] dials without listening (client side).
/// The `server_name` SNI argument to `connect` is accepted and ignored:
/// loopback performs no TLS verification — pinning stays covered by the
/// Quinn tests. Dropping a bound transport unregisters its listener, so
/// later dials fail as connect errors instead of hanging.
#[derive(Debug)]
pub struct LoopbackTransport {
    /// Address this endpoint is known by in the listener registry.
    addr: SocketAddr,
    /// Inbound pending sockets while bound; `None` for ephemeral endpoints.
    /// A Tokio mutex because `accept` holds it across the queue wait.
    incoming: Option<tokio::sync::Mutex<mpsc::UnboundedReceiver<PendingSocket>>>,
}

impl LoopbackTransport {
    /// Binds a listener on a fresh loopback address.
    pub fn bind() -> Result<Self, NetError> {
        let addr = next_addr();
        let (tx, rx) = mpsc::unbounded_channel();
        listeners()
            .lock()
            .map_err(|_| NetError::Transport("loopback listener registry is poisoned".to_string()))?
            .insert(addr, tx);
        Ok(Self {
            addr,
            incoming: Some(tokio::sync::Mutex::new(rx)),
        })
    }

    /// A dial-only endpoint with no listener. `accept` on it always fails
    /// as [`NetError::TransportAccept`].
    #[must_use]
    pub fn ephemeral() -> Self {
        Self {
            addr: next_addr(),
            incoming: None,
        }
    }
}

impl Drop for LoopbackTransport {
    /// Unregisters the listener so later dials fail fast instead of hanging
    /// on a dead address. Lock failure during teardown is ignored: there
    /// is no caller to report to, and a poisoned registry already fails
    /// every operation loudly.
    fn drop(&mut self) {
        if self.incoming.is_some() {
            if let Ok(mut table) = listeners().lock() {
                table.remove(&self.addr);
            }
        }
    }
}

impl NetTransport for LoopbackTransport {
    type Send = LoopbackSend;
    type Recv = LoopbackRecv;

    fn connect(
        &self,
        addr: SocketAddr,
        _server_name: &str,
    ) -> impl std::future::Future<Output = Result<(Self::Send, Self::Recv), NetError>> + Send + '_
    {
        // The listener handle is resolved synchronously (no awaits — the
        // registry lock never crosses one); the future below owns it, so it
        // borrows only `self`, exactly like the Quinn adapter's connect,
        // whose SNI string is likewise copied for the future to own.
        let listener = listeners()
            .lock()
            .map_err(|_| NetError::Transport("loopback listener registry is poisoned".to_string()))
            .and_then(|table| {
                table
                    .get(&addr)
                    .cloned()
                    .ok_or_else(|| NetError::TransportConnect {
                        detail: format!("no loopback listener on {addr}"),
                    })
            });
        async move {
            let listener = listener?;
            let (client_tx, server_rx) = mpsc::unbounded_channel();
            let (server_tx, client_rx) = mpsc::unbounded_channel();
            listener
                .send(PendingSocket {
                    send: LoopbackSend {
                        tx: Some(server_tx),
                    },
                    recv: LoopbackRecv { rx: server_rx },
                })
                .map_err(|_| NetError::TransportConnect {
                    detail: format!("loopback listener on {addr} is gone"),
                })?;
            Ok((
                LoopbackSend {
                    tx: Some(client_tx),
                },
                LoopbackRecv { rx: client_rx },
            ))
        }
    }

    async fn accept(&self) -> Result<(Self::Send, Self::Recv), NetError> {
        let Some(incoming) = self.incoming.as_ref() else {
            return Err(NetError::TransportAccept {
                detail: "loopback endpoint is not bound".to_string(),
            });
        };
        let mut queue = incoming.lock().await;
        let pending = queue
            .recv()
            .await
            .ok_or_else(|| NetError::TransportAccept {
                detail: "loopback listener closed".to_string(),
            })?;
        Ok((pending.send, pending.recv))
    }

    fn local_addr(&self) -> Result<SocketAddr, NetError> {
        Ok(self.addr)
    }
}

/// Deterministic fault profile for [`FaultySend`].
///
/// Every field is a counter rule, not a probability: `drop_every: 4` drops
/// the 4th, 8th, 12th, … frame sent. Zero disables that fault. Counter
/// rules reproduce exactly — same profile, same dropped/duplicated/
/// reordered frames — so matrix cells never flake and never need seed
/// management.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaultProfile {
    /// Drops every n-th outbound frame (the receiver observes a sequence
    /// gap and must gap→resync; the sender still reports success, modeling
    /// loss *above* the reliable stream — the session layer's contract).
    pub drop_every: u64,
    /// Sends every n-th outbound frame twice (the receiver's sequence gate
    /// must reject the copy, never apply twice).
    pub duplicate_every: u64,
    /// Holds every n-th outbound frame and emits it after its successor
    /// (one-frame inversion: successor then held frame — the receiver must
    /// reject the late frame, never reorder application).
    pub reorder_every: u64,
    /// Sleeps this long before every send, drops and holds included.
    /// Fixed sleeps only; tests assert logical convergence first and use
    /// generous margins around timing, never exact deadlines.
    pub latency: Option<Duration>,
}

impl FaultProfile {
    /// No faults: every frame passes through untouched.
    #[must_use]
    pub fn clean() -> Self {
        Self {
            drop_every: 0,
            duplicate_every: 0,
            reorder_every: 0,
            latency: None,
        }
    }

    /// Whether every fault is disabled.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        *self == Self::clean()
    }
}

impl Default for FaultProfile {
    /// [`FaultProfile::clean`].
    fn default() -> Self {
        Self::clean()
    }
}

/// [`NetSend`](crate::transport::NetSend) wrapper injecting a
/// [`FaultProfile`]'s deterministic faults into one direction.
///
/// Ordering faults compose per frame in a fixed order — drop, then reorder
/// hold, then send (+ flush of the previously held frame), then duplicate —
/// so a profile enabling several faults stays reproducible. A frame held
/// for reorder is flushed by the next send, or by `finish` (which replays
/// it under the constructor `limits` before closing): no frame is ever
/// silently kept past stream end.
pub struct FaultySend<S> {
    /// Wrapped reliable sender; the faults model unreliability above it.
    inner: S,
    /// Which faults to inject.
    profile: FaultProfile,
    /// Limits for replaying a held frame in `finish`.
    limits: NetLimits,
    /// Frames offered so far (1-based frame index for the counter rules).
    sent: u64,
    /// Frame held for reorder inversion, awaiting its successor.
    held: Option<Vec<u8>>,
}

impl<S> FaultySend<S> {
    /// Wraps `inner` with `profile`; `limits` replays a held frame at
    /// `finish` time.
    pub fn new(inner: S, profile: FaultProfile, limits: NetLimits) -> Self {
        Self {
            inner,
            profile,
            limits,
            sent: 0,
            held: None,
        }
    }

    /// The wrapped sender. Drains nothing: a held frame stays held until
    /// the next `send_frame` or `finish`.
    pub fn inner(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Unwraps the sender, discarding an unflushed held frame. Prefer
    /// `finish` (which replays it) unless the test intentionally drops the
    /// tail — in which case the receiver's gap→resync is the assertion.
    #[must_use]
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: NetSend> NetSend for FaultySend<S> {
    fn send_frame(
        &mut self,
        body: &[u8],
        limits: &NetLimits,
    ) -> impl std::future::Future<Output = Result<(), NetError>> + Send + '_ {
        // The trait future borrows only `self`: body and limits are copied
        // eagerly (mirroring the Quinn adapter), so the async block below
        // owns everything it touches besides `self`.
        let body = body.to_vec();
        let limits = *limits;
        async move {
            if let Some(latency) = self.profile.latency {
                tokio::time::sleep(latency).await;
            }
            self.sent = self.sent.saturating_add(1);
            let index = self.sent;
            if self.profile.drop_every > 0 && index % self.profile.drop_every == 0 {
                return Ok(());
            }
            if self.profile.reorder_every > 0 && index % self.profile.reorder_every == 0 {
                let previous = self.held.replace(body);
                if let Some(previous) = previous {
                    // Back-to-back triggers: the older held frame can wait
                    // no longer — emit it in order first, hold the new one.
                    self.inner.send_frame(&previous, &limits).await?;
                }
                return Ok(());
            }
            self.inner.send_frame(&body, &limits).await?;
            // The held frame goes second: successor then held is the
            // inversion the receiver's gate must refuse to un-apply.
            if let Some(held) = self.held.take() {
                self.inner.send_frame(&held, &limits).await?;
            }
            if self.profile.duplicate_every > 0 && index % self.profile.duplicate_every == 0 {
                self.inner.send_frame(&body, &limits).await?;
            }
            Ok(())
        }
    }

    async fn finish(&mut self) -> Result<(), NetError> {
        if let Some(held) = self.held.take() {
            self.inner.send_frame(&held, &self.limits).await?;
        }
        self.inner.finish().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    use crate::envelope::NetEnvelope;
    use crate::handshake::{decide_handshake, decode_hello, decode_welcome, encode_hello};
    use crate::ids::{NetEntityId, NetSequence, SimTick, PROTOCOL_VERSION_1};
    use crate::input::InputValidator;
    use crate::replication::{decode_delta, decode_snapshot, encode_delta, encode_snapshot};
    use crate::resync::{BaselineRetention, ResyncPlan};
    use crate::sequence::SequenceGate;
    use crate::session::{decode_ack, ClientAck, ClientId, ClientSession, SessionTable};
    use crate::tombstone::{Tombstone, TombstoneLog};

    /// Action schema the resync proof accepts input for (mirrors the
    /// separate-process proof's vocabulary).
    const ACTION_MOVE: &str = "canary.input/move@1";
    /// Schema manifest both ends speak.
    const PROOF_MANIFEST: crate::ids::SchemaManifestVersion = crate::ids::SchemaManifestVersion(3);
    /// Game release both ends run.
    const PROOF_GAME: crate::ids::GameVersion = crate::ids::GameVersion(11);
    /// Plugin/API surface both ends support.
    const PROOF_PLUGIN_API: crate::ids::PluginApiVersion = crate::ids::PluginApiVersion(4);

    /// Seals `payload` at `sequence` and sends it on `send`.
    async fn send_envelope<S: NetSend>(
        send: &mut S,
        sequence: NetSequence,
        payload: Vec<u8>,
        limits: &NetLimits,
    ) -> Result<(), NetError> {
        let body = NetEnvelope::seal(PROTOCOL_VERSION_1, sequence, payload).encode(limits)?;
        send.send_frame(&body, limits).await
    }

    /// Receives one envelope, checks the protocol version and the inbound
    /// sequence gate. Version and replay failures are typed errors, never
    /// silent acceptance.
    async fn recv_envelope<R: NetRecv>(
        recv: &mut R,
        gate: &mut SequenceGate,
        limits: &NetLimits,
    ) -> Result<NetEnvelope, NetError> {
        let body = recv.recv_frame(limits).await?;
        let envelope = NetEnvelope::decode(&body)?;
        if envelope.protocol_version != PROTOCOL_VERSION_1 {
            return Err(NetError::UnsupportedProtocol {
                got: envelope.protocol_version.0,
                supported: PROTOCOL_VERSION_1.0,
            });
        }
        gate.check(envelope.sequence)?;
        Ok(envelope)
    }

    /// Connects a client to a bound server: dial first (the pending socket
    /// queues — no rendezvous needed), then accept.
    async fn connect_pair(
        server: &LoopbackTransport,
    ) -> Result<(LoopbackSend, LoopbackRecv, LoopbackSend, LoopbackRecv), NetError> {
        let client_endpoint = LoopbackTransport::ephemeral();
        let client_addr = client_endpoint.local_addr()?;
        let server_addr = server.local_addr()?;
        let (client_send, client_recv) = client_endpoint.connect(server_addr, "loopback").await?;
        let (server_send, server_recv) = server.accept().await?;
        // The addresses are real registry entries (used for the dial), and
        // endpoints stay alive for the session: dropping the client
        // endpoint must not disturb the established halves.
        assert_ne!(client_addr, server_addr);
        Ok((client_send, client_recv, server_send, server_recv))
    }

    /// Authoritative entries for a state map (one schema, hp payloads).
    fn entries_of(
        state: &HashMap<(u64, String), Vec<u8>>,
    ) -> Vec<crate::replication::ReplicatedEntry> {
        state
            .iter()
            .map(
                |((entity, schema), payload)| crate::replication::ReplicatedEntry {
                    entity: NetEntityId(*entity),
                    schema: schema.clone(),
                    payload: payload.clone(),
                },
            )
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loopback_connect_accept_and_envelope_round_trip() {
        let limits = NetLimits::default();
        let server = LoopbackTransport::bind().expect("bind loopback listener");
        let (mut client_send, mut client_recv, mut server_send, mut server_recv) =
            connect_pair(&server).await.expect("loopback dial");

        let mut client_gate = SequenceGate::new();
        let mut server_gate = SequenceGate::new();
        send_envelope(
            &mut client_send,
            NetSequence(1),
            b"client input".to_vec(),
            &limits,
        )
        .await
        .expect("client send");
        let inbound = recv_envelope(&mut server_recv, &mut server_gate, &limits)
            .await
            .expect("server receive");
        assert_eq!(inbound.payload, b"client input");

        send_envelope(
            &mut server_send,
            NetSequence(1),
            b"authoritative delta".to_vec(),
            &limits,
        )
        .await
        .expect("server reply");
        let inbound = recv_envelope(&mut client_recv, &mut client_gate, &limits)
            .await
            .expect("client receive");
        assert_eq!(inbound.payload, b"authoritative delta");

        // Finish closes the receiver: the next read is a clean peer-close,
        // not a hang and not a truncation.
        client_send.finish().await.expect("client finish");
        assert!(matches!(
            server_recv.recv_frame(&limits).await,
            Err(NetError::PeerClosed)
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loopback_dial_to_dead_address_fails_fast() {
        let client = LoopbackTransport::ephemeral();
        let dead = SocketAddr::from(([127, 0, 0, 1], 9));
        let outcome = client.connect(dead, "loopback").await;
        assert!(
            matches!(outcome, Err(NetError::TransportConnect { .. })),
            "dial to nothing must fail fast, got {outcome:?}"
        );
        // So does accept on an endpoint that never bound.
        assert!(matches!(
            client.accept().await,
            Err(NetError::TransportAccept { .. })
        ));
        // And dropping the listener unregisters it: dials fail, not hang.
        let server = LoopbackTransport::bind().expect("bind");
        let addr = server.local_addr().expect("address");
        drop(server);
        assert!(matches!(
            client.connect(addr, "loopback").await,
            Err(NetError::TransportConnect { .. })
        ));
    }

    /// Reconnect/resync end-to-end over loopback: the client converges
    /// through snapshot → delta, drops mid-session, reconnects as a *new*
    /// session, resynchronizes incrementally from its last-acked tick
    /// (shared baseline + replayed tombstones, no full transfer), and
    /// keeps converging on live deltas after.
    #[tokio::test(flavor = "multi_thread")]
    async fn reconnect_resync_converges_after_mid_session_drop() {
        let limits = NetLimits::default();
        let server_endpoint = LoopbackTransport::bind().expect("bind");
        let (mut client_send, mut client_recv, mut server_send, mut server_recv) =
            connect_pair(&server_endpoint).await.expect("first dial");

        let mut table = SessionTable::new();
        let mut tombstones = TombstoneLog::new(64);
        let mut retention = BaselineRetention::new(8);
        let client = ClientId(1);
        let (send_caps, recv_caps) = (
            ClientSession::caps_from_limits(&limits),
            ClientSession::caps_from_limits(&limits),
        );
        let validator = || InputValidator::new(7, &[ACTION_MOVE], 64, 4, 8);

        // --- Session 1 (session id 100): handshake, snapshot, delta. ---
        let mut server_gate = SequenceGate::new();
        let mut client_gate = SequenceGate::new();
        let hello = crate::handshake::Hello {
            protocol_version: PROTOCOL_VERSION_1,
            schema_manifest: PROOF_MANIFEST,
            game_version: PROOF_GAME,
            plugin_api_version: PROOF_PLUGIN_API,
            client_label: "resync-proof".to_string(),
        };
        send_envelope(
            &mut client_send,
            NetSequence(1),
            encode_hello(&hello, &limits).expect("hello"),
            &limits,
        )
        .await
        .expect("hello send");
        let hello_body = recv_envelope(&mut server_recv, &mut server_gate, &limits)
            .await
            .expect("hello recv");
        let hello = decode_hello(&hello_body.payload).expect("hello decode");
        let welcome = decide_handshake(
            &hello,
            PROTOCOL_VERSION_1,
            PROOF_MANIFEST,
            PROOF_GAME,
            PROOF_PLUGIN_API,
            7,
            100,
        )
        .expect("compatible hello");
        table.connect(
            client,
            welcome.assigned_slot,
            welcome.session_id,
            send_caps,
            recv_caps,
            validator(),
        );
        let mut next_outbound = 1u64;
        let mut outbound_seq = || {
            let sequence = NetSequence(next_outbound);
            next_outbound += 1;
            sequence
        };
        send_envelope(
            &mut server_send,
            outbound_seq(),
            crate::handshake::encode_welcome(&welcome, &limits).expect("welcome"),
            &limits,
        )
        .await
        .expect("welcome send");
        let welcome_body = recv_envelope(&mut client_recv, &mut client_gate, &limits)
            .await
            .expect("welcome recv");
        let welcome = decode_welcome(&welcome_body.payload).expect("welcome decode");
        assert_eq!(welcome.session_id, 100);

        // Authoritative tick 41: two entities alive.
        let mut authoritative: HashMap<(u64, String), Vec<u8>> = HashMap::from([
            ((2, "canary.health".to_string()), b"hp:7".to_vec()),
            ((9, "canary.health".to_string()), b"hp:10".to_vec()),
        ]);
        retention.push(SimTick(41), entries_of(&authoritative));
        let snapshot_seq = outbound_seq();
        send_envelope(
            &mut server_send,
            snapshot_seq,
            encode_snapshot(entries_of(&authoritative), SimTick(41), &limits).expect("snapshot"),
            &limits,
        )
        .await
        .expect("snapshot send");

        // Client converges to the snapshot.
        let snapshot_body = recv_envelope(&mut client_recv, &mut client_gate, &limits)
            .await
            .expect("snapshot recv");
        assert_eq!(snapshot_body.sequence, snapshot_seq);
        let snapshot = decode_snapshot(&snapshot_body.payload).expect("snapshot decode");
        assert_eq!(snapshot.envelope.sim_tick, SimTick(41));
        let mut local: HashMap<(u64, String), Vec<u8>> = snapshot
            .entries
            .iter()
            .map(|entry| {
                (
                    (entry.entity.0, entry.schema.clone()),
                    entry.payload.clone(),
                )
            })
            .collect();
        assert_eq!(local, authoritative);

        // Authoritative tick 42: entity 9 changes, entity 2 is removed.
        authoritative.insert((9, "canary.health".to_string()), b"hp:9".to_vec());
        authoritative.remove(&(2, "canary.health".to_string()));
        tombstones.record(Tombstone::component_removed(
            NetEntityId(2),
            "canary.health",
            42,
        ));
        retention.push(SimTick(42), entries_of(&authoritative));
        let delta_seq = outbound_seq();
        let pending: Vec<Tombstone> = tombstones
            .pending_since(0)
            .expect("cursor retained")
            .into_iter()
            .map(|logged| logged.tombstone.clone())
            .collect();
        send_envelope(
            &mut server_send,
            delta_seq,
            encode_delta(
                delta_seq,
                snapshot_seq,
                SimTick(42),
                vec![crate::replication::ReplicatedEntry {
                    entity: NetEntityId(9),
                    schema: "canary.health".to_string(),
                    payload: b"hp:9".to_vec(),
                }],
                pending,
                &limits,
            )
            .expect("delta"),
            &limits,
        )
        .await
        .expect("delta send");
        let delta_body = recv_envelope(&mut client_recv, &mut client_gate, &limits)
            .await
            .expect("delta recv");
        let delta = decode_delta(&delta_body.payload).expect("delta decode");
        delta.validate_all(snapshot_seq).expect("delta base");
        for entry in &delta.changes {
            local.insert(
                (entry.entity.0, entry.schema.clone()),
                entry.payload.clone(),
            );
        }
        for tombstone in &delta.removals {
            if let Some(schema) = tombstone.schema.clone() {
                local.remove(&(tombstone.entity.0, schema));
            }
        }
        assert_eq!(local, authoritative);
        // The client's resync cursor after converging to tick 42: it holds
        // tombstone log sequence 1 next (applied log 0).
        let last_acked_tick = SimTick(42);
        let tombstone_cursor = 1u64;

        // --- Mid-session drop: the client vanishes without goodbye. ---
        drop(client_send);
        drop(client_recv);
        table.disconnect(client);
        assert!(table.is_empty());

        // The server advances alone to tick 43: entity 5 spawns, entity 9
        // is removed. Retention and the tombstone log outlive the session.
        authoritative.insert((5, "canary.health".to_string()), b"hp:5".to_vec());
        authoritative.remove(&(9, "canary.health".to_string()));
        tombstones.record(Tombstone::component_removed(
            NetEntityId(9),
            "canary.health",
            43,
        ));
        retention.push(SimTick(43), entries_of(&authoritative));

        // --- Session 2 (session id 101): reconnect + incremental resync. ---
        let (mut client_send, mut client_recv, mut server_send2, mut server_recv2) =
            connect_pair(&server_endpoint).await.expect("second dial");
        let mut server_gate2 = SequenceGate::new();
        let mut client_gate2 = SequenceGate::new();
        let mut next_outbound2 = 1u64;
        let mut outbound_seq2 = || {
            let sequence = NetSequence(next_outbound2);
            next_outbound2 += 1;
            sequence
        };
        // New session, new id, fresh record: no residue from session 1.
        table.connect(client, 7, 101, send_caps, recv_caps, validator());
        let fresh = table.get(client).expect("reconnected");
        assert_eq!(fresh.session_id(), 101);
        assert_eq!(fresh.last_acked_tick(), SimTick(0));

        // The client requests resync from its last-acked tick and cursor.
        // `last_applied` rides along informationally (old-session wire
        // sequences are meaningless in the new session); the server plans
        // from the tick and cursor only.
        let request = ClientAck {
            last_applied: delta_seq,
            tombstone_cursor,
            last_acked_tick,
        };
        let request_bytes = crate::session::encode_ack(&request, &limits).expect("resync request");
        send_envelope(&mut client_send, NetSequence(1), request_bytes, &limits)
            .await
            .expect("request send");
        let request_body = recv_envelope(&mut server_recv2, &mut server_gate2, &limits)
            .await
            .expect("request recv");
        let request = decode_ack(&request_body.payload).expect("request decode");
        let plan = retention.plan(
            request.last_acked_tick,
            request.tombstone_cursor,
            &tombstones,
        );
        let ResyncPlan::Covered {
            tick,
            entries,
            tombstones: replay,
        } = plan
        else {
            panic!("tick 42 baseline and cursor 1 are retained: must be covered");
        };
        assert_eq!(tick, SimTick(42));
        // The shared baseline is exactly what the client already holds
        // (incremental: no full transfer needed)...
        let baseline: HashMap<(u64, String), Vec<u8>> = entries
            .iter()
            .map(|entry| {
                (
                    (entry.entity.0, entry.schema.clone()),
                    entry.payload.clone(),
                )
            })
            .collect();
        assert_eq!(baseline, local);
        // ...and the replay carries exactly the removals since its cursor.
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].entity, NetEntityId(9));

        // The server streams the baseline snapshot plus a catch-up delta
        // carrying the replayed tombstones, on fresh session-2 sequences.
        let resync_snapshot_seq = outbound_seq2();
        send_envelope(
            &mut server_send2,
            resync_snapshot_seq,
            encode_snapshot(entries, tick, &limits).expect("resync snapshot"),
            &limits,
        )
        .await
        .expect("resync snapshot send");
        let catchup_seq = outbound_seq2();
        send_envelope(
            &mut server_send2,
            catchup_seq,
            encode_delta(
                catchup_seq,
                resync_snapshot_seq,
                SimTick(43),
                Vec::new(),
                replay,
                &limits,
            )
            .expect("catch-up delta"),
            &limits,
        )
        .await
        .expect("catch-up send");

        // The client adopts the baseline (identical to its state — the
        // proof it shared history) and applies the catch-up removals.
        let snapshot_body = recv_envelope(&mut client_recv, &mut client_gate2, &limits)
            .await
            .expect("resync snapshot recv");
        let snapshot = decode_snapshot(&snapshot_body.payload).expect("resync decode");
        assert_eq!(snapshot.envelope.sim_tick, SimTick(42));
        local = snapshot
            .entries
            .iter()
            .map(|entry| {
                (
                    (entry.entity.0, entry.schema.clone()),
                    entry.payload.clone(),
                )
            })
            .collect();
        let catchup_body = recv_envelope(&mut client_recv, &mut client_gate2, &limits)
            .await
            .expect("catch-up recv");
        let catchup = decode_delta(&catchup_body.payload).expect("catch-up decode");
        catchup
            .validate_all(resync_snapshot_seq)
            .expect("catch-up base");
        for tombstone in &catchup.removals {
            if let Some(schema) = tombstone.schema.clone() {
                local.remove(&(tombstone.entity.0, schema));
            }
        }
        // Incremental replay, second half: the authoritative *changes*
        // since the shared baseline ride a normal chained delta (the
        // catch-up above carried removals only — additions need their
        // delta, which baseline retention plus the live log make sendable).
        let replay_seq = outbound_seq2();
        send_envelope(
            &mut server_send2,
            replay_seq,
            encode_delta(
                replay_seq,
                catchup_seq,
                SimTick(43),
                vec![crate::replication::ReplicatedEntry {
                    entity: NetEntityId(5),
                    schema: "canary.health".to_string(),
                    payload: b"hp:5".to_vec(),
                }],
                Vec::new(),
                &limits,
            )
            .expect("replay delta"),
            &limits,
        )
        .await
        .expect("replay send");
        let replay_body = recv_envelope(&mut client_recv, &mut client_gate2, &limits)
            .await
            .expect("replay recv");
        let replayed = decode_delta(&replay_body.payload).expect("replay decode");
        replayed.validate_all(catchup_seq).expect("replay base");
        for entry in &replayed.changes {
            local.insert(
                (entry.entity.0, entry.schema.clone()),
                entry.payload.clone(),
            );
        }
        // Converged to tick-43 server state without ever taking a full
        // live snapshot in session 2: shared baseline, replayed tombstones,
        // replayed changes.
        assert_eq!(local, authoritative);

        // Live deltas flow after the resync on the same session.
        authoritative.insert((5, "canary.health".to_string()), b"hp:4".to_vec());
        retention.push(SimTick(44), entries_of(&authoritative));
        let live_seq = outbound_seq2();
        send_envelope(
            &mut server_send2,
            live_seq,
            encode_delta(
                live_seq,
                replay_seq,
                SimTick(44),
                vec![crate::replication::ReplicatedEntry {
                    entity: NetEntityId(5),
                    schema: "canary.health".to_string(),
                    payload: b"hp:4".to_vec(),
                }],
                Vec::new(),
                &limits,
            )
            .expect("live delta"),
            &limits,
        )
        .await
        .expect("live send");
        let live_body = recv_envelope(&mut client_recv, &mut client_gate2, &limits)
            .await
            .expect("live recv");
        let live = decode_delta(&live_body.payload).expect("live decode");
        live.validate_all(replay_seq).expect("live base");
        for entry in &live.changes {
            local.insert(
                (entry.entity.0, entry.schema.clone()),
                entry.payload.clone(),
            );
        }
        assert_eq!(local, authoritative);

        // Session-1 envelopes are unintelligible to session 2: the old
        // session's sequences were retired with it (fresh gates both sides
        // accepted sequences restarting at 1 above, which a resumed gate
        // would have rejected as reordered).
        assert_eq!(client_gate2.last_accepted(), Some(live_seq));
    }

    /// Applies one received envelope to `local` under the session rules:
    /// first message and designated final message are snapshots (replace),
    /// the rest are deltas (validate base, then apply). Returns `true` when
    /// the message advanced convergence. Gap, duplicate, and reorder
    /// rejections are counted, never applied.
    fn apply_for_matrix(
        envelope: &NetEnvelope,
        is_snapshot: bool,
        local: &mut HashMap<(u64, String), Vec<u8>>,
        last_applied: &mut NetSequence,
        gaps: &mut u64,
    ) -> Result<(), NetError> {
        if is_snapshot {
            let snapshot = decode_snapshot(&envelope.payload)?;
            *local = snapshot
                .entries
                .iter()
                .map(|entry| {
                    (
                        (entry.entity.0, entry.schema.clone()),
                        entry.payload.clone(),
                    )
                })
                .collect();
            return Ok(());
        }
        let delta = decode_delta(&envelope.payload)?;
        if let Err(error) = delta.validate_all(*last_applied) {
            if error.resync_required() {
                *gaps = gaps.saturating_add(1);
            }
            return Err(error);
        }
        for entry in &delta.changes {
            local.insert(
                (entry.entity.0, entry.schema.clone()),
                entry.payload.clone(),
            );
        }
        for tombstone in &delta.removals {
            if let Some(schema) = tombstone.schema.clone() {
                local.remove(&(tombstone.entity.0, schema));
            }
        }
        *last_applied = delta.sequence;
        Ok(())
    }

    /// Network-conditions matrix over loopback: the same snapshot + deltas
    /// + final-snapshot script runs under each fault cell, and the client
    /// converges to server state every time without ever applying a message
    /// twice. Each fault cell additionally asserts its fault actually fired
    /// (a gap, a duplicate rejection, a reorder rejection), so a silently
    /// clean run cannot pass as fault tolerance. Logical assertions first;
    /// the latency cell's sleep is fixed with generous margins, never an
    /// exact deadline.
    #[tokio::test(flavor = "multi_thread")]
    async fn network_conditions_matrix_converges_without_corruption() {
        let limits = NetLimits::default();
        let cells: [(&str, FaultProfile); 5] = [
            ("clean", FaultProfile::clean()),
            (
                "loss",
                FaultProfile {
                    drop_every: 3,
                    ..FaultProfile::clean()
                },
            ),
            (
                "duplication",
                FaultProfile {
                    duplicate_every: 4,
                    ..FaultProfile::clean()
                },
            ),
            (
                "reorder",
                FaultProfile {
                    reorder_every: 4,
                    ..FaultProfile::clean()
                },
            ),
            (
                "latency",
                FaultProfile {
                    latency: Some(Duration::from_millis(5)),
                    ..FaultProfile::clean()
                },
            ),
        ];

        for (name, profile) in cells {
            let server_endpoint = LoopbackTransport::bind().expect("bind");
            let (client_send, mut client_recv, server_send, _) =
                connect_pair(&server_endpoint).await.expect("dial");
            drop(client_send);
            let mut faulty = FaultySend::new(server_send, profile, limits);
            let mut next_outbound = 1u64;
            let mut outbound_seq = || {
                let sequence = NetSequence(next_outbound);
                next_outbound += 1;
                sequence
            };

            // Scripted authority: snapshot, three deltas (one carries a
            // removal), then a final snapshot that supersedes everything.
            let mut authoritative: HashMap<(u64, String), Vec<u8>> = HashMap::from([
                ((1, "canary.health".to_string()), b"hp:10".to_vec()),
                ((2, "canary.health".to_string()), b"hp:20".to_vec()),
            ]);
            let snapshot_seq = outbound_seq();
            send_envelope(
                &mut faulty,
                snapshot_seq,
                encode_snapshot(entries_of(&authoritative), SimTick(50), &limits)
                    .expect("snapshot"),
                &limits,
            )
            .await
            .expect("send snapshot");

            authoritative.insert((1, "canary.health".to_string()), b"hp:11".to_vec());
            let delta_seqs = [outbound_seq(), outbound_seq(), outbound_seq()];
            send_envelope(
                &mut faulty,
                delta_seqs[0],
                encode_delta(
                    delta_seqs[0],
                    snapshot_seq,
                    SimTick(51),
                    vec![crate::replication::ReplicatedEntry {
                        entity: NetEntityId(1),
                        schema: "canary.health".to_string(),
                        payload: b"hp:11".to_vec(),
                    }],
                    Vec::new(),
                    &limits,
                )
                .expect("delta 1"),
                &limits,
            )
            .await
            .expect("send delta 1");

            authoritative.remove(&(2, "canary.health".to_string()));
            let removal = Tombstone::component_removed(NetEntityId(2), "canary.health", 52);
            send_envelope(
                &mut faulty,
                delta_seqs[1],
                encode_delta(
                    delta_seqs[1],
                    delta_seqs[0],
                    SimTick(52),
                    Vec::new(),
                    vec![removal],
                    &limits,
                )
                .expect("delta 2"),
                &limits,
            )
            .await
            .expect("send delta 2");

            authoritative.insert((1, "canary.health".to_string()), b"hp:12".to_vec());
            authoritative.insert((3, "canary.health".to_string()), b"hp:30".to_vec());
            send_envelope(
                &mut faulty,
                delta_seqs[2],
                encode_delta(
                    delta_seqs[2],
                    delta_seqs[1],
                    SimTick(53),
                    vec![
                        crate::replication::ReplicatedEntry {
                            entity: NetEntityId(1),
                            schema: "canary.health".to_string(),
                            payload: b"hp:12".to_vec(),
                        },
                        crate::replication::ReplicatedEntry {
                            entity: NetEntityId(3),
                            schema: "canary.health".to_string(),
                            payload: b"hp:30".to_vec(),
                        },
                    ],
                    Vec::new(),
                    &limits,
                )
                .expect("delta 3"),
                &limits,
            )
            .await
            .expect("send delta 3");

            let final_seq = outbound_seq();
            send_envelope(
                &mut faulty,
                final_seq,
                encode_snapshot(entries_of(&authoritative), SimTick(54), &limits)
                    .expect("final snapshot"),
                &limits,
            )
            .await
            .expect("send final");
            faulty.finish().await.expect("finish stream");

            // Client: apply under session rules; count, never corrupt.
            let mut gate = SequenceGate::new();
            let mut local: HashMap<(u64, String), Vec<u8>> = HashMap::new();
            let mut last_applied = NetSequence(0);
            let mut gaps = 0u64;
            let mut duplicates = 0u64;
            let mut reordered = 0u64;
            let mut gate_accepted: Vec<u64> = Vec::new();
            loop {
                let body = match client_recv.recv_frame(&limits).await {
                    Ok(body) => body,
                    Err(NetError::PeerClosed) => break,
                    Err(other) => panic!("{name}: transport failed cleanly? {other:?}"),
                };
                let envelope = NetEnvelope::decode(&body).expect("envelope verifies");
                match gate.check(envelope.sequence) {
                    Ok(()) => {
                        gate_accepted.push(envelope.sequence.0);
                    }
                    Err(NetError::DuplicateSequence { .. }) => {
                        duplicates = duplicates.saturating_add(1);
                        continue;
                    }
                    Err(NetError::ReorderedSequence { .. }) => {
                        reordered = reordered.saturating_add(1);
                        continue;
                    }
                    Err(other) => panic!("{name}: unexpected gate error {other:?}"),
                }
                if envelope.sequence == snapshot_seq {
                    last_applied = snapshot_seq;
                }
                let is_snapshot =
                    envelope.sequence == snapshot_seq || envelope.sequence == final_seq;
                match apply_for_matrix(
                    &envelope,
                    is_snapshot,
                    &mut local,
                    &mut last_applied,
                    &mut gaps,
                ) {
                    Ok(()) => {}
                    Err(error) if error.resync_required() => {}
                    Err(other) => panic!("{name}: unexpected apply error {other:?}"),
                }
            }

            // Convergence in every cell: the final snapshot supersedes any
            // skipped delta, and removals applied before it agree (the
            // removal of entity 2 is in both the delta and the snapshot).
            assert_eq!(local, authoritative, "{name}: client did not converge");
            // No session corruption: every accepted sequence applied at
            // most once (gate order is strictly increasing by construction).
            let mut accepted = gate_accepted.clone();
            accepted.sort_unstable();
            accepted.dedup();
            assert_eq!(
                accepted, gate_accepted,
                "{name}: a sequence was accepted twice"
            );
            // Faults actually fired where claimed — otherwise the cell is
            // vacuous.
            match name {
                "clean" => {
                    assert_eq!(gaps, 0, "clean cell must not gap");
                    assert_eq!(duplicates, 0, "clean cell must not duplicate");
                    assert_eq!(reordered, 0, "clean cell must not reorder");
                }
                "loss" => assert!(gaps > 0, "loss cell injected no gap"),
                "duplication" => {
                    assert!(duplicates > 0, "duplication cell injected no copy");
                }
                "reorder" => assert!(reordered > 0, "reorder cell inverted nothing"),
                "latency" => {
                    assert_eq!(gaps, 0, "latency alone must not gap");
                }
                _ => unreachable!("matrix cell {name} is not scripted"),
            }
        }
    }
}
