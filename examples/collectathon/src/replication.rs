//! Networking seam: replicated schemas, wire codecs, opt-in marking.
//!
//! [`REPLICATED_SCHEMAS`] is the type-level opt-in (`.15`): a component
//! replicates for an entity only when the entity carries [`Replicated`]
//! *and* its schema is registered here. [`replication_codecs`] builds the
//! matching wire-codec registry; [`mark_replicated`] marks the entity-level
//! half. Later work packages run the server/client processes against this
//! mapping.
//!
//! [`Replicated`]: canary_ecs::Replicated

use canary_ecs::{CanaryComponent, EcsError, Entity, Replicated, World};
use canary_net::{identity_codec, ReplicationRegistry, SchemaCodecs};

use crate::{Pickup, Player, Score};

/// Schemas the server may publish, in deterministic order.
pub const REPLICATED_SCHEMAS: &[&str] = &[Player::SCHEMA_ID, Pickup::SCHEMA_ID, Score::SCHEMA_ID];

/// Builds the wire-codec registry for the replicated schemas. Identity
/// codecs are the documented proof placeholder: real schemas adapt typed
/// (de)serialization at registration once the wire format review lands.
#[must_use]
pub fn replication_codecs() -> SchemaCodecs {
    let mut codecs = SchemaCodecs::new();
    for schema in REPLICATED_SCHEMAS {
        codecs.register(schema, identity_codec());
    }
    codecs
}

/// Builds the per-schema replication opt-in registry.
#[must_use]
pub fn replication_policy() -> ReplicationRegistry {
    let mut policy = ReplicationRegistry::new();
    for schema in REPLICATED_SCHEMAS {
        policy.register(schema);
    }
    policy
}

/// Marks `entity` as a replication candidate. Local-only entities (HUD
/// scratch, client prediction state) stay unmarked.
pub fn mark_replicated(world: &mut World, entity: Entity) -> Result<(), EcsError> {
    world.insert(entity, Replicated)?;
    Ok(())
}
