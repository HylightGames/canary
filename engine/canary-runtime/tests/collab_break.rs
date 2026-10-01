//! Runtime pump break-it: malformed frames stay alive, sync gates stay
//! typed, and the transport pump reports the kind it actually served.
//!
//! No production code is touched; no timing-sensitive asserts.

use std::collections::BTreeMap;
use std::path::PathBuf;

use canary_collab::{
    decode_response, decode_sync_response, encode_sync_request, split_tagged_body, tag_edit_body,
    CollabError, CredentialBinding, EditRequest, RejectCode, Role, SyncRequest, WireResponse,
    COLLAB_PROTOCOL_VERSION, TAG_EDIT, TAG_SYNC,
};
use canary_net::{LoopbackTransport, NetLimits, NetRecv, NetSend, NetTransport};
use canary_runtime::{serve_single_frame, CollabSessionHost, ServeOutcome};

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
        "canary-runtime-break-{}-{}-{tag}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn seed_project(path: &std::path::Path) {
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

fn open_host(dir: &std::path::Path) -> CollabSessionHost {
    let project_path = dir.join("project.json");
    let permission_path = dir.join("permissions.json");
    seed_project(&project_path);
    CollabSessionHost::open(&project_path, &permission_path, &credentials()).expect("open host")
}

/// Every malformed shape through `handle_frame_body` answers with a tagged
/// generic `Malformed` reject echoing its tag, and the very next good
/// request on the same host is still accepted.
#[test]
fn malformed_frames_answer_tagged_malformed_and_the_host_stays_usable() {
    let dir = scratch_dir("malformed");
    let mut host = open_host(&dir);
    let malformed: Vec<Vec<u8>> = vec![
        vec![],
        vec![0x00],
        vec![0x03, 0x01, 0x02],
        vec![0x7F, 0x01, 0x02],
        vec![0x80, 0x09],
        vec![0xFF],
        {
            let mut body = vec![TAG_EDIT];
            body.extend_from_slice(b"\xff\xff\xff");
            body
        },
        {
            let mut body = vec![TAG_SYNC];
            body.extend_from_slice(b"\x01\x02\x03\x04");
            body
        },
    ];
    for body in &malformed {
        let reply = host.handle_frame_body(body);
        let expected_tag = body.first().copied().unwrap_or(0x00);
        assert_eq!(
            reply.first().copied().unwrap_or(0xFF),
            expected_tag,
            "reply echoes inbound tag for input {body:?}"
        );
        let (_, codec) = split_tagged_body(&reply);
        match decode_response(codec).expect("generic reject decodes") {
            WireResponse::Rejected(rejection) => {
                assert_eq!(rejection.code, RejectCode::Malformed);
            }
            WireResponse::Accepted(_) => panic!("malformed input {body:?} was accepted"),
        }
    }
    // Untouched by all of that: the next good edit sequences at 1.
    let reply = host
        .handle_request_bytes(
            &canary_collab::encode_request(&edit("owner-secret", "op-1", 0)).expect("encode"),
        )
        .expect("good request serves after malformed frames");
    match decode_response(&reply).expect("decode good reply") {
        WireResponse::Accepted(envelope) => assert_eq!(envelope.sequence, 1),
        WireResponse::Rejected(rejection) => {
            panic!(
                "good edit rejected after malformed frames: {:?}",
                rejection.code
            )
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// Sync gates through the host seam: bad protocol, forged credential, and
/// ahead-of-history cursors all fail typed, while covered tails and the
/// tip serve.
#[test]
fn sync_gates_reject_typed_but_covered_cursors_serve() {
    let dir = scratch_dir("sync-gates");
    let mut host = open_host(&dir);
    host.handle_request_bytes(
        &canary_collab::encode_request(&edit("owner-secret", "op-1", 0)).expect("encode"),
    )
    .expect("seed one accept");

    let ask = |last: u64| SyncRequest {
        protocol: COLLAB_PROTOCOL_VERSION,
        credential: "reader-secret".to_owned(),
        last_sequence: last,
    };
    let covered = host
        .handle_sync_bytes(&encode_sync_request(&ask(0)).expect("encode"))
        .expect("covered tail serves");
    let reply = decode_sync_response(&covered).expect("decode covered");
    assert_eq!(reply.tail.len(), 1);
    assert!(reply.snapshot.is_none());

    let tip = host
        .handle_sync_bytes(&encode_sync_request(&ask(1)).expect("encode"))
        .expect("tip serves empty");
    assert!(decode_sync_response(&tip)
        .expect("decode tip")
        .tail
        .is_empty());

    let mut bad_protocol = ask(0);
    bad_protocol.protocol = 999;
    assert!(matches!(
        host.handle_sync_bytes(&encode_sync_request(&bad_protocol).expect("encode")),
        Err(CollabError::Decode(_))
    ));
    let mut forged = ask(0);
    forged.credential = "forged".to_owned();
    assert!(matches!(
        host.handle_sync_bytes(&encode_sync_request(&forged).expect("encode")),
        Err(CollabError::Unauthenticated)
    ));
    assert!(matches!(
        host.handle_sync_bytes(&encode_sync_request(&ask(99)).expect("encode")),
        Err(CollabError::InvalidCursor)
    ));
    std::fs::remove_dir_all(&dir).ok();
}

/// The transport pump survives a garbage frame mid-stream: garbage is
/// answered with a tagged `Malformed` reject, the next good sync still
/// serves, and outcome kinds match what was actually served.
#[tokio::test(flavor = "multi_thread")]
async fn transport_pump_survives_garbage_then_serves_sync() {
    let dir = scratch_dir("pump-garbage");
    let project_path = dir.join("project.json");
    let permission_path = dir.join("permissions.json");
    seed_project(&project_path);
    let mut host =
        CollabSessionHost::open(&project_path, &permission_path, &credentials()).expect("open");
    let limits = NetLimits::default();
    let server = LoopbackTransport::bind().expect("bind server");
    let addr = server.local_addr().expect("server address");
    let accept = tokio::spawn(async move { server.accept().await });
    let client = LoopbackTransport::ephemeral();
    let (mut send, mut recv) = client.connect(addr, "test").await.expect("connect");
    let (mut server_send, mut server_recv) = accept.await.expect("accept task").expect("accept");

    // Good edit first: establishes sequence 1.
    send.send_frame(&tag_edit_body(&edit("owner-secret", "op-1", 0)), &limits)
        .await
        .expect("send edit");
    let outcome = serve_single_frame(&mut server_send, &mut server_recv, &mut host, &limits)
        .await
        .expect("serve edit");
    assert_eq!(outcome, ServeOutcome::Edit);
    let reply = recv.recv_frame(&limits).await.expect("edit reply");
    let (tag, body) = split_tagged_body(&reply);
    assert_eq!(tag, TAG_EDIT);
    assert!(matches!(
        decode_response(body).expect("decode edit reply"),
        WireResponse::Accepted(_)
    ));

    // Garbage mid-stream: answered, tagged, typed — connection alive.
    send.send_frame(&[0x42, 0xDE, 0xAD], &limits)
        .await
        .expect("send garbage");
    let outcome = serve_single_frame(&mut server_send, &mut server_recv, &mut host, &limits)
        .await
        .expect("serve garbage");
    assert_eq!(outcome, ServeOutcome::Edit, "unknown tags report Edit");
    let reply = recv.recv_frame(&limits).await.expect("garbage reply");
    let (tag, body) = split_tagged_body(&reply);
    assert_eq!(tag, 0x42, "garbage reply echoes the inbound tag");
    match decode_response(body).expect("decode garbage reply") {
        WireResponse::Rejected(rejection) => assert_eq!(rejection.code, RejectCode::Malformed),
        WireResponse::Accepted(_) => panic!("garbage frame accepted"),
    }

    // Empty frame: same typed answer, still alive.
    send.send_frame(&[], &limits).await.expect("send empty");
    serve_single_frame(&mut server_send, &mut server_recv, &mut host, &limits)
        .await
        .expect("serve empty");
    let reply = recv.recv_frame(&limits).await.expect("empty reply");
    let (tag, body) = split_tagged_body(&reply);
    assert_eq!(tag, 0x00);
    assert!(matches!(
        decode_response(body).expect("decode empty reply"),
        WireResponse::Rejected(_)
    ));

    // The stream still serves: a sync cursor replays the tail.
    let sync = SyncRequest {
        protocol: COLLAB_PROTOCOL_VERSION,
        credential: "reader-secret".to_owned(),
        last_sequence: 0,
    };
    let mut sync_body = vec![TAG_SYNC];
    sync_body.extend_from_slice(&encode_sync_request(&sync).expect("encode sync"));
    send.send_frame(&sync_body, &limits)
        .await
        .expect("send sync");
    let outcome = serve_single_frame(&mut server_send, &mut server_recv, &mut host, &limits)
        .await
        .expect("serve sync");
    assert_eq!(outcome, ServeOutcome::Sync);
    let reply = recv.recv_frame(&limits).await.expect("sync reply");
    let (tag, body) = split_tagged_body(&reply);
    assert_eq!(tag, TAG_SYNC);
    let sync_reply = decode_sync_response(body).expect("decode sync reply");
    assert_eq!(sync_reply.tail.len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}
