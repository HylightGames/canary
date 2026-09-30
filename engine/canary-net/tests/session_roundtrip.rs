//! Separate-process session proof: a real server process and a real client
//! (this test) perform handshake → snapshot → delta → input → ack over
//! loopback QUIC on an ephemeral port.
//!
//! The server runs in a child OS process — the same test binary re-executed
//! with `--exact child_server_main` plus `CANARY_NET_CHILD_DIR` pointing at
//! the pre-generated dev identity — so no loopback mock stands in for the
//! network: real UDP sockets, real TLS pinning, real ALPN negotiation.
//!
//! Assertions compare logical state (decoded entries, ticks, ack fields),
//! never raw bytes: the server inserts snapshot entries in shuffled order
//! and the client still converges to the same key→payload map.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use canary_net::{
    decide_handshake, decode_ack, decode_delta, decode_hello, decode_input, decode_snapshot,
    decode_welcome, encode_ack, encode_delta, encode_hello, encode_input, encode_snapshot,
    encode_welcome, identity_codec,
};
use canary_net::{
    ClientId, ClientSession, GameVersion, InputValidator, NetEntityId, NetEnvelope, NetError,
    NetLimits, NetRecv, NetSend, NetSequence, NetTransport, PluginApiVersion, ProtocolVersion,
    QuinnTransport, SchemaCodecs, SchemaManifestVersion, SequenceGate, SessionTable, SimTick,
    Tombstone, TombstoneLog, PROTOCOL_VERSION_1,
};

/// Action schema the proof server accepts input for.
const ACTION_MOVE: &str = "canary.input/move@1";
/// Schema manifest both ends of the proof speak.
const PROOF_MANIFEST: SchemaManifestVersion = SchemaManifestVersion(3);
/// Game release both ends of the proof run.
const PROOF_GAME: GameVersion = GameVersion(11);
/// Plugin/API surface both ends of the proof support.
const PROOF_PLUGIN_API: PluginApiVersion = PluginApiVersion(4);
/// Assigned player slot for the proof connection.
const PROOF_SLOT: u64 = 7;
/// Env var (present only in the child) pointing at the dev identity dir.
const CHILD_DIR_ENV: &str = "CANARY_NET_CHILD_DIR";

/// Issues a self-signed certificate for `localhost` (proof/test use only)
/// and returns DER certificate + PKCS#8 key bytes.
fn localhost_identity() -> (Vec<u8>, Vec<u8>) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("rcgen self-signed certificate");
    (
        certified.cert.der().to_vec(),
        certified.key_pair.serialize_der(),
    )
}

async fn send_envelope<S: NetSend>(
    send: &mut S,
    sequence: NetSequence,
    payload: Vec<u8>,
    limits: &NetLimits,
) -> Result<(), NetError> {
    let body = NetEnvelope::seal(PROTOCOL_VERSION_1, sequence, payload).encode(limits)?;
    send.send_frame(&body, limits).await
}

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

/// Validator for the proof connection: the assigned slot, the one accepted
/// action schema, and a window around the proof ticks.
fn proof_validator() -> InputValidator {
    InputValidator::new(PROOF_SLOT, &[ACTION_MOVE], 64, 4, 8)
}

