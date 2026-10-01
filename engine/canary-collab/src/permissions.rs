//! Server-side permission storage and out-of-protocol provisioning.
//!
//! The [`PermissionStore`] holds roles plus the session fencing epoch in a
//! server-side JSON file written with atomic-replace semantics (sibling
//! temp file, flush, rename — the same crash contract as the authored
//! save). Roles are provisioned out-of-protocol: the server operator maps
//! pre-shared credentials to `(actor, role)` in server configuration, and
//! the wire credential only ever looks that mapping up. There is no
//! in-protocol grant/revoke in `.16`.

use std::collections::BTreeMap;
use std::path::Path;

use canary_state::atomic_write;

use crate::actor::{ActorId, Role};
use crate::error::PermissionError;

/// Session fencing epoch: one per server generation.
///
/// The session admits a single epoch for accepted operations; every server
/// start over an existing permission file bumps it, so a restarted server
/// never shares an epoch with its pre-crash generation. Accepted envelopes
/// carry the epoch they were sequenced under.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    Default,
)]
pub struct SessionEpoch(pub u64);

/// One credential's provisioned principal, as written in server config.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CredentialBinding {
    /// Server-minted actor for this credential.
    pub actor: ActorId,
    /// Server-owned role for this credential.
    pub role: Role,
}

/// Server-side roles plus fencing epoch.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PermissionStore {
    /// Current fencing epoch.
    #[serde(default)]
    pub epoch: SessionEpoch,
    /// Authoritative role per actor. The wire never writes this map.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<ActorId, Role>,
}

impl PermissionStore {
    /// Builds a store from provisioning config: every binding becomes an
    /// entry, and the returned authenticator maps each credential to its
    /// `(actor, role)` for request-time lookup.
    ///
    /// Two credentials MUST NOT claim the same actor: the second binding
    /// fails with [`PermissionError::DuplicateActor`] and the first
    /// binding stands — last-credential-wins would let a misconfigured
    /// credential silently steal an actor's role. (`credentials` iterates
    /// in lexicographic order, so "first" is deterministic: the lowest
    /// credential string.) There is no in-protocol grant/revoke; this is
    /// provisioning-time-only.
    pub fn provision(
        credentials: &BTreeMap<String, CredentialBinding>,
    ) -> Result<(Self, BTreeMap<String, (ActorId, Role)>), PermissionError> {
        let mut roles = BTreeMap::new();
        let mut authenticator = BTreeMap::new();
        for (credential, binding) in credentials {
            if roles.contains_key(&binding.actor) {
                return Err(PermissionError::DuplicateActor {
                    actor: binding.actor.0,
                    credential: credential.clone(),
                });
            }
            roles.insert(binding.actor, binding.role);
            authenticator.insert(credential.clone(), (binding.actor, binding.role));
        }
        Ok((
            Self {
                epoch: SessionEpoch(1),
                roles,
            },
            authenticator,
        ))
    }

    /// The authoritative role for `actor`, if provisioned.
    #[must_use]
    pub fn role_of(&self, actor: &ActorId) -> Option<Role> {
        self.roles.get(actor).copied()
    }

