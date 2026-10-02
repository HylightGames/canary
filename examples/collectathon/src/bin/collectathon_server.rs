//! `collectathon_server`: the authoritative replication server.
//!
//! Serves exactly one session over real UDP/TLS/ALPN (QUIC via
//! [`QuinnTransport`](canary_net::QuinnTransport)): handshake → snapshot →
//! delta → input → ack, then a clean disconnect. The snapshot and delta
//! carry the collectathon game codecs from
//! [`replication`](collectathon::replication) — `SimComponent` bytes flow
//! into [`ReplicatedEntry`](canary_net::ReplicatedEntry) payloads, the
//! server-scoped [`NetEntityMap`](canary_net::NetEntityMap) owns identity,
//! and a [`TombstoneLog`](canary_net::TombstoneLog) carries the pickup
//! removal. Between snapshot and delta the server advances its own
//! authoritative state (one pickup collected and banked, one pickup removed),
//! so the delta is a real state progression, not an echo.
//!
//! The server is authoritative and this path carries no prediction or
//! rollback: the client converges on canonical entries and submits
//! frame-tagged input for this server to validate (ADR 0027).
//!
//! ```sh
//! cargo run -p collectathon --bin collectathon_server -- --bind 127.0.0.1:0
//! cargo run -p collectathon --bin collectathon_client -- --server <READY addr> --cert <identity dir>/server.der
//! ```

use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;

use canary_ecs::World;
use canary_net::{
    decide_handshake, decode_hello, decode_input, encode_ack, encode_delta, encode_reject,
    encode_snapshot, encode_welcome, ClientId, ClientSession, InputAck, NetEntityId, NetEntityMap,
    NetError, NetLimits, NetRecv, NetSend, NetSequence, NetTransport, QuinnTransport, SequenceGate,
    SessionTable, SimTick, Tombstone, TombstoneLog,
};
use canary_runtime::AuthoredSpawner;
use collectathon::assets;
use collectathon::game::{self, SimHandles};
use collectathon::net_session::{
    self, NetHarnessError, ACTION_SCHEMA, DELTA_TICK, INPUT_TICK, SESSION_GAME, SESSION_ID,
    SESSION_MANIFEST, SESSION_PLUGIN_API, SESSION_SLOT, SNAPSHOT_TICK,
};
use collectathon::replication::{mark_replicated, replication_codecs, REPLICATED_SCHEMAS};
use collectathon::state::{CollectathonDecoder, GameStats};
use collectathon::{Pickup, Score};

/// Builds the authoritative game world: authored room spawn plus the
/// simulation attachments, with every gameplay entity marked as a
/// replication candidate.
fn build_authoritative_world() -> Result<World, NetHarnessError> {
    let mut world = World::new();
    game::register_game_components(&mut world)?;
    let game_assets = assets::load_game_assets()
        .map_err(|error| NetHarnessError::Usage(format!("server asset load: {error}")))?;
    let room = assets::load_room(&assets::asset_dir().join(assets::ROOM_FILE))?;
    let decoder = CollectathonDecoder;
    let spawner = AuthoredSpawner::new(&decoder, &assets::resolve_asset);
    let report = spawner
        .spawn(&mut world, &room)
        .map_err(NetHarnessError::from)?;
    game::attach_simulation_components(
        &mut world,
        &report,
        &SimHandles {
            pickup_sound: game_assets.pickup_sound,
        },
    )?;
    world.insert_resource(game_assets.sounds);
    let player_entities: Vec<_> = world
        .query::<collectathon::Player>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in player_entities {
        mark_replicated(&mut world, entity)?;
    }
    let pickup_entities: Vec<_> = world.query::<Pickup>().map(|(entity, _)| entity).collect();
    for entity in pickup_entities {
        mark_replicated(&mut world, entity)?;
    }
    let score_entities: Vec<_> = world.query::<Score>().map(|(entity, _)| entity).collect();
    for entity in score_entities {
        mark_replicated(&mut world, entity)?;
    }
    Ok(world)
}

