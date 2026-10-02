//! `collectathon_collab_client`: a two-client shared-authored-edit CLI.
//!
//! Drives the authoritative collaboration path through the public
//! [`CollabSessionHost`](collectathon::collab::CollabSessionHost) seam: the
//! CLI seeds a room project carrying `entity.*` sections with
//! `canary.transform` blocks, opens the host over three credentials
//! (owner/editor/reader), and plays two logical clients against it — an
//! owner edit (accepted), a reader edit (denied; reader is read-only), a
//! stale editor edit (revision conflict with a canonical refresh), a fresh
//! editor edit (accepted), and a reader sync (incremental tail). Edit
//! requests travel as [`encode_request`](canary_collab::encode_request)
//! bytes and tagged [`tag_edit_body`](canary_collab::tag_edit_body) frames;
//! replies decode through
//! [`decode_response`](canary_collab::decode_response).
//!
//! Gameplay state never enters this path: score and pickup data live in the
//! replicated simulation world (`.15`), while collaboration edits only ever
//! name authored `canary.transform` sections. The CLI asserts that
//! separation on the durable file before exiting.
//!
//! ```sh
//! cargo run -p collectathon --bin collectathon_collab_client
//! cargo run -p collectathon --bin collectathon_collab_client -- --project /tmp/room.json --permissions /tmp/perms.json
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use canary_collab::{
    ActorId, CollabError, CredentialBinding, EditRequest, RejectCode, Role, SyncRequest,
    TransformPayload, WireResponse, COLLAB_PROTOCOL_VERSION,
};
use canary_state::{AuthoredDocument, ProjectId, StateError, TRANSFORM_SCHEMA_KEY};
use collectathon::collab::{open_host, PLAYER_SECTION};

/// Typed failure for the collaboration CLI.
#[derive(Debug)]
enum CollabCliError {
    /// A collaboration operation failed.
    Collab(CollabError),
    /// Project file load/save failed.
    State(StateError),
    /// Local IO failed.
    Io(std::io::Error),
    /// Usage or expectation failure with a human-readable message.
    Usage(String),
}

impl fmt::Display for CollabCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collab(error) => write!(f, "collaboration failure: {error}"),
            Self::State(error) => write!(f, "project failure: {error}"),
            Self::Io(error) => write!(f, "IO failure: {error}"),
            Self::Usage(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for CollabCliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Collab(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Usage(_) => None,
        }
    }
}

impl From<CollabError> for CollabCliError {
    fn from(error: CollabError) -> Self {
        Self::Collab(error)
    }
}

impl From<StateError> for CollabCliError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

impl From<std::io::Error> for CollabCliError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn print_usage() {
    println!(
        "collectathon_collab_client: shared-authored-edit CLI\n\
         \n\
         usage: collectathon_collab_client [--project PATH] [--permissions PATH]\n\
         \n\
         Seeds PATH with the authored room transforms when missing, opens the\n\
         collaboration host, and plays two logical clients through the\n\
         edit/conflict/reconnect path (owner accept, reader deny, stale\n\
         conflict, fresh accept, sync tail). Defaults live under the temp dir."
    );
}

/// The three credentials this CLI serves: owner, editor, and reader.
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

