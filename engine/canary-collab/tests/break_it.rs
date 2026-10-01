//! Break-it pass over the `.16` op path: seams changed since review.
//!
//! Each test names the seam it attacks. All helpers use `expect` with a
//! message instead of `unwrap`; no production logic is touched here.

use std::cell::RefCell;
use std::collections::BTreeMap;

use canary_collab::{
    decode_request, decode_response, decode_sync_request, encode_request, encode_sync_request,
    AcceptEnvelope, ActorId, ClientOpId, CollabError, CredentialBinding, DurableProjectStore,
    EditRequest, PermissionStore, RejectCode, Role, Session, SyncRequest, TransformPayload,
    COLLAB_PROTOCOL_VERSION, MAX_RETAINED_OPERATIONS, MAX_SNAPSHOT_BYTES, TAG_EDIT, TAG_SYNC,
};
use canary_state::{
    AuthoredDocument, LogicalEntityId, ProjectId, StateError, TRANSFORM_SCHEMA_KEY,
    TRANSFORM_SCHEMA_VERSION,
};

struct MemStore {
    doc: RefCell<AuthoredDocument>,
}

impl DurableProjectStore for MemStore {
    fn load(&self) -> Result<AuthoredDocument, StateError> {
        Ok(self.doc.borrow().clone())
    }

    fn save(&self, document: &AuthoredDocument) -> Result<(), StateError> {
        *self.doc.borrow_mut() = document.clone();
        Ok(())
    }
}

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

fn hero_fields(x: f64) -> serde_json::Value {
    serde_json::json!({
        "translation": [x, 0.0, 0.0],
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0],
    })
}

