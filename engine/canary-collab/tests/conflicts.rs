//! Conflict chaos sweep over the collaboration op path (findings only).
//!
//! Deterministic scripted faults, no wall clock: concurrent same-key edits
//! submitted in both orders, stale-epoch replays, duplicate op-ID
//! redelivery (identical and conflicting payloads), and interleaved
//! permission changes with data ops. Every scenario asserts
//! single-winner determinism plus typed rejections: exactly one accept per
//! race, conflicts carrying the canonical refresh, and rejected requests
//! leaving no trace — never divergent accepted state.

use std::cell::RefCell;
use std::collections::BTreeMap;

use canary_collab::{
    AcceptEnvelope, ActorId, CredentialBinding, DurableProjectStore, EditRequest, PermissionStore,
    RejectCode, Role, Session, SessionEpoch, SubmitOutcome, TransformPayload,
    COLLAB_PROTOCOL_VERSION,
};
use canary_state::{
    AuthoredDocument, ProjectId, StateError, TRANSFORM_SCHEMA_KEY, TRANSFORM_SCHEMA_VERSION,
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

fn payload_at(x: f32) -> TransformPayload {
    TransformPayload {
        translation: [x, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    }
}

fn hero_fields(x: f64) -> serde_json::Value {
    serde_json::json!({
        "translation": [x, 0.0, 0.0],
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0],
    })
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

fn edit(credential: &str, op: &str, target: &str, expected: u64, x: f32) -> EditRequest {
    EditRequest {
        protocol: COLLAB_PROTOCOL_VERSION,
        credential: credential.to_owned(),
        client_op_id: op.to_owned(),
        target: target.to_owned(),
        expected_revision: expected,
        schema: TRANSFORM_SCHEMA_KEY.to_owned(),
        schema_version: TRANSFORM_SCHEMA_VERSION,
        payload: payload_at(x),
    }
}

fn accepted(outcome: SubmitOutcome) -> AcceptEnvelope {
    match outcome {
        SubmitOutcome::Accepted(accepted) => accepted.envelope,
        SubmitOutcome::Rejected(rejection) => {
            panic!(
                "expected accept, got {:?}: {}",
                rejection.code, rejection.message
            )
        }
    }
}

fn rejected(outcome: SubmitOutcome) -> canary_collab::Rejection {
    match outcome {
        SubmitOutcome::Rejected(rejection) => rejection,
        SubmitOutcome::Accepted(accepted) => {
            panic!(
                "expected reject, got accept at seq {}",
                accepted.envelope.sequence
            )
        }
    }
}

fn translation_of(session: &Session) -> serde_json::Value {
    session.document().sections["entity.hero"]["canary.transform"]["translation"].clone()
}

/// Concurrent same-key edits: both submission orders yield exactly one
/// winner and one typed conflict carrying the canonical refresh — the
/// first-submitted op deterministically wins, and accepted state never
/// diverges within a run.
#[test]
fn concurrent_same_key_edits_have_a_single_deterministic_winner() {
    for (first_cred, first_op, first_x, second_cred, second_op, second_x) in [
        (
            "owner-secret",
            "op-1",
            1.0_f32,
            "editor-secret",
            "op-2",
            9.0_f32,
        ),
        (
            "editor-secret",
            "op-2",
            9.0_f32,
            "owner-secret",
            "op-1",
            1.0_f32,
        ),
    ] {
        let (mut session, store) = harness();
        let won = accepted(session.submit(
            &edit(first_cred, first_op, "entity.hero", 0, first_x),
            &store,
        ));
        assert_eq!(won.sequence, 1, "winner takes sequence 1");
        let lost = rejected(session.submit(
            &edit(second_cred, second_op, "entity.hero", 0, second_x),
            &store,
        ));
        assert_eq!(
            lost.code,
            RejectCode::RevisionConflict,
            "loser gets a typed conflict, not a silent drop"
        );
        assert_eq!(lost.target_revision, 1);
        assert_eq!(
            lost.refresh,
            Some(payload_at(first_x)),
            "conflict carries the canonical current target (the winner)"
        );
        assert_eq!(
            session.document().history.accepted.len(),
            1,
            "exactly one accepted record: no divergent accepted state"
        );
        assert_eq!(
            translation_of(&session),
            serde_json::json!([first_x, 0.0, 0.0]),
            "durable state holds exactly the winner's payload"
        );
        // The loser retries against the refresh and converges next.
        let retry = accepted(session.submit(
            &edit(second_cred, "op-retry", "entity.hero", 1, second_x),
            &store,
        ));
        assert_eq!(retry.sequence, 2);
        assert_eq!(retry.target_revision, 2);
    }
}

/// Stale-epoch replay plus duplicate op-ID redelivery: an identical replay
/// (even under a bumped fencing epoch) returns the prior accept without
/// double-applying; a stale revision conflicts typed; a reused op-ID with a
/// different payload is an `IdConflict` — never a silent overwrite.
#[test]
fn stale_epoch_replay_is_idempotent_never_a_double_apply() {
    let (mut session, store) = harness();
    let request = edit("owner-secret", "op-1", "entity.hero", 0, 1.0);
    let first = accepted(session.submit(&request, &store));
    assert_eq!(first.sequence, 1);
    assert_eq!(first.epoch, 1);

    // Identical redelivery on the live session: same envelope, no advance.
    let replay = accepted(session.submit(&request, &store));
    assert_eq!(replay, first, "identical replay returns the prior result");
    assert_eq!(
        session.document().history.next_sequence.0,
        2,
        "no new sequence assigned"
    );
    assert!(
        session.drain_broadcasts().len() == 1,
        "replay queues no second broadcast"
    );

    // Same op under a bumped fencing epoch (restart generation): still the
    // same operation, still no double apply. The envelope is re-minted
    // under the new generation's epoch while sequence, revisions, and
    // payload stay pinned to the original accept.
    let (permissions, authenticator) =
        PermissionStore::provision(&credentials()).expect("provision");
    let mut epoch2 = permissions;
    epoch2.epoch = SessionEpoch(2);
    let mut restarted = Session::open(session.document().clone(), epoch2, authenticator);
    let replay2 = accepted(restarted.submit(&request, &store));
    assert_eq!(replay2.sequence, 1, "no double apply across the restart");
    assert_eq!(replay2.target_revision, 1);
    assert_eq!(replay2.payload, payload_at(1.0));
    assert_eq!(
        restarted.document().history.accepted.len(),
        1,
        "history still holds exactly one record"
    );
    assert_eq!(
        translation_of(&restarted),
        serde_json::json!([1.0, 0.0, 0.0]),
        "state still holds exactly the winner's payload"
    );

    // Stale-revision replay of the same bytes is a typed conflict now that
    // the caller's cursor is behind (fresh op-ID, old revision).
    let conflict = rejected(session.submit(
        &edit("owner-secret", "op-stale", "entity.hero", 0, 5.0),
        &store,
    ));
    assert_eq!(conflict.code, RejectCode::RevisionConflict);
    assert_eq!(conflict.target_revision, 1);

    // Reused op-ID with a different payload: typed id conflict, state kept.
    let clash =
        rejected(session.submit(&edit("owner-secret", "op-1", "entity.hero", 1, 7.0), &store));
    assert_eq!(clash.code, RejectCode::IdConflict);
    assert_eq!(
        translation_of(&session),
        serde_json::json!([1.0, 0.0, 0.0]),
        "conflicting redelivery mutates nothing"
    );
}

/// Interleaved permission change and data op: a forbidden submit records
/// nothing (the next sequence is untouched), a later out-of-protocol grant
/// lets the same actor's next op through, and a demotion still answers the
/// old op's identical replay from the idempotency record while refusing new
/// ops — accepted state never diverges across the interleaving.
#[test]
fn interleaved_permission_change_and_data_op_stays_typed() {
    let (mut session, store) = harness();
    // Reader submits: forbidden, connection usable, nothing recorded.
    let denial = rejected(session.submit(
        &edit("reader-secret", "op-1", "entity.hero", 0, 1.0),
        &store,
    ));
    assert_eq!(denial.code, RejectCode::Forbidden);
    assert_eq!(session.document().history.next_sequence.0, 1);
    assert!(session.drain_broadcasts().is_empty());
    assert_eq!(
        translation_of(&session),
        serde_json::json!([0.0, 0.0, 0.0]),
        "denied op mutates nothing"
    );

    // Out-of-protocol grant: the same actor is now an editor. A fresh
    // session over the same document (durable state is untouched) accepts
    // the actor's next op at sequence 1 — no gap, no ghost entry.
    let granted_credentials = BTreeMap::from([
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
                role: Role::Editor,
            },
        ),
    ]);
    let (granted_permissions, granted_auth) =
        PermissionStore::provision(&granted_credentials).expect("grant provisions");
    let mut granted = Session::open(
        session.document().clone(),
        granted_permissions,
        granted_auth,
    );
    let envelope = accepted(granted.submit(
        &edit("reader-secret", "op-2", "entity.hero", 0, 3.0),
        &store,
    ));
    assert_eq!(envelope.sequence, 1, "denial left no gap behind");
    assert_eq!(envelope.actor, 3);
    assert_eq!(
        translation_of(&granted),
        serde_json::json!([3.0, 0.0, 0.0]),
        "post-grant op commits exactly its payload"
    );

    // Demotion: the actor is a reader again. The identical replay of the
    // accepted op still answers from the idempotency record (replay is the
    // same edit returning, not a new edit), while a new op is forbidden.
    let (demoted_permissions, demoted_auth) =
        PermissionStore::provision(&credentials()).expect("demotion provisions");
    let mut demoted = Session::open(
        granted.document().clone(),
        demoted_permissions,
        demoted_auth,
    );
    let replay = accepted(demoted.submit(
        &edit("reader-secret", "op-2", "entity.hero", 0, 3.0),
        &store,
    ));
    assert_eq!(replay.sequence, 1, "replay re-applies nothing");
    assert_eq!(
        demoted.document().history.accepted.len(),
        1,
        "still exactly one accepted record after the interleaving"
    );
    let fresh_denial = rejected(demoted.submit(
        &edit("reader-secret", "op-3", "entity.hero", 1, 4.0),
        &store,
    ));
    assert_eq!(fresh_denial.code, RejectCode::Forbidden);
    assert_eq!(
        translation_of(&demoted),
        serde_json::json!([3.0, 0.0, 0.0]),
        "denied post-demotion op mutates nothing"
    );
}
