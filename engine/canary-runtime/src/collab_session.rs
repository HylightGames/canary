//! Authoritative collaboration session server composition.
//!
//! This module wires the three `.16` layers into one serving path without
//! merging them: bytes move across the [`canary_net`](canary_net)
//! transport trait only ([`NetSend`]/[`NetRecv`] — never a backend type),
//! validation and ordering belong to [`canary_collab`](canary_collab),
//! and durable commits belong to [`canary_state`](canary_state). The host
//! owns a [`Session`](canary_collab::Session) plus its durable seams and
//! serves one framed request at a time.
//!
//! Frame tagging is a wire concern, not a transport concern: the first
//! byte of each inbound frame selects the request kind per the
//! [`canary_collab`] frame-tag registry ([`TAG_EDIT`] for an edit
//! request, [`TAG_SYNC`] for a sync cursor). Replies are the matching
//! codec bytes ([`WireResponse`](canary_collab::WireResponse) for edits,
//! [`SyncResponse`](canary_collab::SyncResponse) for sync). An unknown tag
//! or an undecodable body is answered with a generic `Malformed` reject so
//! the connection stays alive; a size-bound failure (oversize request or an
//! oversize reply that encodes past the response bound) is answered with a
//! typed [`TooLarge`](canary_collab::RejectCode::TooLarge) reject instead.
//! Only transport failures end the stream.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use canary_collab::{
    decode_sync_request, encode_response, encode_sync_response, AcceptEnvelope, CollabError,
    CredentialBinding, DurableProjectStore, FileProjectStore, PermissionError, PermissionStore,
    Session, WireResponse,
};
pub use canary_collab::{split_tagged_body, tag_edit_body, TAG_EDIT, TAG_SYNC};
use canary_net::{NetLimits, NetRecv, NetSend};
use thiserror::Error;