/// Advances the authoritative state between snapshot and delta: collects
/// the first uncollected shard (banking one point) and removes one other
/// pickup entity, recording its destruction in `tombstones`. Returns how
/// many pickups were collected and the destroyed network id, if any.
fn advance_authoritative_state(
    world: &mut World,
    entity_map: &NetEntityMap,
    tombstones: &mut TombstoneLog,
) -> Result<(u32, Option<NetEntityId>), NetHarnessError> {
    let mut collected = 0u32;
    let mut destroy_candidate: Option<canary_ecs::Entity> = None;
    let pickups: Vec<_> = world.query::<Pickup>().map(|(entity, _)| entity).collect();
    for entity in pickups {
        let Some(pickup) = world.get::<Pickup>(entity) else {
            continue;
        };
        if pickup.collected {
            continue;
        }
        if collected == 0 && !pickup.is_goal {
            if let Some(slot) = world.get_mut::<Pickup>(entity) {
                slot.collected = true;
            }
            collected += 1;
        } else if destroy_candidate.is_none() {
            destroy_candidate = Some(entity);
        }
    }
    if collected > 0 {
        let scores: Vec<_> = world.query::<Score>().map(|(entity, _)| entity).collect();
        for entity in scores {
            if let Some(score) = world.get_mut::<Score>(entity) {
                score.points = score.points.saturating_add(collected);
            }
        }
        if let Some(stats) = world.resource_mut::<GameStats>() {
            stats.collected = stats.collected.saturating_add(collected);
        }
    }
    let mut destroyed = None;
    if let Some(entity) = destroy_candidate {
        let net_id = entity_map
            .resolve(entity.index(), entity.generation())
            .ok_or_else(|| {
                NetHarnessError::Usage("destroyed pickup has no server network id".to_owned())
            })?;
        world.despawn(entity)?;
        tombstones.record(Tombstone::entity_destroyed(net_id, DELTA_TICK.0));
        destroyed = Some(net_id);
    }
    Ok((collected, destroyed))
}

fn print_usage() {
    println!(
        "collectathon_server: authoritative replication server\n\
         \n\
         usage: collectathon_server [--bind ADDR] [--identity-dir DIR]\n\
         \n\
         Binds a QUIC server on ADDR (default 127.0.0.1:0), writes the\n\
         development identity to DIR/server.der plus DIR/server.key\n\
         (default: a fresh temp dir), prints `READY <addr> <dir>`, serves\n\
         one session (handshake, snapshot, delta, input, ack), then exits."
    );
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("collectathon_server: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), NetHarnessError> {
    let mut bind: SocketAddr = "127.0.0.1:0".parse()?;
    let mut identity_dir: Option<std::path::PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            "--bind" => {
                let value = args
                    .next()
                    .ok_or_else(|| NetHarnessError::Usage("--bind needs an address".to_owned()))?;
                bind = value.parse()?;
            }
            "--identity-dir" => {
                let value = args.next().ok_or_else(|| {
                    NetHarnessError::Usage("--identity-dir needs a directory".to_owned())
                })?;
                identity_dir = Some(std::path::PathBuf::from(value));
            }
            other => {
                return Err(NetHarnessError::Usage(format!(
                    "unknown argument '{other}'; see --help"
                )));
            }
        }
    }

    let dir = identity_dir.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("collectathon-server-{}", std::process::id()))
    });
    std::fs::create_dir_all(&dir)?;
    let (cert_der, key_der) = net_session::localhost_identity()?;
    std::fs::write(dir.join("server.der"), &cert_der)?;
    std::fs::write(dir.join("server.key"), &key_der)?;

    let mut world = build_authoritative_world()?;
    let mut entity_map = NetEntityMap::new();

    let limits = NetLimits::default();
    let server = QuinnTransport::server(bind, &cert_der, &key_der)?;
    println!("READY {} {}", server.local_addr()?, dir.display());
    std::io::stdout().flush()?;
    eprintln!(
        "collectathon_server: serving schemas [{}]",
        REPLICATED_SCHEMAS.join(", ")
    );

    serve_one(server, &mut world, &mut entity_map, &limits).await?;
    Ok(())
}

