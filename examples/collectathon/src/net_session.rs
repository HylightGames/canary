//! Shared replication-session harness: version domains, envelope framing,
//! and the `SimComponent` bytes ↔ `ReplicatedEntry.payload` bridge.
//!
//! Both the authoritative server binary and the replication client binary
//! build on this module so their version domains, action schema, and
//! payload encoding cannot drift apart. The payload encoding is plain
//! `SimComponent` snapshot fields rendered through
//! [`SnapshotValue::to_json`](canary_state::SnapshotValue::to_json): the
//! same pure functions [`CollectathonDecoder`](crate::state::CollectathonDecoder)
//! territory uses, validated on the way back in by
//! [`SimComponent::apply_snapshot`](canary_runtime::SimComponent).
//!
//! The server is authoritative and this path carries no prediction or
//! rollback: the client converges on the server's canonical entries and
//! submits frame-tagged input for the server to validate (ADR 0027).

use std::collections::BTreeMap;
use std::fmt;

use canary_ecs::{CanaryComponent, Entity, Replicated, World};
use canary_net::{
    GameVersion, NetEntityId, NetEnvelope, NetError, NetLimits, NetRecv, NetSend, NetSequence,
    PluginApiVersion, ReplicatedEntry, SchemaManifestVersion, SequenceGate, SimTick,
    PROTOCOL_VERSION_1,
};
use canary_runtime::SimComponent;
use canary_state::{RemapTable, SnapshotValue, StateError};

use crate::{Pickup, Player, Score};

/// Schema-manifest version both ends of the collectathon session speak.
pub const SESSION_MANIFEST: SchemaManifestVersion = SchemaManifestVersion(1);
/// Game release both ends of the collectathon session run.
pub const SESSION_GAME: GameVersion = GameVersion(1);
/// Plugin/API surface both ends of the collectathon session support.
pub const SESSION_PLUGIN_API: PluginApiVersion = PluginApiVersion(1);
/// Assigned player slot for the collectathon session.
pub const SESSION_SLOT: u64 = 1;
/// Fresh session id minted per server run.
pub const SESSION_ID: u64 = 7;
/// Action schema the server accepts input for. The trailing `@1` pins the
/// single accepted payload version: [`ClientInput::action_version`](canary_net::ClientInput::action_version)
/// must equal it exactly.
pub const ACTION_SCHEMA: &str = "collectathon.input/move@1";
/// Accepted payload version, pinned by [`ACTION_SCHEMA`]'s suffix.
pub const ACTION_VERSION: u32 = 1;
/// Simulation tick of the authoritative snapshot.
pub const SNAPSHOT_TICK: SimTick = SimTick(1);
/// Simulation tick of the authoritative delta.
pub const DELTA_TICK: SimTick = SimTick(2);
/// Simulation tick the client's input targets and the ack confirms.
pub const INPUT_TICK: u64 = 3;