    /// Opens (or creates) the server-side permission file at `path`.
    ///
    /// A missing file provisions from `credentials` at epoch 1 and
    /// persists. An existing file loads the authoritative roles, merges
    /// in any newly provisioned credentials for unknown actors, bumps
    /// the fencing epoch for the new server generation, and persists.
    /// Config never demotes or removes a persisted role: the file wins
    /// for known actors.
    ///
    /// A duplicate-actor collision in `credentials` fails before the file
    /// is read or written: the persisted store is left untouched.
    pub fn open_or_provision(
        path: &Path,
        credentials: &BTreeMap<String, CredentialBinding>,
    ) -> Result<(Self, BTreeMap<String, (ActorId, Role)>), PermissionError> {
        let (mut store, mut authenticator) = Self::provision(credentials)?;
        if path.exists() {
            let text = std::fs::read_to_string(path).map_err(|error| PermissionError::File {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
            let persisted: PermissionStore =
                serde_json::from_str(&text).map_err(|error| PermissionError::Codec {
                    path: path.to_path_buf(),
                    reason: error.to_string(),
                })?;
            for (actor, role) in &persisted.roles {
                authenticator
                    .iter_mut()
                    .filter(|(_, bound)| bound.0 == *actor)
                    .for_each(|(_, bound)| bound.1 = *role);
            }
            store.roles = persisted.roles;
            for (actor, role) in authenticator.values() {
                store.roles.entry(*actor).or_insert(*role);
            }
            store.epoch = SessionEpoch(persisted.epoch.0.saturating_add(1));
        }
        store.save(path)?;
        Ok((store, authenticator))
    }

    /// Atomically persists the store through the shared
    /// [`atomic_write`](canary_state::atomic_write) helper: complete bytes
    /// to a sibling temp file, flush, then rename. A crash before the
    /// rename leaves the previous file untouched. The temp suffix is
    /// permission-specific so this file can never share a sibling temp
    /// name with a project file saved alongside it.
    pub fn save(&self, path: &Path) -> Result<(), PermissionError> {
        let text = serde_json::to_string_pretty(self).map_err(|error| PermissionError::Codec {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
        atomic_write(path, text.as_bytes(), PERMISSION_TEMP_SUFFIX)?;
        Ok(())
    }
}

/// Temp suffix for the permission file (see
/// [`atomic_write`](canary_state::atomic_write)): distinct from the
/// project-file suffix so the two files never share a sibling temp name.
const PERMISSION_TEMP_SUFFIX: &str = ".permissions-tmp";

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn credentials() -> BTreeMap<String, CredentialBinding> {
        BTreeMap::from([
            (
                "owner-secret".to_owned(),
                CredentialBinding {
                    actor: ActorId(1),
                    role: Role::Owner,
                },
            ),
            (
                "editor-secret".to_owned(),
                CredentialBinding {
                    actor: ActorId(2),
                    role: Role::Editor,
                },
            ),
            (
                "reader-secret".to_owned(),
                CredentialBinding {
                    actor: ActorId(3),
                    role: Role::Reader,
                },
            ),
        ])
    }

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "canary-collab-{}-{}-{tag}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("permissions.json")
    }

    #[test]
    fn provisioning_maps_credentials_to_principals() {
        let (store, authenticator) =
            PermissionStore::provision(&credentials()).expect("distinct test actors provision");
        assert_eq!(store.epoch, SessionEpoch(1));
        assert_eq!(store.role_of(&ActorId(1)), Some(Role::Owner));
        assert_eq!(store.role_of(&ActorId(2)), Some(Role::Editor));
        assert_eq!(store.role_of(&ActorId(3)), Some(Role::Reader));
        assert_eq!(
            authenticator["owner-secret"],
            (ActorId(1), Role::Owner),
            "wire credential resolves without any client claim"
        );
        assert!(store.role_of(&ActorId(99)).is_none());
    }

    #[test]
    fn duplicate_actor_binding_fails_typed_with_the_first_binding_intact() {
        // Two credentials claiming the same ActorId must fail with a
        // typed error, not resolve to last-credential-wins. The error
        // names the later credential; the first binding stands.
        let dup = BTreeMap::from([
            (
                "first-secret".to_owned(),
                CredentialBinding {
                    actor: ActorId(1),
                    role: Role::Owner,
                },
            ),
            (
                "second-secret".to_owned(),
                CredentialBinding {
                    actor: ActorId(1),
                    role: Role::Editor,
                },
            ),
        ]);
        match PermissionStore::provision(&dup) {
            Err(PermissionError::DuplicateActor { actor, credential }) => {
                assert_eq!(actor, 1, "the contested actor is reported");
                assert_eq!(
                    credential, "second-secret",
                    "the later credential is the one rejected"
                );
            }
            other => panic!("duplicate actor must fail DuplicateActor, got {other:?}"),
        }

        // The same collision over an existing permission file fails
        // before the file is read or written: the persisted first
        // binding (and its epoch) is left untouched.
        let path = scratch("dup-intact");
        let (first, _) =
            PermissionStore::open_or_provision(&path, &credentials()).expect("first provision");
        assert_eq!(first.role_of(&ActorId(1)), Some(Role::Owner));
        let before = std::fs::read(&path).expect("read persisted file");
        match PermissionStore::open_or_provision(&path, &dup) {
            Err(PermissionError::DuplicateActor { actor, .. }) => assert_eq!(actor, 1),
            other => panic!("duplicate actor must fail DuplicateActor, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).expect("reread persisted file"),
            before,
            "a rejected provisioning leaves the permission file byte-identical"
        );
        let (reopened, _) =
            PermissionStore::open_or_provision(&path, &credentials()).expect("reopen");
        assert_eq!(reopened.role_of(&ActorId(1)), Some(Role::Owner));
        assert_eq!(
            reopened.epoch,
            SessionEpoch(2),
            "only the successful reopen bumped the epoch — the rejected \
             attempt advanced nothing"
        );
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn restart_keeps_roles_and_bumps_the_epoch() {
        let path = scratch("epoch");
        let (first, _) =
            PermissionStore::open_or_provision(&path, &credentials()).expect("first provision");
        assert_eq!(first.epoch, SessionEpoch(1));
        assert!(path.exists(), "provisioning persists the store");

        let (second, authenticator) =
            PermissionStore::open_or_provision(&path, &credentials()).expect("reopen");
        assert_eq!(second.epoch, SessionEpoch(2), "new generation, new epoch");
        assert_eq!(second.role_of(&ActorId(2)), Some(Role::Editor));
        assert_eq!(authenticator["reader-secret"], (ActorId(3), Role::Reader));
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }

    #[test]
    fn save_shares_no_temp_file_with_a_project_save_beside_it() {
        // The permission store persists through the shared
        // `canary-state` atomic-write helper with its own temp suffix:
        // a project file saved in the same directory must never collide
        // with it, and a stale temp from a killed writer is overwritten,
        // never merged.
        let path = scratch("shared-tmp");
        let dir = path.parent().expect("parent").to_path_buf();
        let project_path = dir.join("project.json");
        canary_state::atomic_write(&project_path, b"{}", ".tmp").expect("project save");

        let (store, _) =
            PermissionStore::provision(&credentials()).expect("distinct test actors provision");
        // A killed writer's partial temp: the next save must consume it.
        let stale = dir.join("permissions.json.permissions-tmp");
        std::fs::write(&stale, b"{partial").expect("plant stale tmp");
        store.save(&path).expect("save");

        assert!(!stale.exists(), "rename consumes the temp file");
        canary_state::atomic_write(&project_path, b"{}", ".tmp").expect("project save");
        assert_eq!(
            std::fs::read(&project_path).expect("read project"),
            b"{}",
            "the project save beside the permission file is untouched"
        );
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("list")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["permissions.json", "project.json"],
            "no stray temp file may survive beside the two saves: {names:?}"
        );
        let (reopened, _) =
            PermissionStore::open_or_provision(&path, &credentials()).expect("reopen");
        assert_eq!(reopened.role_of(&ActorId(2)), Some(Role::Editor));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn persisted_roles_win_over_reprovisioning() {
        let path = scratch("wins");
        let (mut store, _) =
            PermissionStore::open_or_provision(&path, &credentials()).expect("provision");
        // Operator-side change made directly to the file (the only writer).
        store.roles.insert(ActorId(2), Role::Reader);
        store.save(&path).expect("save");

        let (reopened, authenticator) =
            PermissionStore::open_or_provision(&path, &credentials()).expect("reopen");
        assert_eq!(
            reopened.role_of(&ActorId(2)),
            Some(Role::Reader),
            "the file is authoritative for known actors"
        );
        assert_eq!(
            authenticator["editor-secret"],
            (ActorId(2), Role::Reader),
            "lookup follows the file, not the stale config"
        );
        std::fs::remove_dir_all(path.parent().expect("parent")).ok();
    }
}