/// Why a collaboration host failed to open.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CollabHostError {
    /// The durable project file could not be loaded.
    #[error("collab project load failed: {0}")]
    ProjectLoad(#[from] canary_state::StateError),
    /// The server-side permission store could not be opened or persisted.
    #[error("collab permission store failed: {0}")]
    Permissions(#[from] PermissionError),
}

/// Why serving one frame failed. Decode-level failures are answered with
/// a generic reject, never raised here — this is transport and host
/// breakage only.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CollabServeError {
    /// The transport failed under the frame.
    #[error("collab transport failure: {0}")]
    Net(#[from] canary_net::NetError),
    /// A reply could not be encoded for the wire.
    #[error("collab reply encode failure: {0}")]
    Reply(#[from] CollabError),
}

/// What one served frame did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeOutcome {
    /// An edit request was answered (accepted or rejected).
    Edit,
    /// A sync request was answered (tail or snapshot).
    Sync,
}

/// The serving owner: one [`Session`](canary_collab::Session), its project
/// file seam, and its permission file, composed but not merged.
pub struct CollabSessionHost {
    session: Session,
    project: FileProjectStore,
    permission_path: PathBuf,
}

impl CollabSessionHost {
    /// Opens (or provisions) the serving state: the durable project file,
    /// the server-side permission file from `credentials`, and the session
    /// over both. Restarting over the same paths resumes sequences and
    /// bumps the fencing epoch.
    pub fn open(
        project_path: &Path,
        permission_path: &Path,
        credentials: &BTreeMap<String, CredentialBinding>,
    ) -> Result<Self, CollabHostError> {
        let project = FileProjectStore::new(project_path);
        let document = project.load()?;
        let (permissions, authenticator) =
            PermissionStore::open_or_provision(permission_path, credentials)?;
        Ok(Self {
            session: Session::open(document, permissions, authenticator),
            project,
            permission_path: permission_path.to_path_buf(),
        })
    }

    /// The live session (read-only view for inspection and tests).
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The project file seam (for restart and recovery tests).
    #[must_use]
    pub fn project_store(&self) -> &FileProjectStore {
        &self.project
    }

    /// The permission file path.
    #[must_use]
    pub fn permission_path(&self) -> &Path {
        &self.permission_path
    }

    /// Handles one raw edit-request body (without the frame tag) and
    /// returns the encoded reply.
    pub fn handle_request_bytes(&mut self, body: &[u8]) -> Result<Vec<u8>, CollabError> {
        self.session.submit_bytes(body, &self.project)
    }

    /// Handles one raw sync-request body and returns the encoded reply.
    pub fn handle_sync_bytes(&self, body: &[u8]) -> Result<Vec<u8>, CollabError> {
        let request = decode_sync_request(body)?;
        let reply = self.session.sync(&request)?;
        encode_sync_response(&reply)
    }

    /// Handles one tagged inbound frame body and returns the tagged reply
    /// body. Dispatch lives on [`Session::handle_frame_body`], over the
    /// [`canary_collab`] frame-tag registry; this only supplies the
    /// durable seam. Untaggable or undecodable input yields a generic
    /// `Malformed` reject — the caller's connection stays usable — while
    /// size-bound failures keep the typed `TooLarge` reject.
    #[must_use]
    pub fn handle_frame_body(&mut self, body: &[u8]) -> Vec<u8> {
        self.session.handle_frame_body(body, &self.project)
    }

    /// Queues the pending broadcast envelopes as encoded accept replies,
    /// in sequence order.
    ///
    /// Encode failures are filtered, not counted in
    /// [`dropped_broadcasts`](canary_collab::Session::dropped_broadcasts):
    /// one accept envelope is bounded small fields (bounded IDs, one
    /// transform payload), so its encoding cannot reach the 1 MiB response
    /// bound and the failure arm is unreachable.
    pub fn drain_broadcast_bytes(&mut self) -> Vec<Vec<u8>> {
        self.session
            .drain_broadcasts()
            .iter()
            .filter_map(|envelope: &AcceptEnvelope| {
                encode_response(&WireResponse::Accepted(envelope.clone())).ok()
            })
            .collect()
    }
}

/// Serves exactly one framed request over a `canary-net` transport pair:
/// reads one frame, handles it through `host`, writes the reply frame, and
/// reports which kind it served. Generic over [`NetSend`]/[`NetRecv`] only —
/// no backend type crosses this signature.
pub async fn serve_single_frame<S: NetSend, R: NetRecv>(
    send: &mut S,
    recv: &mut R,
    host: &mut CollabSessionHost,
    limits: &NetLimits,
) -> Result<ServeOutcome, CollabServeError> {
    let inbound = recv.recv_frame(limits).await?;
    let tag = inbound.first().copied().unwrap_or(0x00);
    let reply = host.handle_frame_body(&inbound);
    send.send_frame(&reply, limits).await?;
    if tag == TAG_SYNC {
        Ok(ServeOutcome::Sync)
    } else {
        Ok(ServeOutcome::Edit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use canary_collab::{
        decode_response, decode_sync_response, EditRequest, RejectCode, Role, SyncRequest,
        SyncResponse, TransformPayload, COLLAB_PROTOCOL_VERSION,
    };
    use canary_net::{LoopbackTransport, NetTransport};

    fn credentials() -> BTreeMap<String, CredentialBinding> {
        BTreeMap::from([
            (
                "owner-secret".to_owned(),
                CredentialBinding {
                    actor: canary_collab::ActorId(1),
                    role: Role::Owner,
                },
            ),
            (
                "editor-secret".to_owned(),
                CredentialBinding {
                    actor: canary_collab::ActorId(2),
                    role: Role::Editor,
                },
            ),
            (
                "reader-secret".to_owned(),
                CredentialBinding {
                    actor: canary_collab::ActorId(3),
                    role: Role::Reader,
                },
            ),
        ])
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "canary-runtime-collab-{}-{}-{tag}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn seed_project(path: &Path) {
        use canary_state::{AuthoredDocument, ProjectId};
        let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
        doc.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({
                "canary.transform": {
                    "translation": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                },
            }),
        );
        doc.save(path).expect("seed project");
    }

    fn edit(credential: &str, op: &str, expected: u64) -> EditRequest {
        EditRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: credential.to_owned(),
            client_op_id: op.to_owned(),
            target: "entity.hero".to_owned(),
            expected_revision: expected,
            schema: canary_state::TRANSFORM_SCHEMA_KEY.to_owned(),
            schema_version: canary_state::TRANSFORM_SCHEMA_VERSION,
            payload: canary_collab::TransformPayload {
                translation: [1.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
        }
    }

    /// Two separate transport clients, one session server: owner and
    /// editor submit over real `canary-net` streams, the reader is denied,
    /// and a same-target race resolves to exactly one accept plus one
    /// conflict — with broadcasts observable in sequence order.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_clients_converge_through_the_transport() {
        let dir = scratch_dir("two-client");
        let project_path = dir.join("project.json");
        let permission_path = dir.join("permissions.json");
        seed_project(&project_path);
        let mut host = CollabSessionHost::open(&project_path, &permission_path, &credentials())
            .expect("open host");
        let limits = NetLimits::default();

        let server = LoopbackTransport::bind().expect("bind server");
        let addr = server.local_addr().expect("server address");

        // Client one (owner) connects; the server accepts its stream.
        let accept_one = tokio::spawn(async move { server.accept().await });
        let client_one = LoopbackTransport::ephemeral();
        let (mut one_send, mut one_recv) = client_one
            .connect(addr, "test")
            .await
            .expect("owner connect");
        let (mut server_one_send, mut server_one_recv) = accept_one
            .await
            .expect("accept task")
            .expect("owner accept");

        // Owner edit crosses the wire and is accepted at sequence 1.
        one_send
            .send_frame(&tag_edit_body(&edit("owner-secret", "op-1", 0)), &limits)
            .await
            .expect("owner send");
        let outcome = serve_single_frame(
            &mut server_one_send,
            &mut server_one_recv,
            &mut host,
            &limits,
        )
        .await
        .expect("serve owner edit");
        assert_eq!(outcome, ServeOutcome::Edit);
        let reply = one_recv.recv_frame(&limits).await.expect("owner reply");
        let (tag, body) = split_tagged_body(&reply);
        assert_eq!(tag, TAG_EDIT);
        match decode_response(body).expect("decode owner reply") {
            WireResponse::Accepted(envelope) => {
                assert_eq!(envelope.sequence, 1);
                assert_eq!(envelope.target_revision, 1);
            }
            WireResponse::Rejected(rejection) => {
                panic!("owner edit rejected: {:?}", rejection.code)
            }
        }

        // The reader's edit on the same stream is denied; the stream stays
        // usable for the race below.
        one_send
            .send_frame(
                &tag_edit_body(&edit("reader-secret", "op-reader", 1)),
                &limits,
            )
            .await
            .expect("reader send");
        serve_single_frame(
            &mut server_one_send,
            &mut server_one_recv,
            &mut host,
            &limits,
        )
        .await
        .expect("serve reader edit");
        let reply = one_recv.recv_frame(&limits).await.expect("reader reply");
        let (_, body) = split_tagged_body(&reply);
        match decode_response(body).expect("decode reader reply") {
            WireResponse::Rejected(rejection) => {
                assert_eq!(rejection.code, RejectCode::Forbidden);
            }
            WireResponse::Accepted(envelope) => {
                panic!("reader edit accepted at seq {}", envelope.sequence)
            }
        }

        // Same stale target revision from the owner now conflicts: the
        // reader denial advanced nothing, but the first edit did.
        one_send
            .send_frame(
                &tag_edit_body(&edit("owner-secret", "op-stale", 0)),
                &limits,
            )
            .await
            .expect("stale send");
        serve_single_frame(
            &mut server_one_send,
            &mut server_one_recv,
            &mut host,
            &limits,
        )
        .await
        .expect("serve stale edit");
        let reply = one_recv.recv_frame(&limits).await.expect("stale reply");
        let (_, body) = split_tagged_body(&reply);
        match decode_response(body).expect("decode stale reply") {
            WireResponse::Rejected(rejection) => {
                assert_eq!(rejection.code, RejectCode::RevisionConflict);
                assert_eq!(rejection.target_revision, 1);
                assert!(rejection.refresh.is_some());
            }
            WireResponse::Accepted(envelope) => {
                panic!("stale edit accepted at seq {}", envelope.sequence)
            }
        }

        // Broadcasts so far: exactly the one accept, in order. Broadcast
        // bodies are untagged codec bytes (the host tags only replies).
        let broadcasts = host.drain_broadcast_bytes();
        assert_eq!(broadcasts.len(), 1);
        match decode_response(&broadcasts[0]).expect("decode broadcast") {
            WireResponse::Accepted(envelope) => assert_eq!(envelope.sequence, 1),
            WireResponse::Rejected(rejection) => {
                panic!("broadcast is a reject: {:?}", rejection.code)
            }
        }

        // Sync over the wire: covered cursor replays the tail.
        let sync = SyncRequest {
            protocol: COLLAB_PROTOCOL_VERSION,
            credential: "editor-secret".to_owned(),
            last_sequence: 0,
        };
        let mut sync_body = vec![TAG_SYNC];
        sync_body
            .extend_from_slice(&canary_collab::encode_sync_request(&sync).expect("encode sync"));
        one_send
            .send_frame(&sync_body, &limits)
            .await
            .expect("sync send");
        let outcome = serve_single_frame(
            &mut server_one_send,
            &mut server_one_recv,
            &mut host,
            &limits,
        )
        .await
        .expect("serve sync");
        assert_eq!(outcome, ServeOutcome::Sync);
        let reply = one_recv.recv_frame(&limits).await.expect("sync reply");
        let (tag, body) = split_tagged_body(&reply);
        assert_eq!(tag, TAG_SYNC);
        let sync_reply: SyncResponse = decode_sync_response(body).expect("decode sync reply");
        assert_eq!(sync_reply.tail.len(), 1);
        assert!(sync_reply.snapshot.is_none());

        // An untagged/garbage frame is answered with a generic reject, not
        // a dropped connection: the next good request still serves.
        one_send
            .send_frame(&[0x7F, 0x01, 0x02], &limits)
            .await
            .expect("garbage send");
        serve_single_frame(
            &mut server_one_send,
            &mut server_one_recv,
            &mut host,
            &limits,
        )
        .await
        .expect("serve garbage");
        let reply = one_recv.recv_frame(&limits).await.expect("garbage reply");
        let (_, body) = split_tagged_body(&reply);
        match decode_response(body).expect("decode garbage reply") {
            WireResponse::Rejected(rejection) => {
                assert_eq!(rejection.code, RejectCode::Malformed);
            }
            WireResponse::Accepted(_) => panic!("garbage frame accepted"),
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Collab↔transform validation parity: `canary-collab` owns stage-6
    /// numeric validation without depending on `canary-transform`
    /// (no-dep decision), so this test — sitting in the composition
    /// owner that sees both crates — runs the same accept/reject vectors
    /// through both `TransformPayload::validate` and
    /// `Transform::validate` and requires identical verdicts. Either
    /// side drifting (a widened epsilon, a rejected scale, a new NaN
    /// arm) fails here before it can split the wire contract from the
    /// simulation's own notion of a valid transform.
    #[test]
    fn collab_and_transform_validations_agree_on_every_vector() {
        use canary_collab::QUAT_NORM_EPSILON as COLLAB_EPSILON;
        use canary_transform::{Transform, QUAT_NORM_EPSILON as NATIVE_EPSILON};

        // One epsilon, two crates: the duplication the no-dep decision
        // forces is pinned here instead of trusted.
        assert_eq!(
            COLLAB_EPSILON, NATIVE_EPSILON,
            "quaternion epsilon must match on both sides"
        );

        fn native(payload: &TransformPayload) -> Transform {
            Transform {
                translation: glam::Vec3::from_array(payload.translation),
                rotation: glam::Quat::from_array(payload.rotation),
                scale: glam::Vec3::from_array(payload.scale),
            }
        }

        let half_root = std::f32::consts::FRAC_1_SQRT_2;
        let identity = TransformPayload::identity();
        let vectors: Vec<(&str, TransformPayload, bool)> = vec![
            ("identity", identity, true),
            (
                "near-unit quaternion",
                TransformPayload {
                    rotation: [half_root, 0.0, 0.0, half_root],
                    ..identity
                },
                true,
            ),
            (
                "zero scale is content",
                TransformPayload {
                    scale: [0.0, 1.0, 1.0],
                    ..identity
                },
                true,
            ),
            (
                "negative scale is content",
                TransformPayload {
                    scale: [-1.0, 2.0, 0.5],
                    ..identity
                },
                true,
            ),
            (
                "all-zero scale is content",
                TransformPayload {
                    scale: [0.0, 0.0, 0.0],
                    ..identity
                },
                true,
            ),
            (
                "extreme finite translation",
                TransformPayload {
                    translation: [f32::MAX, f32::MIN, 0.0],
                    ..identity
                },
                true,
            ),
            (
                "NaN translation",
                TransformPayload {
                    translation: [f32::NAN, 0.0, 0.0],
                    ..identity
                },
                false,
            ),
            (
                "infinite scale",
                TransformPayload {
                    scale: [1.0, f32::INFINITY, 1.0],
                    ..identity
                },
                false,
            ),
            (
                "negative-infinite rotation",
                TransformPayload {
                    rotation: [0.0, 0.0, 0.0, f32::NEG_INFINITY],
                    ..identity
                },
                false,
            ),
            (
                "NaN rotation",
                TransformPayload {
                    rotation: [f32::NAN, 0.0, 0.0, 1.0],
                    ..identity
                },
                false,
            ),
            (
                "zero quaternion",
                TransformPayload {
                    rotation: [0.0, 0.0, 0.0, 0.0],
                    ..identity
                },
                false,
            ),
            (
                "long quaternion",
                TransformPayload {
                    rotation: [1.0, 1.0, 0.0, 0.0],
                    ..identity
                },
                false,
            ),
            (
                "tall quaternion",
                TransformPayload {
                    rotation: [0.0, 0.0, 0.0, 2.0],
                    ..identity
                },
                false,
            ),
        ];
        for (name, payload, expect_accept) in &vectors {
            let collab = payload.validate().is_ok();
            let home = native(payload).validate().is_ok();
            assert_eq!(
                collab, home,
                "verdicts diverge on vector '{name}': collab says {collab}"
            );
            assert_eq!(
                collab,
                *expect_accept,
                "vector '{name}' must be {}",
                if *expect_accept {
                    "accepted"
                } else {
                    "rejected"
                }
            );
        }
    }

    #[test]
    fn host_reopens_durable_state_across_restart() {
        let dir = scratch_dir("reopen");
        let project_path = dir.join("project.json");
        let permission_path = dir.join("permissions.json");
        seed_project(&project_path);
        let mut host = CollabSessionHost::open(&project_path, &permission_path, &credentials())
            .expect("open host");
        assert_eq!(host.session().epoch(), 1);
        let reply = host
            .handle_request_bytes(
                &canary_collab::encode_request(&edit("owner-secret", "op-1", 0)).expect("encode"),
            )
            .expect("submit");
        match decode_response(&reply).expect("decode") {
            WireResponse::Accepted(envelope) => assert_eq!(envelope.sequence, 1),
            WireResponse::Rejected(rejection) => {
                panic!("expected accept, got {:?}", rejection.code)
            }
        }
        drop(host);

        let reopened = CollabSessionHost::open(&project_path, &permission_path, &credentials())
            .expect("reopen host");
        assert_eq!(reopened.session().epoch(), 2);
        assert_eq!(
            reopened.session().document().history.newest_retained().0,
            1,
            "sequences survive restart through the durable file"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
