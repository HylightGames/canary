//! Authored project state and deterministic simulation snapshots.
//!
//! Two products share one serialization vocabulary but never the same bytes:
//!
//! - [`authored`] — human-readable project files. Canonical pretty JSON with
//!   deterministic [`BTreeMap`](std::collections::BTreeMap) ordering, so diffs
//!   are stable and merges stay textual.
//! - [`snapshot`] — machine payloads for checksums and (later) netcode.
//!   `postcard` 1.x bytes with a canonical field order, checksummed with
//!   SHA-256 over the encoded payload.
//!
//! Both layers speak [`serde`] through versioned envelopes ([`schema`]) keyed
//! by [`SchemaId`](schema::SchemaId), [`SchemaVersion`](schema::SchemaVersion),
//! and [`EncodingVersion`](schema::EncodingVersion). Unknown fields survive a
//! load→save round trip as version-tagged [`serde_json::Value`]. Schema drift
//! is repaired by the linear per-schema chains in [`migration`].
//!
//! Identity ([`identity`]) keeps three domains apart: [`ProjectId`](identity::ProjectId)
//! (stable GUID for the authored project), canonical snapshot-local IDs
//! (deterministic per encode), and live runtime handles (never serialized).
//!
//! This crate is a leaf: it depends only on third-party codecs, never on
//! another `canary-*` crate. Callers supply their own record types; the
//! snapshot profile declares which component schemas participate.
//!
//! Encoding choice (JSON authored, postcard snapshots) is decided in
//! ADR 0026 ("Encoding selection", 2026-09-28); RON and TOML were rejected
//! there. The decision is closed for `v0.1.0` unless implementation evidence
//! contradicts the research.

pub mod authored;
pub mod codec;
pub mod error;
pub mod identity;
pub mod migration;
pub mod schema;
pub mod snapshot;
pub mod value;

pub use authored::{AuthoredChange, AuthoredDocument};
pub use codec::{AuthoredFormat, SnapshotFormat};
pub use error::StateError;
pub use identity::{ProjectId, ProjectRegistry};
pub use migration::{MigrationChain, MigrationError};
pub use schema::{AuthoredEnvelope, EncodingVersion, SchemaId, SchemaVersion, SnapshotEnvelope};
pub use snapshot::{
    decode_snapshot, encode_snapshot, OwnedRng, RemapTable, Snapshot, SnapshotChecksum,
    SnapshotProfile, SnapshotRecord,
};
pub use value::SnapshotValue;