/// The server half. Runs in the child process: binds an ephemeral
/// loopback port, prints `READY <addr>`, serves exactly one session
/// (handshake → snapshot → delta → input → ack), removes the client
/// record, and exits.
async fn server_main(cert_der: Vec<u8>, key_der: Vec<u8>) -> Result<(), NetError> {
    let limits = NetLimits::default();
    let server =
        QuinnTransport::server(SocketAddr::from(([127, 0, 0, 1], 0)), &cert_der, &key_der)?;
    println!("READY {}", server.local_addr()?);
    std::io::stdout().flush().map_err(|error| {
        NetError::Transport(format!(
            "proof server failed to announce its address: {error}"
        ))
    })?;

    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(30), server.accept())
        .await
        .map_err(|_| NetError::TransportAccept {
            detail: "proof server timed out waiting for the client".to_string(),
        })??;
    let mut gate = SequenceGate::new();
    let mut table = SessionTable::new();
    // Server outbound wire sequences: one counter for this connection.
    let mut next_outbound: u64 = 1;
    let mut outbound_seq = || {
        let sequence = NetSequence(next_outbound);
        next_outbound += 1;
        sequence
    };

    // 1. Handshake: typed Hello in, typed Welcome (or typed Reject + close)
    // out. No session record exists before the hello is accepted.
    let hello_body = recv_envelope(&mut recv, &mut gate, &limits).await?;
    let hello = decode_hello(&hello_body.payload)?;
    let (send_caps, recv_caps) = (
        ClientSession::caps_from_limits(&limits),
        ClientSession::caps_from_limits(&limits),
    );
    // Admit the client before deciding, so a reject still tears down a
    // real record path; a rejected hello leaves no record behind.
    let client = ClientId(1);
    let session_id: u64 = 100;
    match decide_handshake(
        &hello,
        PROTOCOL_VERSION_1,
        PROOF_MANIFEST,
        PROOF_GAME,
        PROOF_PLUGIN_API,
        PROOF_SLOT,
        session_id,
    ) {
        Ok(welcome) => {
            table.connect(
                client,
                welcome.assigned_slot,
                welcome.session_id,
                send_caps,
                recv_caps,
                proof_validator(),
            );
            let welcome_seq = outbound_seq();
            send_envelope(
                &mut send,
                welcome_seq,
                encode_welcome(&welcome, &limits)?,
                &limits,
            )
            .await?;
        }
        Err(reject) => {
            let reject_seq = outbound_seq();
            send_envelope(
                &mut send,
                reject_seq,
                canary_net::encode_reject(&reject, &limits)?,
                &limits,
            )
            .await?;
            send.finish().await?;
            return Err(NetError::UnsupportedProtocol {
                got: hello.protocol_version.0,
                supported: reject.supported_protocol.0,
            });
        }
    }

    // 2. Snapshot: entries inserted shuffled; the canonical codec sorts
    // them — the client converges on content, not arrival order.
    let snapshot_seq = outbound_seq();
    let snapshot_bytes = encode_snapshot(
        vec![
            canary_net::ReplicatedEntry {
                entity: NetEntityId(9),
                schema: "canary.health".to_string(),
                payload: b"hp:10".to_vec(),
            },
            canary_net::ReplicatedEntry {
                entity: NetEntityId(2),
                schema: "canary.health".to_string(),
                payload: b"hp:7".to_vec(),
            },
        ],
        SimTick(41),
        &limits,
    )?;
    send_envelope(&mut send, snapshot_seq, snapshot_bytes, &limits).await?;

    // 3. Delta against the snapshot base: one change plus the tombstone
    // the removal log still holds for this client's cursor.
    let mut tombstones = TombstoneLog::new(64);
    tombstones.record(Tombstone::component_removed(
        NetEntityId(2),
        "canary.health",
        41,
    ));
    let cursor = table
        .get(client)
        .map(|session| session.tombstone_cursor())
        .unwrap_or(0);
    let pending: Vec<Tombstone> = tombstones
        .pending_since(cursor)
        .map_err(|error| {
            NetError::Transport(format!(
                "proof tombstone cursor unexpectedly fell behind: {error}"
            ))
        })?
        .into_iter()
        .map(|logged| logged.tombstone.clone())
        .collect();
    let delta_seq = outbound_seq();
    let delta_bytes = encode_delta(
        delta_seq,
        snapshot_seq,
        SimTick(42),
        vec![canary_net::ReplicatedEntry {
            entity: NetEntityId(9),
            schema: "canary.health".to_string(),
            payload: b"hp:9".to_vec(),
        }],
        pending,
        &limits,
    )?;
    send_envelope(&mut send, delta_seq, delta_bytes, &limits).await?;

    // 4. Input: validate-all-before-apply, then acknowledge the sequence.
    // A rejected input would keep the connection alive; the proof sends a
    // valid one and expects the echo.
    let input_body = recv_envelope(&mut recv, &mut gate, &limits).await?;
    let input = decode_input(&input_body.payload)?;
    let validated = table
        .get_mut(client)
        .ok_or_else(|| NetError::Transport("proof client record vanished".to_string()))?
        .validator()
        .validate(&input, 42)?;
    let ack = canary_net::InputAck {
        input_seq: validated.input_seq,
        applied_tick: 43,
    };
    let ack_seq = outbound_seq();
    send_envelope(&mut send, ack_seq, encode_ack(&ack, &limits)?, &limits).await?;

    // 5. Clean disconnect: the record is removed whole, nothing lingers.
    let removed = table.disconnect(client);
    assert!(removed.is_some(), "client record must exist to remove");
    assert!(table.is_empty(), "client record must leave no residue");
    send.finish().await?;
    // Graceful shutdown: wait for the client's send-finish EOF before this
    // process exits, so the connection is never torn down under the ack's
    // in-flight bytes. The terminal read resolves as a truncation or a
    // clean close; either way the outcome is ignored — the proof already
    // passed and this only gates process exit on delivery.
    let _ = tokio::time::timeout(Duration::from_secs(20), recv.recv_frame(&limits)).await;
    Ok(())
}

