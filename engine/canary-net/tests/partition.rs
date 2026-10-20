//! Partition chaos sweep over the replication session (findings only).
//!
//! Deterministic scripted faults, no wall-clock sleeps: every receive loop
//! is bounded by an explicit step cap (exhaustion fails as deadlock, never
//! hangs), and every fault is counter-based or scripted message surgery.
//! Each scenario asserts convergence-or-typed-error: the client either
//! converges to authoritative state or reports a typed [`NetError`], and
//! never deadlocks or corrupts state.

use std::collections::HashMap;

use canary_net::{
    decide_handshake, decode_delta, decode_snapshot, encode_delta, encode_snapshot,
    BaselineRetention, ClientId, ClientSession, FaultProfile, FaultySend, GameVersion, Hello,
    InputValidator, LoopbackTransport, NetEntityId, NetEnvelope, NetError, NetLimits, NetRecv,
    NetSend, NetSequence, NetTransport, PluginApiVersion, ResyncPlan, SchemaManifestVersion,
    SequenceGate, SessionTable, SimTick, Tombstone, TombstoneLog, PROTOCOL_VERSION_1,
};

const ACTION_MOVE: &str = "canary.input/move@1";
const MANIFEST: SchemaManifestVersion = SchemaManifestVersion(3);
const GAME: GameVersion = GameVersion(11);
const PLUGIN_API: PluginApiVersion = PluginApiVersion(4);

/// Hard bound on any receive loop: exhaustion is a deadlock finding, and the
/// test fails instead of hanging (no wall-clock timeout involved).
const MAX_STEPS: usize = 64;

type StateMap = HashMap<(u64, String), Vec<u8>>;

fn entries_of(state: &StateMap) -> Vec<canary_net::ReplicatedEntry> {
    state
        .iter()
        .map(|((entity, schema), payload)| canary_net::ReplicatedEntry {
            entity: NetEntityId(*entity),
            schema: schema.clone(),
            payload: payload.clone(),
        })
        .collect()
}

fn apply_snapshot(local: &mut StateMap, payload: &[u8]) {
    let snapshot = decode_snapshot(payload).expect("snapshot decodes");
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
}

/// Applies one delta under session rules: validate-all before touching
/// state. Returns `true` when the delta advanced convergence; a base gap is
/// a counted typed error, never a partial apply.
fn apply_delta(
    local: &mut StateMap,
    last_applied: &mut NetSequence,
    payload: &[u8],
    gaps: &mut u64,
) {
    let delta = decode_delta(payload).expect("delta decodes");
    match delta.validate_all(*last_applied) {
        Ok(()) => {
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
        }
        Err(error) if error.resync_required() => {
            *gaps = gaps.saturating_add(1);
        }
        Err(other) => panic!("unexpected delta error: {other:?}"),
    }
}

async fn send_envelope<S: NetSend>(
    send: &mut S,
    sequence: NetSequence,
    payload: Vec<u8>,
    limits: &NetLimits,
) {
    let body = NetEnvelope::seal(PROTOCOL_VERSION_1, sequence, payload)
        .encode(limits)
        .expect("envelope seals");
    send.send_frame(&body, limits).await.expect("frame sends");
}

