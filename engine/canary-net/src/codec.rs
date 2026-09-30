//! Per-schema payload codecs for component bytes inside deltas (ADR 0027,
//! point 3).
//!
//! [`ReplicatedEntry`](crate::replication::ReplicatedEntry) payloads travel
//! as opaque bytes; [`SchemaCodecs`] maps each stable schema id to the typed
//! encode/decode functions that produce and interpret those bytes. The
//! server registers every schema it publishes, the client registers every
//! schema it understands, and a message naming an unregistered schema fails
//! the whole message as [`NetError::UnknownSchema`] — a resync on the live
//! connection, never a panic, never a partial apply.
//!
//! Codecs are plain byte functions, not trait objects over component types:
//! `canary-net` owns no component types, so typed (de)serialization lives
//! with the schema owner and is adapted to [`EncodeFn`] / [`DecodeFn`] at
//! registration.

use std::collections::BTreeMap;

use crate::error::NetError;
use crate::ids::NetEntityId;
use crate::replication::Delta;

/// Encodes validated component data into wire payload bytes for one schema.
pub type EncodeFn = Box<dyn Fn(&[u8]) -> Result<Vec<u8>, NetError> + Send + Sync + 'static>;
/// Decodes wire payload bytes into validated component data for one schema.
pub type DecodeFn = Box<dyn Fn(&[u8]) -> Result<Vec<u8>, NetError> + Send + Sync + 'static>;

/// One schema's codec pair: how its component bytes are produced and
/// interpreted on the wire.
pub struct SchemaCodec {
    /// Produces wire bytes from component data.
    encode: EncodeFn,
    /// Interprets wire bytes into component data.
    decode: DecodeFn,
}

impl SchemaCodec {
    /// Pairs `encode` with `decode` for one schema.
    pub fn new(
        encode: impl Fn(&[u8]) -> Result<Vec<u8>, NetError> + Send + Sync + 'static,
        decode: impl Fn(&[u8]) -> Result<Vec<u8>, NetError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            encode: Box::new(encode),
            decode: Box::new(decode),
        }
    }

    /// Encodes `data` into wire payload bytes.
    pub fn encode(&self, data: &[u8]) -> Result<Vec<u8>, NetError> {
        (self.encode)(data)
    }

    /// Decodes wire `payload` into component data.
    pub fn decode(&self, payload: &[u8]) -> Result<Vec<u8>, NetError> {
        (self.decode)(payload)
    }
}

impl std::fmt::Debug for SchemaCodec {
    /// Codec closures have no debug form; only the presence is reported.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SchemaCodec")
            .finish_non_exhaustive()
    }
}

/// Builds an identity [`SchemaCodec`]: wire bytes are the component data.
///
/// Test and proof helper for schemas with no real encoding yet. Real schemas
/// adapt typed (de)serialization to [`EncodeFn`] / [`DecodeFn`] (or
/// [`SchemaCodec::new`]) at registration.
#[must_use]
pub fn identity_codec() -> SchemaCodec {
    SchemaCodec::new(
        |data: &[u8]| Ok(data.to_vec()),
        |payload: &[u8]| Ok(payload.to_vec()),
    )
}

/// Registry of schema id to payload codec.
///
/// Deterministic iteration (`BTreeMap`): two peers with the same
/// registrations walk schemas in the same order.
#[derive(Default)]
pub struct SchemaCodecs {
    /// Registered codecs by stable schema id.
    codecs: BTreeMap<String, SchemaCodec>,
}

impl SchemaCodecs {
    /// An empty registry: every schema is unknown until registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            codecs: BTreeMap::new(),
        }
    }

    /// Registers the codec for `schema_id`. Returns `true` when newly
    /// added, `false` when it replaces an existing registration
    /// (re-registration is explicit and total — there is no partial merge
    /// of an old encode with a new decode).
    pub fn register(&mut self, schema_id: &str, codec: SchemaCodec) -> bool {
        self.codecs.insert(schema_id.to_owned(), codec).is_none()
    }

    /// Whether `schema_id` has a registered codec.
    #[must_use]
    pub fn contains(&self, schema_id: &str) -> bool {
        self.codecs.contains_key(schema_id)
    }

    /// Registered schema ids in deterministic (lexicographic) order.
    pub fn schemas(&self) -> impl Iterator<Item = &str> {
        self.codecs.keys().map(String::as_str)
    }

    /// How many schemas are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.codecs.len()
    }

    /// Whether any schema is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.codecs.is_empty()
    }

    /// Encodes `data` into wire payload bytes via the schema's codec.
    /// Fails as [`NetError::UnknownSchema`] when `schema_id` is unregistered.
    pub fn encode(&self, schema_id: &str, data: &[u8]) -> Result<Vec<u8>, NetError> {
        let codec = self
            .codecs
            .get(schema_id)
            .ok_or_else(|| NetError::UnknownSchema {
                schema: schema_id.to_owned(),
            })?;
        codec.encode(data)
    }

    /// Decodes wire `payload` via the schema's codec. Fails as
    /// [`NetError::UnknownSchema`] when `schema_id` is unregistered.
    pub fn decode(&self, schema_id: &str, payload: &[u8]) -> Result<Vec<u8>, NetError> {
        let codec = self
            .codecs
            .get(schema_id)
            .ok_or_else(|| NetError::UnknownSchema {
                schema: schema_id.to_owned(),
            })?;
        codec.decode(payload)
    }

    /// Decodes every change payload in `delta` through its schema's codec.
    ///
    /// Validates all payloads before returning any: the first unknown
    /// schema (or codec failure) rejects the whole delta, so the caller
    /// never applies a prefix of it. The returned triples are in the
    /// delta's canonical `(entity, schema)` order.
    pub fn decode_delta_payloads(
        &self,
        delta: &Delta,
    ) -> Result<Vec<(NetEntityId, String, Vec<u8>)>, NetError> {
        let mut decoded = Vec::with_capacity(delta.changes.len());
        for entry in &delta.changes {
            let data = self.decode(&entry.schema, &entry.payload)?;
            decoded.push((entry.entity, entry.schema.clone(), data));
        }
        Ok(decoded)
    }
}