/// Seeds the authored room project: `entity.player` (the section the first
/// slice edits) plus `entity.shard_a`, each carrying exactly one
/// `canary.transform` block. No gameplay component (`collectathon.score`,
/// `collectathon.pickup`, `collectathon.player`) appears here by design.
fn seed_project(path: &std::path::Path) -> Result<(), CollabCliError> {
    if path.exists() {
        return Ok(());
    }
    let project = ProjectId::generate()?;
    let mut document = AuthoredDocument::new(project);
    for (section, x) in [(PLAYER_SECTION, 0.0), ("entity.shard_a", -60.0)] {
        document.sections.insert(
            section.to_owned(),
            serde_json::json!({
                TRANSFORM_SCHEMA_KEY: {
                    "translation": [x, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                },
            }),
        );
    }
    document.record_change("Seed collab room: player plus one shard transform.");
    document.save(path)?;
    println!("seeded {}", path.display());
    Ok(())
}

/// Builds one transform edit request against `target`.
fn edit_request(
    credential: &str,
    op: &str,
    target: &str,
    expected_revision: u64,
    x: f32,
) -> EditRequest {
    EditRequest {
        protocol: COLLAB_PROTOCOL_VERSION,
        credential: credential.to_owned(),
        client_op_id: op.to_owned(),
        target: target.to_owned(),
        expected_revision,
        schema: TRANSFORM_SCHEMA_KEY.to_owned(),
        schema_version: canary_state::TRANSFORM_SCHEMA_VERSION,
        payload: TransformPayload {
            translation: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        },
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("collectathon_collab_client: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), CollabCliError> {
    let scratch = std::env::temp_dir().join(format!("collectathon-collab-{}", std::process::id()));
    let mut project_path: PathBuf = scratch.join("project.json");
    let mut permission_path: PathBuf = scratch.join("permissions.json");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            "--project" => {
                let value = args
                    .next()
                    .ok_or_else(|| CollabCliError::Usage("--project needs a path".to_owned()))?;
                project_path = PathBuf::from(value);
            }
            "--permissions" => {
                let value = args.next().ok_or_else(|| {
                    CollabCliError::Usage("--permissions needs a path".to_owned())
                })?;
                permission_path = PathBuf::from(value);
            }
            other => {
                return Err(CollabCliError::Usage(format!(
                    "unknown argument '{other}'; see --help"
                )));
            }
        }
    }
    if let Some(parent) = project_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(parent) = permission_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    seed_project(&project_path)?;
    let mut host = open_host(&project_path, &permission_path, &credentials())
        .map_err(|error| CollabCliError::Usage(format!("collab host open: {error}")))?;
    println!(
        "host open: epoch {} project {}",
        host.session().epoch(),
        project_path.display()
    );

    // Client one (owner): the first edit lands at sequence 1, revision 1.
    let reply = host.handle_request_bytes(&canary_collab::encode_request(&edit_request(
        "owner-secret",
        "op-owner-1",
        PLAYER_SECTION,
        0,
        5.0,
    ))?)?;
    match canary_collab::decode_response(&reply)? {
        WireResponse::Accepted(envelope) => {
            println!(
                "owner edit accepted: seq={} target_rev={}",
                envelope.sequence, envelope.target_revision
            );
            if envelope.sequence != 1 || envelope.target_revision != 1 {
                return Err(CollabCliError::Usage(format!(
                    "owner edit landed at seq={} rev={}; expected 1/1",
                    envelope.sequence, envelope.target_revision
                )));
            }
        }
        WireResponse::Rejected(rejection) => {
            return Err(CollabCliError::Usage(format!(
                "owner edit rejected: {:?}",
                rejection.code
            )));
        }
    }

    // Client two (reader) over the tagged frame path: read-only roles
    // cannot submit, and the reply echoes the edit tag.
    let tagged = canary_collab::tag_edit_body(&edit_request(
        "reader-secret",
        "op-reader-1",
        PLAYER_SECTION,
        1,
        9.0,
    ));
    let tagged_reply = host.handle_frame_body(&tagged);
    let (tag, body) = canary_collab::split_tagged_body(&tagged_reply);
    if tag != canary_collab::TAG_EDIT {
        return Err(CollabCliError::Usage(format!(
            "tagged reply echoes tag {tag:#x}; expected TAG_EDIT"
        )));
    }
    match canary_collab::decode_response(body)? {
        WireResponse::Rejected(rejection) => {
            if rejection.code != RejectCode::Forbidden {
                return Err(CollabCliError::Usage(format!(
                    "reader edit rejected as {:?}; expected Forbidden",
                    rejection.code
                )));
            }
            println!("reader edit denied: Forbidden (read-only role)");
        }
        WireResponse::Accepted(envelope) => {
            return Err(CollabCliError::Usage(format!(
                "reader edit accepted at seq {}; readers must be denied",
                envelope.sequence
            )));
        }
    }

    // Client two (editor) races on the stale revision: exactly one conflict
    // with the canonical refresh, so the client can rebase.
    let reply = host.handle_request_bytes(&canary_collab::encode_request(&edit_request(
        "editor-secret",
        "op-editor-stale",
        PLAYER_SECTION,
        0,
        7.0,
    ))?)?;
    match canary_collab::decode_response(&reply)? {
        WireResponse::Rejected(rejection) => {
            if rejection.code != RejectCode::RevisionConflict {
                return Err(CollabCliError::Usage(format!(
                    "stale edit rejected as {:?}; expected RevisionConflict",
                    rejection.code
                )));
            }
            if rejection.target_revision != 1 || rejection.refresh.is_none() {
                return Err(CollabCliError::Usage(
                    "conflict must name revision 1 with a canonical refresh".to_owned(),
                ));
            }
            println!("editor stale edit: RevisionConflict at rev=1 with refresh");
        }
        WireResponse::Accepted(envelope) => {
            return Err(CollabCliError::Usage(format!(
                "stale edit accepted at seq {}; expected a conflict",
                envelope.sequence
            )));
        }
    }

    // Rebasing onto revision 1 accepts at sequence 2.
    let reply = host.handle_request_bytes(&canary_collab::encode_request(&edit_request(
        "editor-secret",
        "op-editor-2",
        PLAYER_SECTION,
        1,
        7.0,
    ))?)?;
    match canary_collab::decode_response(&reply)? {
        WireResponse::Accepted(envelope) => {
            println!(
                "editor rebased edit accepted: seq={} target_rev={}",
                envelope.sequence, envelope.target_revision
            );
            if envelope.sequence != 2 || envelope.target_revision != 2 {
                return Err(CollabCliError::Usage(format!(
                    "rebased edit landed at seq={} rev={}; expected 2/2",
                    envelope.sequence, envelope.target_revision
                )));
            }
        }
        WireResponse::Rejected(rejection) => {
            return Err(CollabCliError::Usage(format!(
                "rebased edit rejected: {:?}",
                rejection.code
            )));
        }
    }

    // Reconnect history: the reader syncs from zero and replays the
    // contiguous tail, with no snapshot on the covered path.
    let sync = SyncRequest {
        protocol: COLLAB_PROTOCOL_VERSION,
        credential: "reader-secret".to_owned(),
        last_sequence: 0,
    };
    let sync_reply = host.handle_sync_bytes(&canary_collab::encode_sync_request(&sync)?)?;
    let sync_response = canary_collab::decode_sync_response(&sync_reply)?;
    if sync_response.tail.len() != 2 || sync_response.snapshot.is_some() {
        return Err(CollabCliError::Usage(format!(
            "sync replays {} ops with snapshot={}; expected 2 ops, no snapshot",
            sync_response.tail.len(),
            sync_response.snapshot.is_some()
        )));
    }
    println!("reader sync: tail of 2 ops, no snapshot (covered cursor)");

    // Gameplay separation: the durable project carries authored transforms
    // only — no simulation component ever crosses this path.
    let durable = std::fs::read_to_string(&project_path)?;
    for schema in [
        "collectathon.score",
        "collectathon.pickup",
        "collectathon.player",
    ] {
        if durable.contains(schema) {
            return Err(CollabCliError::Usage(format!(
                "gameplay schema '{schema}' leaked into the collab project"
            )));
        }
    }
    println!("gameplay separation: no simulation schemas in the project file");
    println!("collectathon_collab_client: two-client run converged");
    Ok(())
}
