//! Per-client session state: who is connected and what they have (ADR 0027,
//! points 4, 8, 10).
//!
//! A [`ClientSession`] is the server's record for one connected client: the
//! slot it was assigned at handshake, the tombstone cursor bounding what
//! removals it still needs, the newest delta sequence and simulation tick it
//! acknowledged, its bounded send/recv queues, its inbound sequence gate,
//! and its input validator. [`SessionTable`] owns every connected record.
//!
//! Disconnect is removal: [`SessionTable::disconnect`] drops the whole
//! record — queues, cursors, gate, validator — so a reconnect starts fresh
//! with no residue. Reconnecting is a new session (new
//! [`session id`](crate::handshake::Welcome::session_id)), never a resumed
//! one; when the server still retains the old baseline the reconnect
//! replays it incrementally instead of restarting from live state (see
//! [`crate::resync::BaselineRetention::plan`]).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::NetError;
use crate::ids::{NetSequence, SimTick};
use crate::input::InputValidator;
use crate::limits::NetLimits;
use crate::queue::BoundedQueue;
use crate::sequence::SequenceGate;

/// Server-side handle for one connected client.
///
/// Opaque beyond equality and hashing: the server mints these, the wire
/// never carries one (the wire binds messages with the handshake
/// [`session id`](crate::handshake::Welcome::session_id) instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub u64);

/// What a client has applied: the wire-level acknowledgement that advances
/// the server's per-client cursors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientAck {
    /// Newest delta sequence the client fully applied. Must not move
    /// backward — see [`ClientSession::apply_ack`].
    pub last_applied: NetSequence,
    /// Tombstone log sequence the client needs next (one past the last it
    /// acknowledged). Feeds ack-gated reclamation across all clients.
    pub tombstone_cursor: u64,
    /// Newest simulation tick the client has converged to. The server
    /// captures dirty sets via `query_changed_since` against this timeline.
    pub last_acked_tick: SimTick,
}

/// One connected client's full server-side record.
#[derive(Debug)]
pub struct ClientSession {
    /// Which client this record belongs to.
    id: ClientId,
    /// Player slot assigned at handshake; input for any other slot fails
    /// ingress validation.
    assigned_slot: u64,
    /// Session id issued in the handshake [`Welcome`](crate::handshake::Welcome).
    session_id: u64,
    /// Tombstone log sequence the client needs next.
    tombstone_cursor: u64,
    /// Newest delta sequence the client acknowledged.
    last_applied: NetSequence,
    /// Newest simulation tick the client converged to.
    last_acked_tick: SimTick,
    /// Outbound wire bytes waiting on the socket. Drained in FIFO order;
    /// overflow follows the queue's push-time policy.
    send_queue: BoundedQueue<Vec<u8>>,
    /// Inbound wire bytes waiting on the simulation. Always pushed under
    /// [`QueuePolicy::Disconnect`](crate::queue::QueuePolicy::Disconnect):
    /// ingress must never silently drop client data to make room.
    recv_queue: BoundedQueue<Vec<u8>>,
    /// Inbound session-sequence gate: a message is never applied twice.
    inbound_gate: SequenceGate,
    /// Ingress validator for this connection's slot. The only mutable state
    /// the input path touches besides the queues.
    validator: InputValidator,
}

impl ClientSession {
    /// A fresh record for a newly admitted connection. Cursors start at the
    /// beginning of history: nothing applied, nothing acknowledged.
    pub fn new(
        id: ClientId,
        assigned_slot: u64,
        session_id: u64,
        send_caps: (usize, usize),
        recv_caps: (usize, usize),
        validator: InputValidator,
    ) -> Self {
        Self {
            id,
            assigned_slot,
            session_id,
            tombstone_cursor: 0,
            last_applied: NetSequence(0),
            last_acked_tick: SimTick(0),
            send_queue: BoundedQueue::new(send_caps.0, send_caps.1),
            recv_queue: BoundedQueue::new(recv_caps.0, recv_caps.1),
            inbound_gate: SequenceGate::new(),
            validator,
        }
    }