impl std::fmt::Debug for SchemaCodecs {
    /// Lists registered schema ids only: codec closures have no debug form.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SchemaCodecs")
            .field("schemas", &self.codecs.keys().collect::<Vec<&String>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{NetSequence, SimTick};
    use crate::limits::NetLimits;
    use crate::replication::{encode_delta, ReplicatedEntry};

    fn registry() -> SchemaCodecs {
        let mut codecs = SchemaCodecs::new();
        codecs.register("canary.health", identity_codec());
        codecs
    }

    #[test]
    fn registered_schema_round_trips_through_its_codec() {
        let codecs = registry();
        assert!(codecs.contains("canary.health"));
        assert!(!codecs.contains("canary.velocity"));
        assert_eq!(codecs.len(), 1);
        assert!(!codecs.is_empty());
        assert_eq!(
            codecs.encode("canary.health", b"hp:100").expect("encode"),
            b"hp:100"
        );
        assert_eq!(
            codecs.decode("canary.health", b"hp:100").expect("decode"),
            b"hp:100"
        );
    }

    #[test]
    fn unknown_schema_is_a_resync_not_a_panic() {
        let codecs = registry();
        let error = codecs
            .decode("canary.velocity", b"v")
            .expect_err("unregistered");
        assert!(matches!(error, NetError::UnknownSchema { .. }));
        assert!(error.resync_required());
        assert_eq!(
            error.disconnect_reason(),
            crate::error::DisconnectReason::NeedsResync
        );
        assert!(!error.is_retryable());
        let error = codecs
            .encode("canary.velocity", b"v")
            .expect_err("unregistered");
        assert!(error.resync_required());
    }

    #[test]
    fn delta_walk_rejects_the_whole_delta_on_one_unknown_schema() {
        let codecs = registry();
        let limits = NetLimits::default();
        let bytes = encode_delta(
            NetSequence(2),
            NetSequence(1),
            SimTick(9),
            vec![
                ReplicatedEntry {
                    entity: NetEntityId(1),
                    schema: "canary.health".to_string(),
                    payload: b"hp:100".to_vec(),
                },
                ReplicatedEntry {
                    entity: NetEntityId(2),
                    schema: "canary.velocity".to_string(),
                    payload: b"v:3".to_vec(),
                },
            ],
            Vec::new(),
            &limits,
        )
        .expect("encode");
        let delta = crate::replication::decode_delta(&bytes).expect("decode");
        // One unknown schema rejects everything: the caller gets an error,
        // not a prefix of decoded payloads to half-apply.
        let error = codecs
            .decode_delta_payloads(&delta)
            .expect_err("unknown schema");
        assert!(matches!(
            &error,
            NetError::UnknownSchema { schema } if schema == "canary.velocity"
        ));
        assert!(error.resync_required());
    }

    #[test]
    fn delta_walk_returns_canonical_triples_when_all_schemas_known() {
        let mut codecs = registry();
        codecs.register("canary.velocity", identity_codec());
        let limits = NetLimits::default();
        let bytes = encode_delta(
            NetSequence(2),
            NetSequence(1),
            SimTick(9),
            vec![ReplicatedEntry {
                entity: NetEntityId(1),
                schema: "canary.health".to_string(),
                payload: b"hp:100".to_vec(),
            }],
            Vec::new(),
            &limits,
        )
        .expect("encode");
        let delta = crate::replication::decode_delta(&bytes).expect("decode");
        let decoded = codecs.decode_delta_payloads(&delta).expect("decode all");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].0, NetEntityId(1));
        assert_eq!(decoded[0].1, "canary.health");
        assert_eq!(decoded[0].2, b"hp:100");
    }

    #[test]
    fn registration_replaces_wholesale_and_reports_it() {
        let mut codecs = registry();
        assert!(!codecs.register("canary.health", identity_codec()));
        assert_eq!(codecs.len(), 1);
        assert_eq!(Vec::from_iter(codecs.schemas()), vec!["canary.health"]);
    }
}