/// The client half. Runs in this test process against the child server's
/// address and asserts logical convergence at every step.
async fn client_main(addr: SocketAddr, cert_der: Vec<u8>) -> Result<(), NetError> {
    let limits = NetLimits::default();
    let client = QuinnTransport::client(&cert_der)?;
    let (mut send, mut recv) =
        tokio::time::timeout(Duration::from_secs(20), client.connect(addr, "localhost"))
            .await
            .map_err(|_| NetError::TransportConnect {
                detail: "proof client timed out dialing the server".to_string(),
            })??;
    let mut gate = SequenceGate::new();
    let mut next_seq = 1u64;
    let mut envelope_seq = || {
        let sequence = NetSequence(next_seq);
        next_seq += 1;
        sequence
    };

    // 1. Handshake.
    let hello = canary_net::Hello {
        protocol_version: PROTOCOL_VERSION_1,
        schema_manifest: PROOF_MANIFEST,
        game_version: PROOF_GAME,
        plugin_api_version: PROOF_PLUGIN_API,
        client_label: "loopback-proof".to_string(),
    };
    send_envelope(
        &mut send,
        envelope_seq(),
        encode_hello(&hello, &limits)?,
        &limits,
    )
    .await?;
    let welcome_body = recv_envelope(&mut recv, &mut gate, &limits).await?;
    let welcome = decode_welcome(&welcome_body.payload)?;
    assert_eq!(welcome.assigned_slot, PROOF_SLOT);
    assert_eq!(welcome.protocol_version, PROTOCOL_VERSION_1);

    // 2. Snapshot: logical content equals the server's set regardless of
    // the order the server inserted (or the bytes) — sort both sides.
    let snapshot_body = recv_envelope(&mut recv, &mut gate, &limits).await?;
    let snapshot = decode_snapshot(&snapshot_body.payload)?;
    assert_eq!(snapshot.envelope.sim_tick, SimTick(41));
    let mut seen: Vec<(u64, String, Vec<u8>)> = snapshot
        .entries
        .iter()
        .map(|entry| (entry.entity.0, entry.schema.clone(), entry.payload.clone()))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (2, "canary.health".to_string(), b"hp:7".to_vec()),
            (9, "canary.health".to_string(), b"hp:10".to_vec()),
        ]
    );

    // 3. Delta: base names the snapshot sequence; every payload passes
    // the registered codec; applying changes+removals converges the map.
    let delta_body = recv_envelope(&mut recv, &mut gate, &limits).await?;
    let delta = decode_delta(&delta_body.payload)?;
    delta.validate_all(snapshot_body.sequence)?;
    assert_eq!(delta.sim_tick, SimTick(42));
    let mut codecs = SchemaCodecs::new();
    codecs.register("canary.health", identity_codec());
    let decoded = codecs.decode_delta_payloads(&delta)?;
    assert_eq!(
        decoded,
        vec![(
            NetEntityId(9),
            "canary.health".to_string(),
            b"hp:9".to_vec()
        )]
    );

    let mut local: HashMap<(u64, String), Vec<u8>> = seen
        .into_iter()
        .map(|(entity, schema, payload)| ((entity, schema), payload))
        .collect();
    for (entity, schema, payload) in &decoded {
        local.insert((entity.0, schema.clone()), payload.clone());
    }
    assert_eq!(delta.removals.len(), 1);
    assert_eq!(
        delta.removals[0],
        Tombstone::component_removed(NetEntityId(2), "canary.health", 41)
    );
    for tombstone in &delta.removals {
        if let Some(schema) = tombstone.schema.clone() {
            local.remove(&(tombstone.entity.0, schema));
        }
    }
    // Logical equality with the server's authoritative post-delta state.
    assert_eq!(
        local,
        HashMap::from([((9, "canary.health".to_string()), b"hp:9".to_vec())])
    );

    // 4. Input → ack: the server echoes the accepted sequence.
    let input = canary_net::ClientInput {
        player_slot: PROOF_SLOT,
        target_tick: 43,
        action_schema: ACTION_MOVE.to_string(),
        action_version: 1,
        payload: b"dx:1".to_vec(),
        input_seq: 1,
    };
    send_envelope(
        &mut send,
        envelope_seq(),
        encode_input(&input, &limits)?,
        &limits,
    )
    .await?;
    let ack_body = recv_envelope(&mut recv, &mut gate, &limits).await?;
    let ack = decode_ack(&ack_body.payload)?;
    assert_eq!(ack.input_seq, 1);
    assert_eq!(ack.applied_tick, 43);

    send.finish().await?;
    Ok(())
}

/// Child-process entry: serves one session and exits. Returns immediately
/// in the parent (where the env var is absent) so a plain `cargo test`
/// never binds a socket here.
#[tokio::test(flavor = "multi_thread")]
async fn child_server_main() {
    let dir = match std::env::var(CHILD_DIR_ENV) {
        Ok(dir) => dir,
        Err(_) => return,
    };
    let dir = std::path::PathBuf::from(dir);
    let cert_der = std::fs::read(dir.join("server.der")).expect("child reads server cert");
    let key_der = std::fs::read(dir.join("server.key")).expect("child reads server key");
    if let Err(error) = server_main(cert_der, key_der).await {
        eprintln!("proof server failed: {error}");
        std::process::exit(1);
    }
}