    /// Builds queue caps from the shared [`NetLimits`] knobs.
    pub fn caps_from_limits(limits: &NetLimits) -> (usize, usize) {
        (
            usize::try_from(limits.max_queued_messages).unwrap_or(usize::MAX),
            usize::try_from(limits.max_queued_bytes).unwrap_or(usize::MAX),
        )
    }

    /// Which client this record belongs to.
    #[must_use]
    pub fn id(&self) -> ClientId {
        self.id
    }

    /// Player slot assigned at handshake.
    #[must_use]
    pub fn assigned_slot(&self) -> u64 {
        self.assigned_slot
    }

    /// Session id issued at handshake.
    #[must_use]
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Tombstone log sequence the client needs next.
    #[must_use]
    pub fn tombstone_cursor(&self) -> u64 {
        self.tombstone_cursor
    }

    /// Newest delta sequence the client acknowledged.
    #[must_use]
    pub fn last_applied(&self) -> NetSequence {
        self.last_applied
    }

    /// Newest simulation tick the client converged to.
    #[must_use]
    pub fn last_acked_tick(&self) -> SimTick {
        self.last_acked_tick
    }

    /// Outbound queue (wire bytes waiting on the socket).
    pub fn send_queue(&mut self) -> &mut BoundedQueue<Vec<u8>> {
        &mut self.send_queue
    }

    /// Inbound queue (wire bytes waiting on the simulation).
    pub fn recv_queue(&mut self) -> &mut BoundedQueue<Vec<u8>> {
        &mut self.recv_queue
    }

    /// Inbound session-sequence gate.
    pub fn inbound_gate(&mut self) -> &mut SequenceGate {
        &mut self.inbound_gate
    }

    /// Ingress validator for this connection's slot.
    pub fn validator(&mut self) -> &mut InputValidator {
        &mut self.validator
    }

    /// Advances the acknowledged cursors from a client acknowledgement.
    ///
    /// Every cursor moves forward or stays (a repeated ack is idempotent);
    /// a backward `last_applied`, `tombstone_cursor`, or `last_acked_tick`
    /// fails as [`NetError::InvalidInput`] and moves nothing — a confused
    /// client cannot rewind the reclamation horizon or the dirty-set
    /// timeline (`query_changed_since` against a rewound tick would
    /// resurrect already-applied state as new changes). The connection
    /// stays alive either way.
    pub fn apply_ack(&mut self, ack: &ClientAck) -> Result<(), NetError> {
        if ack.last_applied < self.last_applied {
            return Err(NetError::InvalidInput {
                detail: "acknowledgement moves last-applied backward".to_string(),
            });
        }
        if ack.tombstone_cursor < self.tombstone_cursor {
            return Err(NetError::InvalidInput {
                detail: "acknowledgement moves the tombstone cursor backward".to_string(),
            });
        }
        if ack.last_acked_tick < self.last_acked_tick {
            return Err(NetError::InvalidInput {
                detail: "acknowledgement moves the acked simulation tick backward".to_string(),
            });
        }
        self.last_applied = ack.last_applied;
        self.tombstone_cursor = ack.tombstone_cursor;
        self.last_acked_tick = ack.last_acked_tick;
        Ok(())
    }
}

/// Encodes a [`ClientAck`] for the wire, gated by `limits` before it
/// reaches the transport.
pub fn encode_ack(ack: &ClientAck, limits: &NetLimits) -> Result<Vec<u8>, NetError> {
    let bytes = postcard::to_allocvec(ack)?;
    let len = u32::try_from(bytes.len()).map_err(|_| NetError::OversizeFrame {
        claimed: u32::MAX,
        max: limits.max_message_bytes,
    })?;
    limits.check_frame_len(len)?;
    Ok(bytes)
}

/// Decodes a [`ClientAck`], rejecting trailing bytes. Decoding is not
/// acceptance: the result is untrusted until [`ClientSession::apply_ack`]
/// checks it against the recorded cursors.
pub fn decode_ack(bytes: &[u8]) -> Result<ClientAck, NetError> {
    let (ack, remainder): (ClientAck, &[u8]) = postcard::take_from_bytes(bytes)?;
    if !remainder.is_empty() {
        return Err(NetError::TrailingBytes {
            trailing: remainder.len(),
        });
    }
    Ok(ack)
}