/// Scripted total partition: every even frame is dropped (`drop_every: 2`),
/// so the client observes sequence gaps mid-stream, then a final snapshot
/// supersedes everything. The client must converge with no corruption, and
/// the loss fault must have observably fired.
#[tokio::test(flavor = "multi_thread")]
async fn drop_all_deltas_mid_stream_then_snapshot_supersedes() {
    let limits = NetLimits::default();
    let server_endpoint = LoopbackTransport::bind().expect("bind loopback");
    let dialer = LoopbackTransport::ephemeral();
    let server_addr = server_endpoint.local_addr().expect("server addr");
    let (client_send, mut client_recv, server_send) = {
        let (client_send, client_recv) =
            dialer.connect(server_addr, "loopback").await.expect("dial");
        let (server_send, _server_recv) = server_endpoint.accept().await.expect("accept");
        (client_send, client_recv, server_send)
    };
    drop(client_send);
    let mut faulty = FaultySend::new(
        server_send,
        FaultProfile {
            drop_every: 2,
            ..FaultProfile::clean()
        },
        limits,
    );

    let mut authoritative: StateMap = HashMap::from([
        ((1, "canary.health".to_string()), b"hp:10".to_vec()),
        ((2, "canary.health".to_string()), b"hp:20".to_vec()),
    ]);
    let mut next_outbound = 1u64;
    let mut outbound_seq = || {
        let sequence = NetSequence(next_outbound);
        next_outbound += 1;
        sequence
    };

    let snapshot_seq = outbound_seq();
    send_envelope(
        &mut faulty,
        snapshot_seq,
        encode_snapshot(entries_of(&authoritative), SimTick(50), &limits).expect("snapshot"),
        &limits,
    )
    .await;
    // Three deltas the partition mauls (frames 2 and 4 dropped, frame 3
    // lands on a broken base), then the superseding snapshot (frame 5).
    // Frame indices count every offered frame, drops included.
    authoritative.insert((1, "canary.health".to_string()), b"hp:11".to_vec());
    let delta_a = outbound_seq();
    send_envelope(
        &mut faulty,
        delta_a,
        encode_delta(
            delta_a,
            snapshot_seq,
            SimTick(51),
            vec![canary_net::ReplicatedEntry {
                entity: NetEntityId(1),
                schema: "canary.health".to_string(),
                payload: b"hp:11".to_vec(),
            }],
            Vec::new(),
            &limits,
        )
        .expect("delta a"),
        &limits,
    )
    .await;
    authoritative.remove(&(2, "canary.health".to_string()));
    let delta_b = outbound_seq();
    send_envelope(
        &mut faulty,
        delta_b,
        encode_delta(
            delta_b,
            delta_a,
            SimTick(52),
            Vec::new(),
            vec![Tombstone::component_removed(
                NetEntityId(2),
                "canary.health",
                52,
            )],
            &limits,
        )
        .expect("delta b"),
        &limits,
    )
    .await;
    // Dropped with frame 4, but its change rides the final snapshot: this
    // is the proof that the snapshot supersedes, not merely repeats.
    authoritative.insert((1, "canary.health".to_string()), b"hp:12".to_vec());
    let delta_c = outbound_seq();
    send_envelope(
        &mut faulty,
        delta_c,
        encode_delta(
            delta_c,
            delta_b,
            SimTick(53),
            vec![canary_net::ReplicatedEntry {
                entity: NetEntityId(1),
                schema: "canary.health".to_string(),
                payload: b"hp:12".to_vec(),
            }],
            Vec::new(),
            &limits,
        )
        .expect("delta c"),
        &limits,
    )
    .await;
    authoritative.insert((3, "canary.health".to_string()), b"hp:30".to_vec());
    let final_seq = outbound_seq();
    send_envelope(
        &mut faulty,
        final_seq,
        encode_snapshot(entries_of(&authoritative), SimTick(54), &limits).expect("final snapshot"),
        &limits,
    )
    .await;
    faulty.finish().await.expect("finish stream");

    let mut gate = SequenceGate::new();
    let mut local: StateMap = HashMap::new();
    let mut last_applied = NetSequence(0);
    let mut gaps = 0u64;
    let mut duplicates = 0u64;
    let mut reordered = 0u64;
    let mut steps = 0usize;
    loop {
        steps += 1;
        assert!(
            steps <= MAX_STEPS,
            "deadlock: stream did not close within {MAX_STEPS} steps"
        );
        let body = match client_recv.recv_frame(&limits).await {
            Ok(body) => body,
            Err(NetError::PeerClosed) => break,
            Err(other) => panic!("transport failed cleanly? {other:?}"),
        };
        let envelope = NetEnvelope::decode(&body).expect("envelope verifies");
        match gate.check(envelope.sequence) {
            Ok(()) => {}
            Err(NetError::DuplicateSequence { .. }) => {
                duplicates = duplicates.saturating_add(1);
                continue;
            }
            Err(NetError::ReorderedSequence { .. }) => {
                reordered = reordered.saturating_add(1);
                continue;
            }
            Err(other) => panic!("unexpected gate error {other:?}"),
        }
        if envelope.sequence == snapshot_seq {
            last_applied = snapshot_seq;
        }
        if envelope.sequence == snapshot_seq || envelope.sequence == final_seq {
            apply_snapshot(&mut local, &envelope.payload);
        } else {
            apply_delta(&mut local, &mut last_applied, &envelope.payload, &mut gaps);
        }
    }

    assert!(
        gaps > 0,
        "drop_every:2 must have opened at least one base gap, else the fault never fired"
    );
    assert_eq!(duplicates, 0, "loss alone must not duplicate");
    assert_eq!(reordered, 0, "loss alone must not reorder");
    assert_eq!(
        local, authoritative,
        "client converges through the superseding snapshot despite the partition"
    );
}