/// Typed failure for the replication harness binaries.
#[derive(Debug)]
#[non_exhaustive]
pub enum NetHarnessError {
    /// A `canary-net` operation failed.
    Net(#[allow(dead_code)] NetError),
    /// A snapshot encode/decode step failed.
    State(StateError),
    /// An ECS operation failed.
    Ecs(canary_ecs::EcsError),
    /// Local IO failed (identity files, stdout announcement).
    Io(std::io::Error),
    /// A payload was not valid JSON.
    Json(serde_json::Error),
    /// A socket address argument did not parse.
    Addr(std::net::AddrParseError),
    /// Usage or expectation failure with a human-readable message.
    Usage(String),
}

impl fmt::Display for NetHarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Net(error) => write!(f, "network harness failure: {error}"),
            Self::State(error) => write!(f, "snapshot harness failure: {error}"),
            Self::Ecs(error) => write!(f, "ECS harness failure: {error}"),
            Self::Io(error) => write!(f, "IO harness failure: {error}"),
            Self::Json(error) => write!(f, "payload JSON failure: {error}"),
            Self::Addr(error) => write!(f, "address parse failure: {error}"),
            Self::Usage(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for NetHarnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Net(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Ecs(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Addr(error) => Some(error),
            Self::Usage(_) => None,
        }
    }
}

impl From<NetError> for NetHarnessError {
    fn from(error: NetError) -> Self {
        Self::Net(error)
    }
}

impl From<StateError> for NetHarnessError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

impl From<canary_ecs::EcsError> for NetHarnessError {
    fn from(error: canary_ecs::EcsError) -> Self {
        Self::Ecs(error)
    }
}

impl From<std::io::Error> for NetHarnessError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for NetHarnessError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<std::net::AddrParseError> for NetHarnessError {
    fn from(error: std::net::AddrParseError) -> Self {
        Self::Addr(error)
    }
}

/// Issues a self-signed certificate for `localhost` and returns DER
/// certificate plus PKCS#8 key bytes. Development and proof use only: per
/// ADR 0027, QUIC TLS protects the bytes but does not decide who the peer
/// is, and certificate pinning on the client stays a proof-only peer policy.
pub fn localhost_identity() -> Result<(Vec<u8>, Vec<u8>), NetHarnessError> {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .map_err(|error| NetHarnessError::Usage(format!("rcgen self-signed identity: {error}")))?;
    Ok((
        certified.cert.der().to_vec(),
        certified.key_pair.serialize_der(),
    ))
}

/// Sends one checksummed envelope on its own wire sequence.
pub async fn send_envelope<S: NetSend>(
    send: &mut S,
    sequence: NetSequence,
    payload: Vec<u8>,
    limits: &NetLimits,
) -> Result<(), NetHarnessError> {
    let body = NetEnvelope::seal(PROTOCOL_VERSION_1, sequence, payload).encode(limits)?;
    send.send_frame(&body, limits).await?;
    Ok(())
}

/// Receives one envelope: checksum-verified decode, protocol agreement,
/// then duplicate/reorder rejection. Never applies a message twice.
pub async fn recv_envelope<R: NetRecv>(
    recv: &mut R,
    gate: &mut SequenceGate,
    limits: &NetLimits,
) -> Result<NetEnvelope, NetHarnessError> {
    let body = recv.recv_frame(limits).await?;
    let envelope = NetEnvelope::decode(&body)?;
    if envelope.protocol_version != PROTOCOL_VERSION_1 {
        return Err(NetError::UnsupportedProtocol {
            got: envelope.protocol_version.0,
            supported: PROTOCOL_VERSION_1.0,
        }
        .into());
    }
    gate.check(envelope.sequence)?;
    Ok(envelope)
}

/// Renders `SimComponent` snapshot fields as canonical payload bytes: the
/// fields map serialized directly, so every [`SnapshotValue`] arm
/// round-trips exactly (the JSON authored bridge would collapse `U64` into
/// `I64` — JSON has no integer-width contract — and `Score` decodes only
/// its exact arm). Key order follows the [`BTreeMap`], so the same
/// component state always encodes the same bytes.
pub fn fields_to_payload(
    fields: &BTreeMap<String, SnapshotValue>,
) -> Result<Vec<u8>, NetHarnessError> {
    Ok(serde_json::to_vec(fields)?)
}

/// Parses payload bytes back into snapshot fields, rejecting anything that
/// is not a fields map before typed decoding begins.
pub fn payload_to_fields(
    payload: &[u8],
) -> Result<BTreeMap<String, SnapshotValue>, NetHarnessError> {
    let fields: BTreeMap<String, SnapshotValue> = serde_json::from_slice(payload)?;
    Ok(fields)
}

/// Builds the authoritative entries for every entity carrying the
/// [`Replicated`] opt-in marker: one entry per present replicated schema,
/// holding that schema's `SimComponent` bytes. `entity_map` assigns the
/// server-scoped [`NetEntityId`] per live `(index, generation)` handle, so
/// recycled slots never alias.
pub fn snapshot_entries(
    world: &World,
    entity_map: &mut canary_net::NetEntityMap,
) -> Vec<ReplicatedEntry> {
    let mut remap = RemapTable::default();
    let mut entries = Vec::new();
    let replicated: Vec<Entity> = world
        .query::<Replicated>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in replicated {
        let net_id = entity_map.assign(entity.index(), entity.generation());
        if world.get::<Player>(entity).is_some() {
            if let Some(fields) = Player::write_snapshot(world, entity, &mut remap) {
                if let Ok(payload) = fields_to_payload(&fields) {
                    entries.push(ReplicatedEntry {
                        entity: net_id,
                        schema: Player::SCHEMA_ID.to_owned(),
                        payload,
                    });
                }
            }
        }
        if world.get::<Pickup>(entity).is_some() {
            if let Some(fields) = Pickup::write_snapshot(world, entity, &mut remap) {
                if let Ok(payload) = fields_to_payload(&fields) {
                    entries.push(ReplicatedEntry {
                        entity: net_id,
                        schema: Pickup::SCHEMA_ID.to_owned(),
                        payload,
                    });
                }
            }
        }
        if world.get::<Score>(entity).is_some() {
            if let Some(fields) = Score::write_snapshot(world, entity, &mut remap) {
                if let Ok(payload) = fields_to_payload(&fields) {
                    entries.push(ReplicatedEntry {
                        entity: net_id,
                        schema: Score::SCHEMA_ID.to_owned(),
                        payload,
                    });
                }
            }
        }
    }
    entries
}

/// Applies one converged entry to the client's world: typed-decodes the
/// payload through the schema's `SimComponent` and inserts it on the local
/// entity for `net_id`, spawning that entity on first sight. Our components
/// carry no entity references, so the resolve closure accepts none.
pub fn apply_entry_to_world(
    world: &mut World,
    local: &mut BTreeMap<u64, Entity>,
    net_id: NetEntityId,
    schema: &str,
    payload: &[u8],
) -> Result<(), NetHarnessError> {
    let fields = payload_to_fields(payload)?;
    let resolve = |_: u32| None;
    let entity = match local.get(&net_id.0) {
        Some(entity) => *entity,
        None => {
            let entity = world.spawn();
            local.insert(net_id.0, entity);
            entity
        }
    };
    if schema == Player::SCHEMA_ID {
        let component = Player::apply_snapshot(&fields, &resolve)?;
        world.insert(entity, component)?;
    } else if schema == Pickup::SCHEMA_ID {
        let component = Pickup::apply_snapshot(&fields, &resolve)?;
        world.insert(entity, component)?;
    } else if schema == Score::SCHEMA_ID {
        let component = Score::apply_snapshot(&fields, &resolve)?;
        world.insert(entity, component)?;
    } else {
        return Err(NetHarnessError::Usage(format!(
            "client converged on an unregistered schema '{schema}'"
        )));
    }
    Ok(())
}

/// Protocol version both ends check first (re-exported for call sites).
pub use canary_net::PROTOCOL_VERSION_1 as SESSION_PROTOCOL;
