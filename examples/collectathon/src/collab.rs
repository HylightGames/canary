//! Authoritative collaboration session seam (`.16`).
//!
//! [`open_host`] opens (or provisions) the serving state over the room
//! project file: the durable project seam, the server-side permission
//! file, and the session over both. [`PLAYER_SECTION`] names the authored
//! object the first collaboration slice edits; later work packages run the
//! two-client edit/conflict/reconnect path against it.

use std::collections::BTreeMap;
use std::path::Path;

pub use canary_runtime::{CollabHostError, CollabSessionHost};

/// The authored section the first collaboration slice edits.
pub const PLAYER_SECTION: &str = "entity.player";

/// Opens the collaboration host over `project_path`, provisioning the
/// permission file at `permission_path` from `credentials` on first run.
/// Restarting over the same paths resumes sequences and bumps the fencing
/// epoch.
pub fn open_host(
    project_path: &Path,
    permission_path: &Path,
    credentials: &BTreeMap<String, canary_collab::CredentialBinding>,
) -> Result<CollabSessionHost, CollabHostError> {
    CollabSessionHost::open(project_path, permission_path, credentials)
}
