//! `collectathon_client`: the replication client.
//!
//! Connects to a [`collectathon_server`](crate::bin::collectathon_server)
//! over real UDP/TLS/ALPN (QUIC via
//! [`QuinnTransport`](canary_net::QuinnTransport)) with the pinned
//! development certificate, then: handshake, converge on the (shuffled)
//! snapshot, apply the sequenced delta, send frame-tagged input, await the
//! ack, and disconnect cleanly.
//!
//! The client is authoritative-server only: it converges on canonical
//! entries and applies them through the typed `SimComponent` decoders, with
//! validate-all-before-apply on the delta. No prediction, no rollback
//! (out of scope per ADR 0027).
//!
//! ```sh
//! cargo run -p collectathon --bin collectathon_client -- --server 127.0.0.1:PORT --cert DIR/server.der
//! ```

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::time::Duration;

use canary_ecs::{CanaryComponent, Entity};
use canary_net::{
    decode_ack, decode_delta, decode_snapshot, decode_welcome, encode_hello, encode_input,
    ClientInput, Hello, NetLimits, NetSend, NetSequence, NetTransport, QuinnTransport,
    SequenceGate,
};
use collectathon::game;
use collectathon::net_session::{
    self, NetHarnessError, ACTION_SCHEMA, ACTION_VERSION, INPUT_TICK, SESSION_GAME,
    SESSION_PLUGIN_API, SESSION_SLOT,
};
use collectathon::replication::{replication_codecs, REPLICATED_SCHEMAS};
use collectathon::state::GameStats;
use collectathon::{Pickup, Score};

fn print_usage() {
    println!(
        "collectathon_client: replication client\n\
         \n\
         usage: collectathon_client --server ADDR --cert PATH\n\
         \n\
         Dials the collectathon server at ADDR, pinning the development\n\
         certificate at PATH (the server prints both on its READY line),\n\
         converges on its snapshot and delta, submits frame-tagged input,\n\
         awaits the ack, and disconnects cleanly."
    );
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("collectathon_client: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), NetHarnessError> {
    let mut server_addr: Option<SocketAddr> = None;
    let mut cert_path: Option<std::path::PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            "--server" => {
                let value = args.next().ok_or_else(|| {
                    NetHarnessError::Usage("--server needs an address".to_owned())
                })?;
                server_addr = Some(value.parse()?);
            }
            "--cert" => {
                let value = args
                    .next()
                    .ok_or_else(|| NetHarnessError::Usage("--cert needs a path".to_owned()))?;
                cert_path = Some(std::path::PathBuf::from(value));
            }
            other => {
                return Err(NetHarnessError::Usage(format!(
                    "unknown argument '{other}'; see --help"
                )));
            }
        }
    }
    let server_addr = server_addr
        .ok_or_else(|| NetHarnessError::Usage("missing --server ADDR; see --help".to_owned()))?;
    let cert_path = cert_path
        .ok_or_else(|| NetHarnessError::Usage("missing --cert PATH; see --help".to_owned()))?;
    let cert_der = std::fs::read(&cert_path)?;

    let mut world = canary_ecs::World::new();
    game::register_game_components(&mut world)?;
    world.insert_resource(GameStats::default());

    let limits = NetLimits::default();
    let client = QuinnTransport::client(&cert_der)?;
    let (mut send, mut recv) = tokio::time::timeout(
        Duration::from_secs(20),
        client.connect(server_addr, "localhost"),
    )
    .await
    .map_err(|_| NetHarnessError::Usage("client timed out dialing the server".to_owned()))??;
    let mut gate = SequenceGate::new();
    let mut next_seq = 1u64;
    let mut envelope_seq = || {
        let sequence = NetSequence(next_seq);
        next_seq += 1;
        sequence
    };

    // 1. Handshake: the server echoes the assigned slot on success.
    let hello = Hello {
        protocol_version: net_session::SESSION_PROTOCOL,
        schema_manifest: net_session::SESSION_MANIFEST,
        game_version: SESSION_GAME,
        plugin_api_version: SESSION_PLUGIN_API,
        client_label: "collectathon-client".to_string(),
    };
    net_session::send_envelope(
        &mut send,
        envelope_seq(),
        encode_hello(&hello, &limits)?,
        &limits,
    )
    .await?;
    let welcome_body = net_session::recv_envelope(&mut recv, &mut gate, &limits).await?;
    let welcome = decode_welcome(&welcome_body.payload)?;
    if welcome.assigned_slot != SESSION_SLOT {
        return Err(NetHarnessError::Usage(format!(
            "server assigned unexpected slot {}",
            welcome.assigned_slot
        )));
    }
    if welcome.protocol_version != net_session::SESSION_PROTOCOL {
        return Err(NetHarnessError::Usage(
            "server welcomed the wrong protocol version".to_owned(),
        ));
    }
    eprintln!(
        "collectathon_client: welcome slot={} session={}",
        welcome.assigned_slot, welcome.session_id
    );

    // 2. Snapshot: logical content equals the server's set regardless of
    // arrival order — the canonical codec sorts both sides.
    let snapshot_body = net_session::recv_envelope(&mut recv, &mut gate, &limits).await?;
    let snapshot = decode_snapshot(&snapshot_body.payload)?;
    if snapshot.envelope.sim_tick != net_session::SNAPSHOT_TICK {
        return Err(NetHarnessError::Usage(format!(
            "snapshot tick {} is not the authoritative {:?}",
            snapshot.envelope.sim_tick.0,
            net_session::SNAPSHOT_TICK
        )));
    }
    let mut local: BTreeMap<u64, Entity> = BTreeMap::new();
    let mut converged: HashMap<(u64, String), Vec<u8>> = HashMap::new();
    // Insertion order here is deliberately shuffled relative to the
    // server's spawn order; convergence is on content, not order.
    let mut entries = snapshot.entries.clone();
    entries.reverse();
    for entry in &entries {
        if !REPLICATED_SCHEMAS.contains(&entry.schema.as_str()) {
            return Err(NetHarnessError::Usage(format!(
                "snapshot carries unregistered schema '{}'",
                entry.schema
            )));
        }
        net_session::apply_entry_to_world(
            &mut world,
            &mut local,
            entry.entity,
            &entry.schema,
            &entry.payload,
        )?;
        converged.insert(
            (entry.entity.0, entry.schema.clone()),
            entry.payload.clone(),
        );
    }
    eprintln!(
        "collectathon_client: snapshot converged on {} entries across {} entities",
        converged.len(),
        local.len()
    );

    // 3. Delta: base names the snapshot sequence; every payload passes the
    // registered game codecs; applying changes plus removals converges the
    // map onto the server's authoritative post-delta state.
    let delta_body = net_session::recv_envelope(&mut recv, &mut gate, &limits).await?;
    let delta = decode_delta(&delta_body.payload)?;
    delta.validate_all(snapshot_body.sequence)?;
    if delta.sim_tick != net_session::DELTA_TICK {
        return Err(NetHarnessError::Usage(format!(
            "delta tick {} is not the authoritative {:?}",
            delta.sim_tick.0,
            net_session::DELTA_TICK
        )));
    }
    let codecs = replication_codecs();
    let decoded = codecs.decode_delta_payloads(&delta)?;
    for (entity, schema, payload) in &decoded {
        net_session::apply_entry_to_world(&mut world, &mut local, *entity, schema, payload)?;
        converged.insert((entity.0, schema.clone()), payload.clone());
    }
    for tombstone in &delta.removals {
        apply_tombstone(&mut world, &mut local, tombstone)?;
        match tombstone.schema.clone() {
            Some(schema) => {
                converged.remove(&(tombstone.entity.0, schema));
            }
            None => {
                converged.retain(|(entity, _), _| *entity != tombstone.entity.0);
            }
        }
    }
    eprintln!(
        "collectathon_client: delta applied — {} changes, {} removals, {} live keys",
        decoded.len(),
        delta.removals.len(),
        converged.len()
    );
    report_world(&world);

    // 4. Input → ack: frame-tagged input for the authoritative tick; the
    // server echoes the accepted sequence.
    let input = ClientInput {
        player_slot: SESSION_SLOT,
        target_tick: INPUT_TICK,
        action_schema: ACTION_SCHEMA.to_string(),
        action_version: ACTION_VERSION,
        payload: b"collect:left".to_vec(),
        input_seq: 1,
    };
    net_session::send_envelope(
        &mut send,
        envelope_seq(),
        encode_input(&input, &limits)?,
        &limits,
    )
    .await?;
    let ack_body = net_session::recv_envelope(&mut recv, &mut gate, &limits).await?;
    let ack = decode_ack(&ack_body.payload)?;
    if ack.input_seq != 1 || ack.applied_tick != INPUT_TICK {
        return Err(NetHarnessError::Usage(format!(
            "ack mismatch: seq={} tick={}",
            ack.input_seq, ack.applied_tick
        )));
    }
    eprintln!(
        "collectathon_client: ack seq={} applied_tick={}",
        ack.input_seq, ack.applied_tick
    );

    send.finish().await?;
    eprintln!("collectathon_client: clean disconnect");
    Ok(())
}

