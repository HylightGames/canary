//! Typed errors for project-state and snapshot operations.
//!
//! Codec failures from `serde_json` and `postcard` surface as owned variants
//! here, so no third-party error type appears in a public signature — the
//! same seam rule the rest of the workspace follows.

use std::path::PathBuf;
use thiserror::Error;

/// Every failure this crate can report.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StateError {
    /// An authored JSON file could not be read, written, or created.
    #[error("project file error at {path}: {reason}")]
    File {
        /// The file the operation targeted.
        path: PathBuf,
        /// What went wrong, in plain words.
        reason: String,
    },

    /// `serde_json` refused to parse or emit an authored document.
    #[error("authored JSON codec failure: {0}")]
    AuthoredJson(#[from] serde_json::Error),

    /// `postcard` refused to encode or decode a snapshot payload.
    #[error("snapshot postcard codec failure: {0}")]
    SnapshotCodec(#[from] postcard::Error),

    /// The envelope names an encoding this build does not implement.
    #[error("unsupported encoding version {found}, newest supported is {supported}")]
    UnsupportedEncoding {
        /// The version the payload claims.
        found: u32,
        /// The newest version this build reads.
        supported: u32,
    },

    /// The envelope names a schema the caller never registered.
    #[error("unknown schema '{0}'")]
    UnknownSchema(String),

    /// A registered schema has no migration path to the requested version.
    #[error("no migration path for schema '{schema}' from v{from} to v{to}")]
    NoMigrationPath {
        /// The schema that cannot advance.
        schema: String,
        /// The version the payload carries.
        from: u32,
        /// The version the caller wants.
        to: u32,
    },

    /// A migration step ran but the payload still fails its own validation.
    #[error("migration of schema '{schema}' to v{to} produced an invalid payload: {reason}")]
    MigrationInvalid {
        /// The schema that failed to validate after migration.
        schema: String,
        /// The version the migration targeted.
        to: u32,
        /// Why the result is invalid.
        reason: String,
    },

    /// A checksum comparison failed: the payload is corrupt or not canonical.
    #[error("snapshot checksum mismatch: expected {expected}, computed {computed}")]
    ChecksumMismatch {
        /// The checksum the envelope carries.
        expected: String,
        /// The checksum recomputed over the payload.
        computed: String,
    },

    /// A snapshot record references a component schema outside the profile.
    #[error("snapshot record uses undeclared component schema '{0}'")]
    UndeclaredComponent(String),

    /// OS randomness was unavailable while minting an identity.
    /// (`getrandom::Error` implements no `std::error::Error`, so the
    /// message is captured as text at the boundary.)
    #[error("randomness unavailable: {0}")]
    Randomness(String),

    /// An authored asset marker named a logical ID no resolver satisfies.
    #[error("unresolved asset reference '{id}'")]
    AssetUnresolved {
        /// The logical asset ID the document asked for.
        id: String,
    },

    /// A staged component passed validation but its typed insert failed
    /// during apply. The leaf cannot name the ECS error type, so the cause
    /// arrives as text; the entity is the fresh runtime handle (debug
    /// rendering) that rejected the insert.
    #[error("spawn placement failed for entity '{entity}': {reason}")]
    PlacementFailed {
        /// Debug rendering of the fresh entity that rejected the insert.
        entity: String,
        /// Why the insert failed.
        reason: String,
    },

    /// A snapshot entity reference names a snapshot-local ID that has no
    /// record in the same snapshot. Checked at restore time (before the
    /// first world mutation), never at capture: capture only assigns IDs.
    #[error("unresolvable snapshot entity reference to snapshot-local id {id}")]
    UnresolvableEntityRef {
        /// The snapshot-local ID no record provides.
        id: u32,
    },
}