/// Handshake-then-snapshot reorder: a snapshot arriving before the
/// handshake ack is a typed gate rejection, never applied state. Replaying
/// in order converges cleanly.
#[test]
fn snapshot_before_handshake_ack_is_typed_reorder_never_corruption() {
    let limits = NetLimits::default();
    let hello = Hello {
        protocol_version: PROTOCOL_VERSION_1,
        schema_manifest: MANIFEST,
        game_version: GAME,
        plugin_api_version: PLUGIN_API,
        client_label: "partition-proof".to_string(),
    };
    let welcome = decide_handshake(
        &hello,
        PROTOCOL_VERSION_1,
        MANIFEST,
        GAME,
        PLUGIN_API,
        7,
        100,
    )
    .expect("compatible hello");

    let state: StateMap = HashMap::from([((9, "canary.health".to_string()), b"hp:10".to_vec())]);
    let snapshot_bytes =
        encode_snapshot(entries_of(&state), SimTick(41), &limits).expect("snapshot");
    let welcome_bytes = canary_net::encode_welcome(&welcome, &limits).expect("welcome encodes");

    // Misordered arrival: snapshot (wire seq 2) before welcome (wire seq 1).
    let mut gate = SequenceGate::new();
    let mut local: StateMap = HashMap::new();
    gate.check(NetSequence(2)).expect("first frame arrives");
    let late = gate.check(NetSequence(1));
    assert!(
        matches!(
            late,
            Err(NetError::ReorderedSequence {
                sequence: 1,
                last_accepted: 2
            })
        ),
        "late handshake ack is a typed reorder, got {late:?}"
    );
    assert!(local.is_empty(), "rejected frame applies no state");

    // In-order replay converges: welcome first, then the snapshot.
    let mut gate = SequenceGate::new();
    gate.check(NetSequence(1)).expect("welcome first");
    let welcome_back = canary_net::decode_welcome(&welcome_bytes).expect("welcome decodes");
    assert_eq!(welcome_back.session_id, 100);
    gate.check(NetSequence(2)).expect("snapshot second");
    apply_snapshot(&mut local, &snapshot_bytes);
    assert_eq!(local, state, "ordered replay converges exactly");

    // A delta against an unknown base is a typed resync signal, not a hang.
    let delta_bytes = encode_delta(
        NetSequence(3),
        NetSequence(99),
        SimTick(42),
        Vec::new(),
        Vec::new(),
        &limits,
    )
    .expect("delta encodes");
    let delta = decode_delta(&delta_bytes).expect("delta decodes");
    let err = delta
        .validate_all(NetSequence(2))
        .expect_err("unknown base fails");
    assert!(
        matches!(err, NetError::UnknownBaseSequence { .. }),
        "typed unknown-base error, got {err:?}"
    );
    assert!(
        err.resync_required(),
        "unknown base must be resync-required, never fatal"
    );
}

/// Ack discipline: repeating an ack is idempotent, while any backward cursor
/// is a typed rejection that moves nothing and keeps the connection alive.
#[test]
fn duplicate_ack_is_idempotent_backward_ack_is_typed() {
    let limits = NetLimits::default();
    let (send_caps, recv_caps) = (
        ClientSession::caps_from_limits(&limits),
        ClientSession::caps_from_limits(&limits),
    );
    let mut table = SessionTable::new();
    let client = ClientId(1);
    table.connect(
        client,
        7,
        100,
        send_caps,
        recv_caps,
        InputValidator::new(7, &[ACTION_MOVE], 64, 4, 8),
    );
    let ack = canary_net::session::ClientAck {
        last_applied: NetSequence(4),
        tombstone_cursor: 2,
        last_acked_tick: SimTick(42),
    };
    let session = table.get_mut(client).expect("record exists");
    session.apply_ack(&ack).expect("forward ack applies");
    // Exact redelivery: idempotent success, cursors unmoved.
    session
        .apply_ack(&ack)
        .expect("duplicate ack is idempotent");
    assert_eq!(session.last_applied(), NetSequence(4));
    assert_eq!(session.tombstone_cursor(), 2);
    assert_eq!(session.last_acked_tick(), SimTick(42));

    for backward in [
        canary_net::session::ClientAck {
            last_applied: NetSequence(3),
            tombstone_cursor: 2,
            last_acked_tick: SimTick(42),
        },
        canary_net::session::ClientAck {
            last_applied: NetSequence(4),
            tombstone_cursor: 1,
            last_acked_tick: SimTick(42),
        },
        canary_net::session::ClientAck {
            last_applied: NetSequence(4),
            tombstone_cursor: 2,
            last_acked_tick: SimTick(41),
        },
    ] {
        let err = session
            .apply_ack(&backward)
            .expect_err("backward cursor fails");
        assert!(
            matches!(err, NetError::InvalidInput { .. }),
            "typed invalid-input for {backward:?}, got {err:?}"
        );
    }
    // Nothing moved and the record is still live: the connection survives.
    let session = table.get(client).expect("connection stays alive");
    assert_eq!(session.last_applied(), NetSequence(4));
    assert_eq!(session.tombstone_cursor(), 2);
    assert_eq!(session.last_acked_tick(), SimTick(42));
}