/// Every connected client's record, owned by the server session.
#[derive(Debug, Default)]
pub struct SessionTable {
    /// Live records by client id.
    clients: HashMap<ClientId, ClientSession>,
}

impl SessionTable {
    /// An empty table: no clients connected.
    #[must_use]
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    /// Admits (or re-admits) `id` with a fresh record.
    ///
    /// Any existing record for `id` is replaced wholesale — queues,
    /// cursors, gate, validator — so a reconnect starts with no residue
    /// from the previous connection. Returns the fresh record.
    pub fn connect(
        &mut self,
        id: ClientId,
        assigned_slot: u64,
        session_id: u64,
        send_caps: (usize, usize),
        recv_caps: (usize, usize),
        validator: InputValidator,
    ) -> &mut ClientSession {
        let session = ClientSession::new(
            id,
            assigned_slot,
            session_id,
            send_caps,
            recv_caps,
            validator,
        );
        self.clients.insert(id, session);
        self.clients
            .get_mut(&id)
            .unwrap_or_else(|| unreachable!("session record was just inserted for an existing key"))
    }

    /// Drops the client's whole record: queues, cursors, gate, validator.
    /// Returns the removed record, or `None` when `id` was not connected.
    pub fn disconnect(&mut self, id: ClientId) -> Option<ClientSession> {
        self.clients.remove(&id)
    }

    /// Live record for `id`, if connected.
    pub fn get(&self, id: ClientId) -> Option<&ClientSession> {
        self.clients.get(&id)
    }

    /// Live record for `id`, mutably, if connected.
    pub fn get_mut(&mut self, id: ClientId) -> Option<&mut ClientSession> {
        self.clients.get_mut(&id)
    }

