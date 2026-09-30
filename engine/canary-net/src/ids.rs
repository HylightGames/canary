//! Network identity and version-domain newtypes (ADR 0027, points 5 and 7).
//!
//! Four domains that must never be confused: the session-scoped entity
//! identity assigned by the authoritative server, the per-connection message
//! sequence, the wire protocol version, and the schema/content manifest
//! version. Each is its own type so a function asking for one cannot silently
//! receive another, and none of them is the runtime ECS `Entity` handle —
//! runtime handles never cross the wire.

use serde::{Deserialize, Serialize};

/// Server-scoped identity for a replicated entity, assigned by the
/// authoritative session.
///
/// Distinct from the runtime ECS `Entity` handle (process-local, never
/// serialized), from authored project identity, and from content identity.
/// Stable for the session; a reconnect starts a fresh mapping unless the
/// server proves the old baseline is still retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NetEntityId(pub u64);

/// Explicit per-connection message sequence.
///
/// The receiver's [`crate::sequence::SequenceGate`] requires strictly
/// increasing values: a duplicate or reordered sequence is rejected, never
/// applied twice. Gaps (a jump forward) are accepted at this layer; the delta
/// layer above treats a missing base sequence as a resync trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NetSequence(pub u64);

/// Wire protocol version: the framing + envelope contract, nothing else.
///
/// Negotiated independently of schema, game content, and plugin/API versions
/// (ADR 0027, point 7). No compatibility is implied by sharing an engine
/// release number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProtocolVersion(pub u16);

/// Schema/content manifest version domain: which component schemas and game
/// content the peer understands.
///
/// Checked separately from [`ProtocolVersion`]; an unsupported required
/// schema version rejects the handshake before any state is applied. Carried
/// on the wire by [`crate::handshake::Hello::schema_manifest`] and gated by
/// [`crate::handshake::decide_handshake`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SchemaManifestVersion(pub u32);

/// Game release version domain: which game build's content the peer runs.
///
/// Negotiated independently of the wire [`ProtocolVersion`], the
/// [`SchemaManifestVersion`], and plugin/API versions (ADR 0027, point 7):
/// sharing an engine release number implies nothing about game-content
/// compatibility. Carried on the wire by
/// [`crate::handshake::Hello::game_version`]; a mismatch rejects the
/// handshake before any state is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GameVersion(pub u32);

/// Plugin/API surface version domain: which plugin interface version the
/// peer requires.
///
/// Negotiated independently of every other version domain (ADR 0027,
/// point 7). Carried on the wire by
/// [`crate::handshake::Hello::plugin_api_version`]; a mismatch rejects the
/// handshake before any state is applied, so a peer that needs a newer (or
/// older) plugin surface than the server supports never reaches the
/// simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PluginApiVersion(pub u32);

/// Authoritative simulation step count carried by snapshots and deltas.
///
/// The third counter in the three-domain disambiguation (ADR 0027 amendment
/// 4): the wire session sequence ([`NetSequence`]) orders messages on a
/// connection, [`SimTick`] counts simulation steps the authority has run,
/// and the scheduler `Tick` (see `canary-ecs`) stamps component writes for
/// `query_changed_since` dirty-set computation. A delta names its wire
/// [`NetSequence`] base *and* the [`SimTick`] its changes were captured at;
/// tombstones additionally record the scheduler-tick value so the server can
/// correlate them with per-client last-acknowledged ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SimTick(pub u64);

/// The wire protocol version this spike speaks.
pub const PROTOCOL_VERSION_1: ProtocolVersion = ProtocolVersion(1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_domains_do_not_compare_across_types() {
        // Compiles only because each domain is its own type: assert the
        // discriminating values ride along as expected.
        assert_eq!(NetEntityId(7).0, 7);
        assert_eq!(NetSequence(7).0, 7);
        assert_eq!(SimTick(7).0, 7);
        assert_eq!(ProtocolVersion(1), PROTOCOL_VERSION_1);
        assert_eq!(SchemaManifestVersion(3).0, 3);
        assert_eq!(GameVersion(7).0, 7);
        assert_eq!(PluginApiVersion(2).0, 2);
        assert!(GameVersion(1) < GameVersion(2));
        assert!(PluginApiVersion(1) < PluginApiVersion(2));
        assert!(NetSequence(1) < NetSequence(2));
    }
}
