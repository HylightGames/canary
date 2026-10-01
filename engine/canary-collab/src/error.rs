//! Typed collaboration failures and stable rejection codes.
//!
//! A rejection is an observable result, never a state change: rejected
//! requests assign no sequence, advance no revision, and enter no history.
//! Error text is diagnostic only — the [`RejectCode`] is the wire contract.
//!
//! Permission-store failures ([`PermissionError`]) are host errors, not
//! wire errors: the session never sees them, and they never become
//! rejections.

use std::path::PathBuf;

use thiserror::Error;

/// Every failure the collaboration path can report.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CollabError {
    /// Wire bytes failed the bounded decode (over-long, truncated, or not
    /// postcard at all). The connection stays the host's decision; the
    /// session consumed nothing.
    #[error("collab wire decode failure: {0}")]
    Decode(String),

    /// Wire bytes exceeded the session's bound before decoding began.
    #[error("collab wire message claims {claimed} bytes, bound is {max}")]
    TooLarge {
        /// The offending byte length.
        claimed: usize,
        /// The bound it exceeded.
        max: usize,
    },

    /// A sync request carried an unknown credential. Nothing is returned;
    /// the cursor learns nothing about history.
    #[error("collab sync authentication failure")]
    Unauthenticated,

    /// A sync cursor claims a sequence newer than anything retained.
    #[error("collab sync cursor is ahead of retained history")]
    InvalidCursor,

    /// The canonical snapshot for a behind-horizon sync cursor could not
    /// be serialized. The cursor learns nothing; the session is
    /// unchanged. This is a host-side failure, never a wire rejection —
    /// a serialization failure is not a malformed request, so it must
    /// not travel as [`CollabError::Decode`].
    #[error("collab sync snapshot encode failure: {0}")]
    SnapshotEncode(String),
}

/// Why the server-side permission store failed to load or persist.
///
/// The permission file is server-owned state provisioned
/// out-of-protocol: failures here are host errors (the server operator's
/// problem), never rejections — no [`RejectCode`] exists for them and the
/// session is never told. Callers keep the typed path instead of a
/// string so a full disk and a corrupt file stay distinguishable.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PermissionError {
    /// The permission file (or its sibling temp file) could not be
    /// read, written, flushed, or renamed.
    #[error("permission file error at {path}: {reason}")]
    File {
        /// The file (or sibling temp) the operation targeted.
        path: PathBuf,
        /// What went wrong, in plain words.
        reason: String,
    },

    /// The permission file (or its in-memory rendering) was not valid
    /// JSON for [`PermissionStore`](crate::permissions::PermissionStore).
    #[error("permission codec failure at {path}: {reason}")]
    Codec {
        /// The file the bytes came from or were bound for.
        path: PathBuf,
        /// What the codec refused, in plain words.
        reason: String,
    },

    /// Two pre-shared credentials claim the same server-minted actor.
    /// Provisioning fails instead of letting the later credential
    /// silently steal the actor's role: the first binding stands and
    /// the collision is a host configuration error to fix, never a
    /// silent last-wins.
    ///
    /// The display names the actor only: the colliding credential is
    /// available on the typed variant for programmatic handling but is
    /// never echoed into logs or error text.
    #[error("duplicate provisioning for actor {actor}: collides with an earlier binding")]
    DuplicateActor {
        /// The server-minted actor value two credentials both claim.
        actor: u64,
        /// The later credential whose binding collided. Kept for
        /// programmatic handling; never rendered in the display text.
        credential: String,
    },
}

impl From<canary_state::StateError> for PermissionError {
    /// Maps the shared atomic-write helper's failures without
    /// flattening them to a string. That helper only ever fails as
    /// `StateError::File`; any other state failure (unreachable on
    /// this path today) maps to [`Self::Codec`] so no error kind is
    /// ever silently dropped.
    fn from(error: canary_state::StateError) -> Self {
        match error {
            canary_state::StateError::File { path, reason } => Self::File { path, reason },
            other => Self::Codec {
                path: PathBuf::from("<permission-store>"),
                reason: other.to_string(),
            },
        }
    }
}

/// Stable machine-readable reason for one rejected edit request.
///
/// The code is the contract; the accompanying message is diagnostic and
/// must not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RejectCode {
    /// The request failed framing, protocol-version, or value-shape
    /// checks before any domain validation could run.
    Malformed,
    /// The schema name is unknown to this build.
    UnknownSchema,
    /// The credential authenticates no provisioned actor.
    Unauthenticated,
    /// The actor's role may not submit edits. The connection stays alive
    /// and no trace of the attempt is recorded.
    Forbidden,
    /// The target section name is malformed or names no entity.
    UnknownTarget,
    /// The expected target revision is stale. Carries the canonical
    /// current revision and target state for refresh-and-resubmit.
    RevisionConflict,
    /// The payload failed numeric validation (non-finite float or a
    /// denormalized quaternion).
    InvalidPayload,
    /// The entity's prefab instance disallows a remote transform
    /// override (see the `canary-state` veto gate).
    PrefabOverrideDenied,
    /// The schema version has no migration path to this build's version.
    UnmigratableSchema,
    /// The durable commit failed. Nothing was broadcast, acknowledged, or
    /// recorded; the previous good revision stays recoverable.
    StorageFailed,
    /// The `(actor, client-op-id)` key is already recorded with a
    /// different payload. Repeating the identical request is idempotent;
    /// reusing the key is not.
    IdConflict,
    /// A length bound failed: an oversize request, or a reply whose
    /// fully-encoded form exceeds [`MAX_RESPONSE_BYTES`](crate::wire::MAX_RESPONSE_BYTES).
    /// Size failures are never [`Malformed`](RejectCode::Malformed): on the
    /// frame path [`Session::handle_frame_body`](crate::session::Session::handle_frame_body)
    /// answers them with this code under the echoed tag, so a snapshot that
    /// passes the content ceiling but encodes past the response bound stays
    /// typed instead of collapsing to a generic reject.
    TooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_actor_display_names_actor_but_not_credential() {
        let error = PermissionError::DuplicateActor {
            actor: 7,
            credential: "s3cr3t-collide".to_owned(),
        };
        let rendered = format!("{error}");
        assert!(
            rendered.contains('7'),
            "DuplicateActor display must name the actor: {rendered}"
        );
        assert!(
            !rendered.contains("s3cr3t-collide"),
            "DuplicateActor display must not echo the credential: {rendered}"
        );
    }
}