    /// How many clients are connected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.clients.len()
    }

    /// Whether any client is connected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Minimum tombstone cursor across connected clients, or `None` when no
    /// client is connected. The caller passes this to
    /// [`TombstoneLog::reclaim_through`](crate::tombstone::TombstoneLog::reclaim_through):
    /// history every client acknowledged is freed, anything any client may
    /// still need is kept.
    #[must_use]
    pub fn min_tombstone_cursor(&self) -> Option<u64> {
        self.clients
            .values()
            .map(|session| session.tombstone_cursor)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{PushOutcome, QueuePolicy};

    fn validator_for(slot: u64) -> InputValidator {
        InputValidator::new(slot, &["canary.input/move@1"], 64, 4, 8)
    }

    fn caps() -> ((usize, usize), (usize, usize)) {
        ((8, 1024), (8, 1024))
    }

    fn ack(last_applied: u64, cursor: u64, tick: u64) -> ClientAck {
        ClientAck {
            last_applied: NetSequence(last_applied),
            tombstone_cursor: cursor,
            last_acked_tick: SimTick(tick),
        }
    }

    #[test]
    fn ack_advances_cursors_and_repeats_are_idempotent() {
        let (send, recv) = caps();
        let mut session = ClientSession::new(ClientId(1), 7, 99, send, recv, validator_for(7));
        session.apply_ack(&ack(3, 2, 40)).expect("forward ack");
        assert_eq!(session.last_applied(), NetSequence(3));
        assert_eq!(session.tombstone_cursor(), 2);
        assert_eq!(session.last_acked_tick(), SimTick(40));
        // A repeated ack changes nothing and fails nothing.
        session.apply_ack(&ack(3, 2, 40)).expect("repeated ack");
        assert_eq!(session.last_applied(), NetSequence(3));
    }

    #[test]
    fn backward_ack_moves_nothing_and_keeps_the_connection() {
        let (send, recv) = caps();
        let mut session = ClientSession::new(ClientId(1), 7, 99, send, recv, validator_for(7));
        session.apply_ack(&ack(5, 4, 50)).expect("seed");
        let error = session
            .apply_ack(&ack(4, 4, 50))
            .expect_err("backward base");
        assert!(matches!(error, NetError::InvalidInput { .. }));
        assert_eq!(session.last_applied(), NetSequence(5));
        let error = session
            .apply_ack(&ack(5, 3, 50))
            .expect_err("backward cursor");
        assert!(matches!(error, NetError::InvalidInput { .. }));
        assert_eq!(session.tombstone_cursor(), 4);
        // The simulation-tick cursor is monotonic too: a rewound tick
        // would resurrect already-applied state as new dirty sets, so it
        // fails and moves nothing — including the other two cursors.
        let error = session
            .apply_ack(&ack(6, 5, 49))
            .expect_err("backward tick");
        assert!(matches!(error, NetError::InvalidInput { .. }));
        assert_eq!(session.last_acked_tick(), SimTick(50));
        assert_eq!(session.last_applied(), NetSequence(5));
        assert_eq!(session.tombstone_cursor(), 4);
        // Still usable: the next forward ack lands.
        session.apply_ack(&ack(6, 5, 51)).expect("usable");
        assert_eq!(session.last_acked_tick(), SimTick(51));
    }

    #[test]
    fn disconnect_removes_everything_and_reconnect_starts_fresh() {
        let (send, recv) = caps();
        let mut table = SessionTable::new();
        let id = ClientId(7);
        table.connect(id, 7, 100, send, recv, validator_for(7));
        {
            let session = table.get_mut(id).expect("connected");
            session
                .send_queue()
                .push(vec![1u8], 1, QueuePolicy::Disconnect)
                .expect("queue");
            session.apply_ack(&ack(9, 8, 70)).expect("advance");
            session
                .inbound_gate()
                .check(NetSequence(3))
                .expect("gate advances");
        }
        let removed = table.disconnect(id).expect("record");
        assert_eq!(removed.id(), id);
        assert!(table.is_empty());
        assert!(table.get(id).is_none());

        // Reconnecting the same id starts with no residue: empty queues,
        // zeroed cursors, a fresh gate and validator.
        table.connect(id, 7, 101, send, recv, validator_for(7));
        let fresh = table.get_mut(id).expect("reconnected");
        assert_eq!(fresh.session_id(), 101);
        assert!(fresh.send_queue().is_empty());
        assert!(fresh.recv_queue().is_empty());
        assert_eq!(fresh.last_applied(), NetSequence(0));
        assert_eq!(fresh.tombstone_cursor(), 0);
        assert_eq!(fresh.inbound_gate().last_accepted(), None);
        assert_eq!(fresh.validator().last_input_seq(), None);
        // The old push outcome is gone with the old record.
        assert_eq!(fresh.send_queue().pop(), None);
        let _ = PushOutcome::Accepted;
    }

    #[test]
    fn min_cursor_spans_connected_clients_for_reclamation() {
        let (send, recv) = caps();
        let mut table = SessionTable::new();
        assert_eq!(table.min_tombstone_cursor(), None);
        table.connect(ClientId(1), 1, 10, send, recv, validator_for(1));
        table.connect(ClientId(2), 2, 11, send, recv, validator_for(2));
        table
            .get_mut(ClientId(1))
            .expect("one")
            .apply_ack(&ack(4, 6, 40))
            .expect("ack one");
        table
            .get_mut(ClientId(2))
            .expect("two")
            .apply_ack(&ack(4, 2, 40))
            .expect("ack two");
        // Reclamation may only free what *every* client acknowledged.
        assert_eq!(table.min_tombstone_cursor(), Some(2));
        table.disconnect(ClientId(2));
        assert_eq!(table.min_tombstone_cursor(), Some(6));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn ack_codec_round_trip_rejects_trailing_bytes() {
        let limits = NetLimits::default();
        let bytes = encode_ack(&ack(2, 1, 30), &limits).expect("encode");
        assert_eq!(decode_ack(&bytes).expect("decode"), ack(2, 1, 30));
        let mut smuggled = bytes;
        smuggled.push(0);
        assert!(matches!(
            decode_ack(&smuggled),
            Err(NetError::TrailingBytes { .. })
        ));
    }
}