fn payload_at(x: f32) -> TransformPayload {
    TransformPayload {
        translation: [x, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    }
}

fn base_document() -> AuthoredDocument {
    let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
    doc.sections.insert(
        "entity.hero".to_owned(),
        serde_json::json!({
            "canary.transform": hero_fields(0.0),
            "test.health": {"hp": 10},
        }),
    );
    doc.sections.insert(
        "entity.sidekick".to_owned(),
        serde_json::json!({"canary.transform": hero_fields(0.0)}),
    );
    doc
}

fn harness() -> (Session, MemStore) {
    let doc = base_document();
    let (permissions, authenticator) =
        PermissionStore::provision(&credentials()).expect("distinct test actors provision");
    let store = MemStore {
        doc: RefCell::new(doc.clone()),
    };
    (Session::open(doc, permissions, authenticator), store)
}

fn edit(credential: &str, op: &str, target: &str, expected: u64) -> EditRequest {
    EditRequest {
        protocol: COLLAB_PROTOCOL_VERSION,
        credential: credential.to_owned(),
        client_op_id: op.to_owned(),
        target: target.to_owned(),
        expected_revision: expected,
        schema: TRANSFORM_SCHEMA_KEY.to_owned(),
        schema_version: TRANSFORM_SCHEMA_VERSION,
        payload: payload_at(1.0),
    }
}

fn accepted(outcome: canary_collab::SubmitOutcome) -> AcceptEnvelope {
    match outcome {
        canary_collab::SubmitOutcome::Accepted(accepted) => accepted.envelope,
        canary_collab::SubmitOutcome::Rejected(rejection) => {
            panic!(
                "expected accept, got {:?}: {}",
                rejection.code, rejection.message
            )
        }
    }
}

fn rejected(outcome: canary_collab::SubmitOutcome) -> canary_collab::Rejection {
    match outcome {
        canary_collab::SubmitOutcome::Rejected(rejection) => rejection,
        canary_collab::SubmitOutcome::Accepted(accepted) => {
            panic!(
                "expected reject, got accept at seq {}",
                accepted.envelope.sequence
            )
        }
    }
}

/// Reserved (`0x00`, `0x03`–`0x7F`) and host-reserved (`0x80`–`0xFF`) tags
/// all answer with a generic `Malformed` reject echoing the inbound tag,
/// and the session stays usable for the next good request.
#[test]
fn reserved_and_host_tags_answer_generic_malformed_and_stay_usable() {
    let (mut session, store) = harness();
    for tag in [0x00u8, 0x03, 0x04, 0x2A, 0x7F, 0x80, 0xC8, 0xFF] {
        let body = vec![tag, 0x01, 0x02];
        let reply = session.handle_frame_body(&body, &store);
        assert_eq!(
            reply.first().copied().unwrap_or(0xFF),
            tag,
            "reply echoes inbound tag {tag:#04x}"
        );
        let response = decode_response(&reply[1..]).expect("generic reject decodes as a response");
        match response {
            canary_collab::WireResponse::Rejected(rejection) => {
                assert_eq!(rejection.code, RejectCode::Malformed);
                assert_eq!(rejection.target_revision, 0);
            }
            canary_collab::WireResponse::Accepted(_) => {
                panic!("unknown tag {tag:#04x} was accepted")
            }
        }
    }
    // Empty input is the untaggable arm: tag 0x00, still a typed reject.
    let reply = session.handle_frame_body(&[], &store);
    assert_eq!(reply.first().copied().unwrap_or(0xFF), 0x00);
    let response = decode_response(&reply[1..]).expect("empty-input reject decodes");
    assert!(matches!(response, canary_collab::WireResponse::Rejected(_)));
    // Truncated edit codec under a valid tag: generic reject, same tag echo.
    let mut truncated = vec![TAG_EDIT, 0x01];
    truncated.extend_from_slice(b"\xff\xff");
    let reply = session.handle_frame_body(&truncated, &store);
    assert_eq!(reply.first().copied().unwrap_or(0xFF), TAG_EDIT);
    // The session advanced nothing and still serves a good edit.
    assert_eq!(session.document().history.next_sequence.0, 1);
    let envelope =
        accepted(session.submit(&edit("owner-secret", "op-1", "entity.hero", 0), &store));
    assert_eq!(envelope.sequence, 1);
}

/// A `TAG_SYNC` body carrying edit bytes (and vice versa) must not be
/// misdispatched: the codec gate fails typed and the connection survives.
#[test]
fn cross_tag_codec_mismatch_is_a_typed_reject_not_a_dispatch() {
    let (mut session, store) = harness();
    let edit_bytes =
        encode_request(&edit("owner-secret", "op-1", "entity.hero", 0)).expect("encode edit");
    let mut mislabeled = vec![TAG_SYNC];
    mislabeled.extend_from_slice(&edit_bytes);
    let reply = session.handle_frame_body(&mislabeled, &store);
    assert_eq!(reply.first().copied().unwrap_or(0xFF), TAG_SYNC);
    let sync_err = decode_sync_request(&mislabeled[1..]);
    assert!(
        sync_err.is_err() || session.document().history.next_sequence.0 == 1,
        "edit bytes under TAG_SYNC must not apply"
    );
    // Sync bytes under TAG_EDIT must not apply either.
    let sync = SyncRequest {
        protocol: COLLAB_PROTOCOL_VERSION,
        credential: "reader-secret".to_owned(),
        last_sequence: 0,
    };
    let sync_bytes = encode_sync_request(&sync).expect("encode sync");
    let mut mislabeled = vec![TAG_EDIT];
    mislabeled.extend_from_slice(&sync_bytes);
    let reply = session.handle_frame_body(&mislabeled, &store);
    assert_eq!(reply.first().copied().unwrap_or(0xFF), TAG_EDIT);
    assert!(decode_request(&sync_bytes).is_err());
    assert_eq!(session.document().history.next_sequence.0, 1);
    let _ = reply;
}

/// Snapshot ceiling boundary: exactly 1 MiB serves, 1 MiB + 1 fails
/// typed. The bulk is inert padding the op path never touches.
#[test]
fn snapshot_ceiling_boundary_exactly_one_mib_serves_but_one_more_byte_fails() {
    fn drive_to_checkpoint(pad: usize) -> (Session, MemStore) {
        let mut doc = base_document();
        doc.sections.insert(
            "entity.bulk".to_owned(),
            serde_json::json!({"blob": "x".repeat(pad)}),
        );
        let (permissions, authenticator) =
            PermissionStore::provision(&credentials()).expect("provision");
        let store = MemStore {
            doc: RefCell::new(doc.clone()),
        };
        let mut session = Session::open(doc, permissions, authenticator);
        let total = MAX_RETAINED_OPERATIONS + 2;
        for index in 0..total {
            let mut request = edit(
                "owner-secret",
                &format!("op-{index}"),
                "entity.hero",
                index as u64,
            );
            request.payload = payload_at(index as f32);
            accepted(session.submit(&request, &store));
        }
        (session, store)
    }

    // Measure the unpadded snapshot, then pad linearly to the ceiling.
    // `x` needs no JSON escaping, so one pad byte is one snapshot byte.
    let (probe, _) = drive_to_checkpoint(0);
    let probe_len = probe
        .document()
        .to_canonical_json()
        .expect("serialize probe")
        .len();
    assert!(
        probe_len < MAX_SNAPSHOT_BYTES,
        "probe {probe_len} must sit below the ceiling or the boundary test needs a smaller base"
    );
    let pad = MAX_SNAPSHOT_BYTES - probe_len;
    let (mut session, _) = drive_to_checkpoint(pad);
    let exact_len = session
        .document()
        .to_canonical_json()
        .expect("serialize exact")
        .len();
    assert_eq!(
        exact_len, MAX_SNAPSHOT_BYTES,
        "calibrated snapshot must sit exactly on the ceiling"
    );
    let behind = SyncRequest {
        protocol: COLLAB_PROTOCOL_VERSION,
        credential: "reader-secret".to_owned(),
        last_sequence: 0,
    };
    let served = session
        .sync(&behind)
        .expect("exact-ceiling snapshot serves");
    let bytes = served.snapshot.expect("snapshot path carries bytes");
    assert_eq!(bytes.len(), MAX_SNAPSHOT_BYTES);

    let (over, _) = drive_to_checkpoint(pad + 1);
    let over_len = over
        .document()
        .to_canonical_json()
        .expect("serialize over")
        .len();
    assert_eq!(over_len, MAX_SNAPSHOT_BYTES + 1);
    // Rebuild a session over the over-ceiling document to hit the gate.
    let (permissions, authenticator) =
        PermissionStore::provision(&credentials()).expect("provision");
    let over_session = Session::open(over.document().clone(), permissions, authenticator);
    match over_session.sync(&behind) {
        Err(CollabError::TooLarge { claimed, max }) => {
            assert_eq!(max, MAX_SNAPSHOT_BYTES);
            assert_eq!(claimed, MAX_SNAPSHOT_BYTES + 1);
        }
        other => panic!("one byte past the ceiling must fail TooLarge, got {other:?}"),
    }
    let _ = session.drain_broadcasts();
}

/// Duplicate-actor determinism: lexicographic order decides which binding
/// stands and which credential is reported, with three bindings.
#[test]
fn duplicate_actor_report_names_the_lexicographically_later_credential() {
    let bindings = BTreeMap::from([
        (
            "aaa-first".to_owned(),
            CredentialBinding {
                actor: ActorId(9),
                role: Role::Owner,
            },
        ),
        (
            "mmm-second".to_owned(),
            CredentialBinding {
                actor: ActorId(9),
                role: Role::Editor,
            },
        ),
        (
            "zzz-third".to_owned(),
            CredentialBinding {
                actor: ActorId(9),
                role: Role::Reader,
            },
        ),
    ]);
    match PermissionStore::provision(&bindings) {
        Err(canary_collab::PermissionError::DuplicateActor { actor, credential }) => {
            assert_eq!(actor, 9);
            assert_eq!(credential, "mmm-second");
        }
        other => panic!("expected DuplicateActor for mmm-second, got {other:?}"),
    }
    // Distinct actors still provision, including the boundary u64::MAX.
    let distinct = BTreeMap::from([
        (
            "a".to_owned(),
            CredentialBinding {
                actor: ActorId(1),
                role: Role::Owner,
            },
        ),
        (
            "b".to_owned(),
            CredentialBinding {
                actor: ActorId(u64::MAX),
                role: Role::Editor,
            },
        ),
    ]);
    let (store, auth) = PermissionStore::provision(&distinct).expect("distinct provisions");
    assert_eq!(store.role_of(&ActorId(u64::MAX)), Some(Role::Editor));
    assert_eq!(auth["b"], (ActorId(u64::MAX), Role::Editor));
}

/// Idempotency horizon: retained keys replay idempotent after a trim,
/// evicted keys never silently reapply, and an evicted key reused with a
/// fresh revision is a new operation, never `IdConflict`.
#[test]
fn idempotency_horizon_retained_replays_evicted_resyncs_or_starts_new() {
    let (mut session, store) = harness();
    let total = MAX_RETAINED_OPERATIONS + 2;
    for index in 0..total {
        let mut request = edit(
            "owner-secret",
            &format!("op-{index}"),
            "entity.hero",
            index as u64,
        );
        request.payload = payload_at(index as f32);
        accepted(session.submit(&request, &store));
    }
    // A retained key replays idempotent: same envelope, no new sequence.
    let retained_index = total - 1;
    let mut retained = edit(
        "owner-secret",
        &format!("op-{retained_index}"),
        "entity.hero",
        retained_index as u64,
    );
    retained.payload = payload_at(retained_index as f32);
    let before = session.document().history.next_sequence.0;
    let replay = accepted(session.submit(&retained, &store));
    assert_eq!(replay.sequence, retained_index as u64 + 1);
    assert_eq!(session.document().history.next_sequence.0, before);

    // An evicted key with its original (now stale) revision conflicts.
    let mut stale = edit("owner-secret", "op-0", "entity.hero", 0);
    stale.payload = payload_at(0.0);
    let conflict = rejected(session.submit(&stale, &store));
    assert_eq!(conflict.code, RejectCode::RevisionConflict);

    // The same evicted key reused with the CURRENT revision and a new
    // payload is a fresh operation at the tip, not IdConflict.
    let tip = session.document().history.target_revision("hero").0;
    let mut fresh_reuse = edit("owner-secret", "op-0", "entity.hero", tip);
    fresh_reuse.payload = payload_at(4242.0);
    let envelope = accepted(session.submit(&fresh_reuse, &store));
    assert_eq!(envelope.sequence, before);
    assert_eq!(envelope.target_revision, tip + 1);
}

/// Restart fencing: epochs bump monotonically across two restarts and
/// each envelope carries the epoch of its own generation.
#[test]
fn restart_fencing_epoch_bumps_monotonically_and_envelopes_carry_their_generation() {
    let dir = std::env::temp_dir().join(format!(
        "canary-collab-epoch-{}-{}",
        std::process::id(),
        "fencing"
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    // Start clean for repeat runs.
    std::fs::remove_file(dir.join("project.json")).ok();
    std::fs::remove_file(dir.join("permissions.json")).ok();
    let project_path = dir.join("project.json");
    let permission_path = dir.join("permissions.json");
    base_document().save(&project_path).expect("genesis save");

    let open = || {
        let (permissions, authenticator) =
            PermissionStore::open_or_provision(&permission_path, &credentials())
                .expect("provision");
        let durable = canary_collab::FileProjectStore::new(&project_path);
        let document = durable.load().expect("load");
        (Session::open(document, permissions, authenticator), durable)
    };
    let (mut first, durable) = open();
    assert_eq!(first.epoch(), 1);
    let first_accept =
        accepted(first.submit(&edit("owner-secret", "op-1", "entity.hero", 0), &durable));
    assert_eq!(first_accept.epoch, 1);
    drop(first);

    let (mut second, durable) = open();
    assert_eq!(second.epoch(), 2);
    let second_accept = accepted(second.submit(
        &edit("editor-secret", "op-2", "entity.sidekick", 0),
        &durable,
    ));
    assert_eq!(second_accept.epoch, 2);
    assert_eq!(second_accept.sequence, 2);
    drop(second);

    let (third, _) = open();
    assert_eq!(third.epoch(), 3);
    assert_eq!(third.document().history.newest_retained().0, 2);
    std::fs::remove_dir_all(&dir).ok();
}

/// CAS: a future expected revision conflicts exactly like a stale one,
/// and target revisions stay independent of the project revision.
#[test]
fn future_expected_revision_conflicts_and_targets_stay_independent() {
    let (mut session, store) = harness();
    accepted(session.submit(&edit("owner-secret", "op-1", "entity.hero", 0), &store));
    // Future token: nothing is at revision 99 yet.
    let future = rejected(session.submit(&edit("owner-secret", "op-2", "entity.hero", 99), &store));
    assert_eq!(future.code, RejectCode::RevisionConflict);
    assert_eq!(future.target_revision, 1);
    assert!(future.refresh.is_some());
    // Project is at 1, sidekick untouched at 0: still no conflict there.
    let envelope =
        accepted(session.submit(&edit("editor-secret", "op-3", "entity.sidekick", 0), &store));
    assert_eq!(envelope.target_revision, 1);
    assert_eq!(envelope.project_revision, 2);
    // Malformed section names never become targets.
    for bad in [
        "entity.",
        "entity.a.b",
        "not-an-entity",
        "entity.with space",
    ] {
        let rejection = rejected(session.submit(&edit("owner-secret", "op-bad", bad, 0), &store));
        assert_eq!(rejection.code, RejectCode::UnknownTarget);
    }
}

/// Prefab veto at the session gate: every `Ok(false)` and `Err` arm maps
/// to `PrefabOverrideDenied`, while an instance carrying its own
/// transform entry is accepted.
#[test]
fn prefab_veto_error_and_false_arms_all_deny_but_owned_transform_accepts() {
    fn session_for(
        sections: BTreeMap<String, serde_json::Value>,
        prefabs: BTreeMap<String, canary_state::Prefab>,
    ) -> (Session, MemStore) {
        let mut doc = base_document();
        for (name, section) in sections {
            doc.sections.insert(name, section);
        }
        for (name, prefab) in prefabs {
            doc.prefabs.insert(name, prefab);
        }
        let (permissions, authenticator) =
            PermissionStore::provision(&credentials()).expect("provision");
        let store = MemStore {
            doc: RefCell::new(doc.clone()),
        };
        (Session::open(doc, permissions, authenticator), store)
    }
    // Non-object section: `transform_override_allowed` errors, session
    // maps it to PrefabOverrideDenied (never a panic, never StorageFailed).
    let (mut session, store) = session_for(
        BTreeMap::from([("entity.flat".to_owned(), serde_json::json!([1, 2, 3]))]),
        BTreeMap::new(),
    );
    let rejection =
        rejected(session.submit(&edit("owner-secret", "op-1", "entity.flat", 0), &store));
    assert_eq!(rejection.code, RejectCode::PrefabOverrideDenied);

    // Non-string prefab marker: `Ok(false)` arm.
    let (mut session, store) = session_for(
        BTreeMap::from([(
            "entity.weird".to_owned(),
            serde_json::json!({"prefab": 42, "canary.transform": hero_fields(0.0)}),
        )]),
        BTreeMap::new(),
    );
    let rejection =
        rejected(session.submit(&edit("owner-secret", "op-1", "entity.weird", 0), &store));
    assert_eq!(rejection.code, RejectCode::PrefabOverrideDenied);

    // Unknown prefab reference: `Ok(false)` arm.
    let (mut session, store) = session_for(
        BTreeMap::from([(
            "entity.lost".to_owned(),
            serde_json::json!({"prefab": "no-such-prefab"}),
        )]),
        BTreeMap::new(),
    );
    let rejection =
        rejected(session.submit(&edit("owner-secret", "op-1", "entity.lost", 0), &store));
    assert_eq!(rejection.code, RejectCode::PrefabOverrideDenied);

    // Chained prefab (base with its own base): `Ok(false)` arm.
    let chained = BTreeMap::from([
        (
            "root".to_owned(),
            canary_state::Prefab {
                base: None,
                overrides: BTreeMap::from([(TRANSFORM_SCHEMA_KEY.to_owned(), hero_fields(0.0))]),
            },
        ),
        (
            "mid".to_owned(),
            canary_state::Prefab {
                base: Some("root".to_owned()),
                overrides: BTreeMap::new(),
            },
        ),
        (
            "leaf".to_owned(),
            canary_state::Prefab {
                base: Some("mid".to_owned()),
                overrides: BTreeMap::new(),
            },
        ),
    ]);
    let (mut session, store) = session_for(
        BTreeMap::from([(
            "entity.chained".to_owned(),
            serde_json::json!({"prefab": "leaf"}),
        )]),
        chained,
    );
    let rejection =
        rejected(session.submit(&edit("owner-secret", "op-1", "entity.chained", 0), &store));
    assert_eq!(rejection.code, RejectCode::PrefabOverrideDenied);

    // Instance carrying its own transform entry: accepted.
    let owned = BTreeMap::from([(
        "goblin".to_owned(),
        canary_state::Prefab {
            base: None,
            overrides: BTreeMap::from([(TRANSFORM_SCHEMA_KEY.to_owned(), hero_fields(0.0))]),
        },
    )]);
    let (mut session, store) = session_for(
        BTreeMap::from([(
            "entity.chief".to_owned(),
            serde_json::json!({
                "prefab": "goblin",
                "canary.transform": hero_fields(0.0),
            }),
        )]),
        owned,
    );
    let envelope =
        accepted(session.submit(&edit("owner-secret", "op-1", "entity.chief", 0), &store));
    assert_eq!(envelope.sequence, 1);
}

/// Stage 8 is accept-only by construction: `SnapshotValue::from_json`
/// outputs always validate, so the numeric stage 6 stays the real reject
/// gate. This pins that contract instead of trusting it.
#[test]
fn stage8_codec_accepts_every_stage6_accept() {
    let valid = vec![
        TransformPayload::identity(),
        payload_at(f32::MAX),
        TransformPayload {
            scale: [0.0, -1.0, 0.5],
            ..TransformPayload::identity()
        },
    ];
    for payload in &valid {
        payload.validate().expect("stage 6 accepts");
        let canonical = canary_state::SnapshotValue::from_json(&payload.to_fields());
        canonical
            .validate()
            .expect("stage 8 accepts every stage-6 accept");
    }
    // And stage 6 still rejects what stage 8 would never see.
    for bad in [
        TransformPayload {
            translation: [f32::NAN, 0.0, 0.0],
            ..TransformPayload::identity()
        },
        TransformPayload {
            rotation: [0.0, 0.0, 0.0, 0.0],
            ..TransformPayload::identity()
        },
    ] {
        assert_eq!(bad.validate(), Err(RejectCode::InvalidPayload));
    }
}

/// Client-op-ID charset boundary: every allowed punctuation passes, every
/// path-traversal or whitespace shape fails, at exactly 128 bytes.
#[test]
fn client_op_id_charset_and_length_boundaries_hold() {
    assert!(ClientOpId::parse("aZ09-_.:").is_ok());
    for bad in [
        "has space",
        "tab\there",
        "slash/a",
        "back\\slash",
        "semi;colon",
        "star*",
    ] {
        assert_eq!(ClientOpId::parse(bad), Err(RejectCode::Malformed));
    }
    let at_bound = "o".repeat(128);
    assert!(ClientOpId::parse(&at_bound).is_ok());
    let past_bound = "o".repeat(129);
    assert_eq!(ClientOpId::parse(&past_bound), Err(RejectCode::Malformed));
    // The overlong ID fails at the session gate too (stage 2), not deeper.
    let (mut session, store) = harness();
    let mut request = edit("owner-secret", &past_bound, "entity.hero", 0);
    request.client_op_id = past_bound;
    assert_eq!(
        rejected(session.submit(&request, &store)).code,
        RejectCode::Malformed
    );
    // Target-entity local names reject the reserved `prefab` key.
    assert!(LogicalEntityId::from_local("prefab").is_err());
}
