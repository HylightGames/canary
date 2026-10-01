//! Actor identity: server-minted IDs, server-owned roles, bounded op IDs.
//!
//! A request never carries identity proof: the wire carries a pre-shared
//! credential (dev provisioning), and the server maps it to an
//! [`ActorId`] plus a [`Role`]. Any client-supplied actor name or role
//! claim is ignored — there is no field for one.

use crate::error::RejectCode;

/// Server-minted actor identity. Minted at provisioning time, never
/// accepted from the wire.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ActorId(pub u64);

/// The three server-owned roles of the first slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Role {
    /// Full edit rights (grant/revoke stays provisioning-time in `.16`).
    Owner,
    /// May submit the `.16` operation on authorized targets.
    Editor,
    /// Read-only: observes history and snapshots, submits nothing.
    Reader,
}

impl Role {
    /// Whether this role may submit edit operations.
    #[must_use]
    pub fn can_submit(&self) -> bool {
        matches!(self, Self::Owner | Self::Editor)
    }
}

/// Maximum wire length of a client operation ID, in bytes.
pub const MAX_CLIENT_OP_ID_BYTES: usize = 128;

/// Client-generated operation ID, scoped to its authenticated actor.
///
/// Bounded and charset-restricted so it is safe to index, log, and echo:
/// non-empty, at most [`MAX_CLIENT_OP_ID_BYTES`] bytes, ASCII
/// alphanumeric plus `-_.:`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ClientOpId(String);

impl ClientOpId {
    /// Validates `raw` into an operation ID.
    pub fn parse(raw: &str) -> Result<Self, RejectCode> {
        if raw.is_empty() || raw.len() > MAX_CLIENT_OP_ID_BYTES {
            return Err(RejectCode::Malformed);
        }
        let shaped = raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
        if !shaped {
            return Err(RejectCode::Malformed);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The validated ID text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_owner_and_editor_may_submit() {
        assert!(Role::Owner.can_submit());
        assert!(Role::Editor.can_submit());
        assert!(!Role::Reader.can_submit());
    }

    #[test]
    fn op_id_bounds_hold() {
        assert!(ClientOpId::parse("op-1_plain.2:3").is_ok());
        assert_eq!(ClientOpId::parse(""), Err(RejectCode::Malformed));
        assert_eq!(ClientOpId::parse("has space"), Err(RejectCode::Malformed));
        assert_eq!(
            ClientOpId::parse("../../../etc"),
            Err(RejectCode::Malformed)
        );
        let long = "o".repeat(MAX_CLIENT_OP_ID_BYTES + 1);
        assert_eq!(ClientOpId::parse(&long), Err(RejectCode::Malformed));
        let at_bound = "o".repeat(MAX_CLIENT_OP_ID_BYTES);
        assert!(ClientOpId::parse(&at_bound).is_ok());
    }
}
