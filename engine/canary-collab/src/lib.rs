//! Server-authoritative live-collaboration operations (ADR 0028, `.16`).
//!
//! # What this crate is
//!
//! `canary-collab` owns the `.16` operation path: the single
//! complete-local-transform-replacement operation on an existing authored
//! entity, server-minted actor identity with three server-owned roles,
//! target-scoped compare-and-set conflicts, `(actor, client-op-id)`
//! idempotency, bounded retained history with checkpoint envelopes, and the
//! ordered validation pipeline. It programs against [`canary_state`] for
//! durable commits and the prefab veto, never against a transport: the
//! session consumes decoded requests and emits ordered outcomes, and the
//! composition owner (`canary-runtime`) moves those bytes across the
//! [`canary_net`](https://docs.rs/canary-net) transport trait.
//!
//! # Shipped slice (v0.0.16 WP2)
//!
//! - [`op`]: the one operation and its numeric validation (finite floats,
//!   normalized quaternion, content-agnostic scale).
//! - [`actor`]: server-minted [`actor::ActorId`], [`actor::Role`], and the
//!   bounded [`actor::ClientOpId`].
//! - [`permissions`]: the server-side [`permissions::PermissionStore`]
//!   (roles plus fencing epoch, atomic JSON file, provisioned
//!   out-of-protocol from pre-shared credentials).
//! - [`wire`]: bounded `postcard` request/response/sync codecs plus the
//!   frame-tag registry and its byte layout (owned here, versioned with
//!   the protocol version; dispatch lives on [`session::Session`]).
//! - [`session`]: the authoritative [`session::Session`] with validation
//!   stages 1–11, durable commit before ack, ordered broadcast outbox, and
//!   reconnect recovery through retained tail or checkpointed snapshot.
//!
//! Explicitly NOT in this slice: generic patches, per-entity ACLs,
//! in-protocol grant/revoke, undo/redo, offline merge, CRDT semantics,
//! presence, or editor UI. No `TombstoneLog`, `NetSequence`, or `Tick`
//! appears anywhere in this op path; no client-supplied identity is ever
//! trusted; there is no global CAS, no LWW, and no history rewrite.

pub mod actor;
pub mod error;
pub mod op;
pub mod permissions;
pub mod session;
pub mod wire;

pub use actor::{ActorId, ClientOpId, Role};
pub use error::{CollabError, PermissionError, RejectCode};
pub use op::{TransformPayload, QUAT_NORM_EPSILON};
pub use permissions::{CredentialBinding, PermissionStore, SessionEpoch};
pub use session::{
    AcceptedOperation, DurableProjectStore, FileProjectStore, Rejection, Session, SubmitOutcome,
    SyncResult, MAX_OUTBOX_MESSAGES, MAX_RETAINED_OPERATIONS,
};
pub use wire::{
    decode_request, decode_response, decode_sync_request, decode_sync_response, encode_request,
    encode_response, encode_sync_request, encode_sync_response, split_tagged_body, tag_body,
    tag_edit_body, AcceptEnvelope, EditRequest, RejectEnvelope, SyncRequest, SyncResponse,
    WireResponse, COLLAB_PROTOCOL_VERSION, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
    MAX_SNAPSHOT_BYTES, MAX_SYNC_REQUEST_BYTES, TAG_EDIT, TAG_HOST_RESERVED_MIN, TAG_SYNC,
};