/// Peer restart under the same identity: reconnecting replaces the record
/// wholesale (fresh session id, reset cursors), then the retained baseline
/// replays incrementally; a baseline past retention plans a full snapshot.
#[test]
fn peer_restart_with_same_identity_resets_then_resyncs() {
    let limits = NetLimits::default();
    let (send_caps, recv_caps) = (
        ClientSession::caps_from_limits(&limits),
        ClientSession::caps_from_limits(&limits),
    );
    let mut table = SessionTable::new();
    let mut tombstones = TombstoneLog::new(64);
    let mut retention = BaselineRetention::new(8);
    let client = ClientId(1);

    table.connect(
        client,
        7,
        100,
        send_caps,
        recv_caps,
        InputValidator::new(7, &[ACTION_MOVE], 64, 4, 8),
    );
    table
        .get_mut(client)
        .expect("record exists")
        .apply_ack(&canary_net::session::ClientAck {
            last_applied: NetSequence(2),
            tombstone_cursor: 0,
            last_acked_tick: SimTick(41),
        })
        .expect("first-session ack");

    // Tick 41 baseline, then the tick-42 removal the crashed client never
    // saw (log sequence 0, so its cursor is still 0).
    let mut authoritative: StateMap = HashMap::from([
        ((2, "canary.health".to_string()), b"hp:7".to_vec()),
        ((9, "canary.health".to_string()), b"hp:10".to_vec()),
    ]);
    retention.push(SimTick(41), entries_of(&authoritative));
    tombstones.record(Tombstone::component_removed(
        NetEntityId(2),
        "canary.health",
        42,
    ));
    authoritative.remove(&(2, "canary.health".to_string()));
    retention.push(SimTick(42), entries_of(&authoritative));

    // Restart: same client identity, new session. No residue may survive.
    table.disconnect(client);
    assert!(table.is_empty(), "disconnect leaves no record");
    table.connect(
        client,
        7,
        101,
        send_caps,
        recv_caps,
        InputValidator::new(7, &[ACTION_MOVE], 64, 4, 8),
    );
    let fresh = table.get(client).expect("reconnected");
    assert_eq!(fresh.session_id(), 101, "reconnect mints a new session id");
    assert_eq!(
        fresh.last_acked_tick(),
        SimTick(0),
        "reconnect resets cursors, never resumes them"
    );
    assert_eq!(fresh.last_applied(), NetSequence(0));

    // Incremental resync from the pre-crash cursor converges: the client
    // holds the tick-41 baseline and needs everything from log cursor 0.
    let plan = retention.plan(SimTick(41), 0, &tombstones);
    let ResyncPlan::Covered {
        tick,
        entries,
        tombstones: replay,
    } = plan
    else {
        panic!("retained tick 41 plus cursor 0 must plan incrementally");
    };
    assert_eq!(tick, SimTick(41));
    let mut local: StateMap = entries
        .iter()
        .map(|entry| {
            (
                (entry.entity.0, entry.schema.clone()),
                entry.payload.clone(),
            )
        })
        .collect();
    assert_eq!(replay.len(), 1, "exactly the post-cursor removal replays");
    assert_eq!(replay[0].entity, NetEntityId(2));
    let replay_bytes = encode_snapshot(entries, tick, &limits).expect("replay snapshot");
    apply_snapshot(&mut local, &replay_bytes);
    for tombstone in &replay {
        if let Some(schema) = tombstone.schema.clone() {
            local.remove(&(tombstone.entity.0, schema));
        }
    }
    assert_eq!(
        local, authoritative,
        "reconnect converges without corruption"
    );

    // A baseline the ring no longer retains plans a full live snapshot,
    // never an error and never a partial replay.
    let mut small = BaselineRetention::new(2);
    small.push(SimTick(100), entries_of(&authoritative));
    small.push(SimTick(101), entries_of(&authoritative));
    assert_eq!(
        small.plan(SimTick(41), 0, &tombstones),
        ResyncPlan::FullSnapshot,
        "unretained baseline falls back to a full snapshot"
    );
}