/// Applies one authoritative tombstone to the client's world: destroyed
/// entities despawn whole, removed components detach by schema.
fn apply_tombstone(
    world: &mut canary_ecs::World,
    local: &mut BTreeMap<u64, Entity>,
    tombstone: &canary_net::Tombstone,
) -> Result<(), NetHarnessError> {
    let Some(entity) = local.get(&tombstone.entity.0).copied() else {
        return Err(NetHarnessError::Usage(format!(
            "tombstone names unknown network entity {}",
            tombstone.entity.0
        )));
    };
    match tombstone.schema.clone() {
        None => {
            world.despawn(entity)?;
            local.remove(&tombstone.entity.0);
        }
        Some(schema) => {
            if schema == Pickup::SCHEMA_ID {
                world.remove::<Pickup>(entity);
            } else if schema == collectathon::Player::SCHEMA_ID {
                world.remove::<collectathon::Player>(entity);
            } else if schema == Score::SCHEMA_ID {
                world.remove::<Score>(entity);
            } else {
                return Err(NetHarnessError::Usage(format!(
                    "tombstone carries unregistered schema '{schema}'"
                )));
            }
        }
    }
    Ok(())
}

/// Prints the converged gameplay state: score, collection stats, player.
fn report_world(world: &canary_ecs::World) {
    let score = world.query::<Score>().next().map(|(_, score)| score.points);
    let stats = world
        .resource::<GameStats>()
        .map(|stats| (stats.collected, stats.goal, world.query::<Pickup>().count()));
    let player = world
        .query::<collectathon::Player>()
        .next()
        .map(|(_, player)| (player.x, player.y));
    eprintln!("collectathon_client: converged score={score:?} stats={stats:?} player={player:?}");
}