/// Reads stdout lines from the child until the `READY <addr>` announcement
/// arrives, skipping libtest harness chatter (which shares stdout under
/// `--nocapture`). Fails on EOF or timeout rather than orphaning the child.
fn read_ready_line(
    child: &mut Child,
) -> Result<(String, std::sync::mpsc::Receiver<Vec<String>>), String> {
    let stdout = child.stdout.take().ok_or("child stdout not piped")?;
    let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
    let (tail_sender, tail_receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        let mut ready_sent = false;
        let mut tailed: Vec<String> = Vec::new();
        for line in lines.by_ref() {
            match line {
                Ok(line) if !ready_sent && line.starts_with("READY ") => {
                    let _ = ready_sender.send(Ok(line));
                    ready_sent = true;
                }
                Ok(line) => {
                    // Harness chatter and the test summary share this pipe:
                    // keep it drained so the child never writes into a
                    // closed pipe, and retain a tail for failure reports.
                    tailed.push(line);
                    if tailed.len() > 32 {
                        tailed.remove(0);
                    }
                }
                Err(error) => {
                    if !ready_sent {
                        let report: Result<String, String> =
                            Err(format!("failed reading server stdout: {error}"));
                        let _ = ready_sender.send(report);
                        ready_sent = true;
                    }
                    break;
                }
            }
        }
        if !ready_sent {
            let report: Result<String, String> =
                Err("server exited without announcing an address".to_string());
            let _ = ready_sender.send(report);
        }
        let _ = tail_sender.send(tailed);
    });
    let line = ready_receiver
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| "timed out waiting for the server READY line".to_string())??;
    Ok((line, tail_receiver))
}

#[tokio::test(flavor = "multi_thread")]
async fn session_roundtrip_over_loopback_quinn() {
    let (cert_der, key_der) = localhost_identity();
    let proof_dir = std::env::temp_dir().join(format!("canary-net-proof-{}", std::process::id()));
    std::fs::create_dir_all(&proof_dir).expect("proof scratch dir");
    std::fs::write(proof_dir.join("server.der"), &cert_der).expect("write server cert");
    std::fs::write(proof_dir.join("server.key"), &key_der).expect("write server key");

    let exe = std::env::current_exe().expect("current test binary");
    let mut child = Command::new(&exe)
        .args(["--exact", "child_server_main", "--nocapture"])
        .env(CHILD_DIR_ENV, &proof_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn proof server process");

    let outcome: Result<(), String> = async {
        let (line, tail_receiver) = read_ready_line(&mut child)?;
        let addr: SocketAddr = line
            .strip_prefix("READY ")
            .ok_or(format!("unexpected server announcement: {line:?}"))?
            .parse()
            .map_err(|error| format!("unparsable server address: {error}"))?;
        // Graceful shutdown order: the client finishes its send direction
        // after reading the ack; the server waits for that EOF before
        // exiting, so neither side tears the connection down under the
        // other's in-flight bytes.
        client_main(addr, cert_der.clone())
            .await
            .map_err(|error| format!("client round-trip failed: {error}"))?;
        // The server exits on its own after its clean disconnect; wait
        // (blocking) with a deadline rather than orphaning it.
        let mut waiting = child;
        let stderr = waiting.stderr.take();
        let status = tokio::task::spawn_blocking(move || waiting.wait())
            .await
            .map_err(|error| format!("server join failed: {error}"))?
            .map_err(|error| format!("waiting for server exit failed: {error}"))?;
        if !status.success() {
            let mut detail = format!("proof server exited with {status}");
            if let Ok(tailed) = tail_receiver.try_recv() {
                let tail = tailed.join("\n");
                if !tail.trim().is_empty() {
                    detail.push_str("\n--- server stdout tail ---\n");
                    detail.push_str(&tail);
                }
            }
            if let Some(mut stderr) = stderr {
                use std::io::Read as _;
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                if !text.trim().is_empty() {
                    detail.push_str("\n--- server stderr ---\n");
                    detail.push_str(&text);
                }
            }
            return Err(detail);
        }
        Ok(())
    }
    .await;

    let _ = std::fs::remove_dir_all(&proof_dir);
    if let Err(detail) = outcome {
        panic!("{detail}");
    }
    // Keep the unused-import census honest across refactors: the version
    // domains really are distinct types on this path.
    assert_eq!(ProtocolVersion(1), PROTOCOL_VERSION_1);
}