/// Serves exactly one session: handshake, snapshot, authoritative delta,
/// validated input, ack, then a clean whole-record disconnect.
async fn serve_one(
    server: QuinnTransport,
    world: &mut World,
    entity_map: &mut NetEntityMap,
    limits: &NetLimits,
) -> Result<(), NetHarnessError> {
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(30), server.accept())
        .await
        .map_err(|_| {
            NetHarnessError::Usage("server timed out waiting for the client".to_owned())
        })??;
    let mut gate = SequenceGate::new();
    let mut table = SessionTable::new();
    let mut next_outbound: u64 = 1;
    let mut outbound_seq = || {
        let sequence = NetSequence(next_outbound);
        next_outbound += 1;
        sequence
    };

    // 1. Handshake: typed Hello in, typed Welcome (or typed Reject + close).
    let hello_body = net_session::recv_envelope(&mut recv, &mut gate, limits).await?;
    let hello = decode_hello(&hello_body.payload)?;
    eprintln!(
        "collectathon_server: hello protocol={} manifest={} game={} plugin_api={} label={}",
        hello.protocol_version.0,
        hello.schema_manifest.0,
        hello.game_version.0,
        hello.plugin_api_version.0,
        hello.client_label
    );
    let client = ClientId(1);
    let (send_caps, recv_caps) = (
        ClientSession::caps_from_limits(limits),
        ClientSession::caps_from_limits(limits),
    );
    let validator = canary_net::InputValidator::new(SESSION_SLOT, &[ACTION_SCHEMA], 64, 4, 8);
    match decide_handshake(
        &hello,
        net_session::SESSION_PROTOCOL,
        SESSION_MANIFEST,
        SESSION_GAME,
        SESSION_PLUGIN_API,
        SESSION_SLOT,
        SESSION_ID,
    ) {
        Ok(welcome) => {
            table.connect(
                client,
                welcome.assigned_slot,
                welcome.session_id,
                send_caps,
                recv_caps,
                validator,
            );
            eprintln!(
                "collectathon_server: welcome slot={} session={}",
                welcome.assigned_slot, welcome.session_id
            );
            net_session::send_envelope(
                &mut send,
                outbound_seq(),
                encode_welcome(&welcome, limits)?,
                limits,
            )
            .await?;
        }
        Err(reject) => {
            eprintln!("collectathon_server: reject {reject:?}");
            net_session::send_envelope(
                &mut send,
                outbound_seq(),
                encode_reject(&reject, limits)?,
                limits,
            )
            .await?;
            send.finish().await?;
            return Err(NetHarnessError::Usage(format!(
                "handshake rejected: {:?}",
                reject.reason
            )));
        }
    }

    // 2. Snapshot: authoritative entries in reverse spawn order — the
    // canonical codec sorts them, so the client converges on content.
    let mut entries = net_session::snapshot_entries(world, entity_map);
    entries.reverse();
    let snapshot_seq = outbound_seq();
    let snapshot_bytes = encode_snapshot(entries, SNAPSHOT_TICK, limits)?;
    net_session::send_envelope(&mut send, snapshot_seq, snapshot_bytes, limits).await?;
    eprintln!("collectathon_server: snapshot at tick {}", SNAPSHOT_TICK.0);

    // 3. Authoritative progression plus the destruction the removal log
    // still holds for this client's cursor.
    let mut tombstones = TombstoneLog::new(64);
    let (collected, destroyed) = advance_authoritative_state(world, entity_map, &mut tombstones)?;
    eprintln!(
        "collectathon_server: authoritative tick — collected {collected}, destroyed {destroyed:?}"
    );
    let cursor = table
        .get(client)
        .map(|session| session.tombstone_cursor())
        .unwrap_or(0);
    let pending: Vec<Tombstone> = tombstones
        .pending_since(cursor)
        .map_err(|error| {
            NetError::Transport(format!(
                "tombstone cursor unexpectedly fell behind: {error}"
            ))
        })?
        .into_iter()
        .map(|logged| logged.tombstone.clone())
        .collect();
    let changes = net_session::snapshot_entries(world, entity_map);
    let delta_seq = outbound_seq();
    let delta_bytes = encode_delta(
        delta_seq,
        snapshot_seq,
        DELTA_TICK,
        changes,
        pending,
        limits,
    )?;
    // The game codecs serve this delta: decode back through the
    // registered registry before it reaches the wire, so an unregistered
    // schema fails here as resync-required, never on the client.
    let delta = canary_net::decode_delta(&delta_bytes)?;
    let decoded = replication_codecs().decode_delta_payloads(&delta)?;
    eprintln!(
        "collectathon_server: delta at tick {} — {} changes through the game codecs, {} removals",
        DELTA_TICK.0,
        decoded.len(),
        delta.removals.len()
    );
    net_session::send_envelope(&mut send, delta_seq, delta_bytes, limits).await?;

    // 4. Input: validate-all-before-apply, then acknowledge the sequence.
    // A rejected input would keep the connection alive; this session
    // expects the valid frame-tagged input and echoes it.
    let input_body = net_session::recv_envelope(&mut recv, &mut gate, limits).await?;
    let input = decode_input(&input_body.payload)?;
    let validated = table
        .get_mut(client)
        .ok_or_else(|| NetHarnessError::Usage("client record vanished before input".to_owned()))?
        .validator()
        .validate(&input, INPUT_TICK)?;
    eprintln!(
        "collectathon_server: input seq={} tick={} schema={}",
        validated.input_seq, validated.target_tick, validated.action_schema
    );
    let ack = InputAck {
        input_seq: validated.input_seq,
        applied_tick: SimTick(INPUT_TICK).0,
    };
    net_session::send_envelope(&mut send, outbound_seq(), encode_ack(&ack, limits)?, limits)
        .await?;

    // 5. Clean disconnect: the record is removed whole, nothing lingers.
    let removed = table.disconnect(client);
    if removed.is_none() {
        return Err(NetHarnessError::Usage(
            "client record must exist to remove".to_owned(),
        ));
    }
    if !table.is_empty() {
        return Err(NetHarnessError::Usage(
            "client record must leave no residue".to_owned(),
        ));
    }
    eprintln!("collectathon_server: clean disconnect, session served");
    send.finish().await?;
    // Graceful shutdown: wait for the client's send-finish EOF before
    // exiting, so the connection is never torn down under the ack's
    // in-flight bytes. The outcome is ignored — the session already
    // passed and this only gates process exit on delivery.
    let _ = tokio::time::timeout(Duration::from_secs(20), recv.recv_frame(limits)).await;
    Ok(())
}
