//! Server-authoritative replication transport for Canary (ADR 0027, accepted
//! 2026-09-30).
//!
//! # What this crate is
//!
//! `canary-net` owns replicated-session networking: stable server-scoped
//! network identity, the wire envelope, bounded decoding gates, a
//! Canary-owned [`transport::NetTransport`] trait, and the replication
//! representation — canonical snapshots, sequenced deltas, and the durable
//! removal/destruction log. The default adapter is QUIC via `quinn`
//! ([`transport::QuinnTransport`]); gameplay, platform, and core code
//! program against the trait, never against `quinn` types.
//!
//! # Shipped slices (v0.0.15 spike WP1, representation WP2, session WP3)
//!
//! - Version-domain and identity newtypes ([`ids`]), including the
//!   simulation-step counter ([`ids::SimTick`]) that keeps the three time
//!   domains apart: wire session sequence vs simulation step vs scheduler
//!   tick.
//! - [`limits::NetLimits`] bounds and [`error::NetError`] /
//!   [`error::DisconnectReason`] typed rejections, with per-operation
//!   transport variants and retryable-vs-fatal ([`error::NetError::is_retryable`])
//!   versus resync-required ([`error::NetError::resync_required`]) policy,
//!   plus session-layer queue-full, invalid-input, and unknown-schema
//!   variants.
//! - Length-prefixed framing ([`frame`]) whose limit gate runs before any
//!   allocation, and a `postcard` envelope codec ([`envelope`]) that rejects
//!   trailing bytes and verifies the SHA-256 checksum before the payload is
//!   trusted.
//! - [`sequence::SequenceGate`] rejecting duplicate/reordered session
//!   sequences so a message is never applied twice.
//! - [`transport::NetTransport`] plus [`transport::QuinnTransport`], with a
//!   compiling, passing pinned-handshake test over a real QUIC connection.
//! - Server-scoped entity identity ([`mapping::NetEntityMap`]): tuple-keyed
//!   assignment that survives slot recycling without aliasing.
//! - Type-level replication opt-in ([`policy::ReplicationRegistry`]),
//!   composing with the entity-level `Replicated` marker in `canary-ecs`.
//! - Canonical snapshots and sequenced deltas ([`replication`]):
//!   sort-then-checksum bytes, base-sequence basing, and
//!   validate-all-before-apply semantics.
//! - The durable tombstone log ([`tombstone::TombstoneLog`]) with per-client
//!   cursors, bounded retention, ack-gated reclamation, and drop→resync.
//! - Typed handshake vocabulary ([`handshake`]): `Hello` / `Welcome` /
//!   `Reject` with independent protocol, schema-manifest, game, and
//!   plugin/API checks — version mismatch is a typed reject followed by
//!   close. The log-only client label is bounded at decode.
//! - Client input ingress validation ([`input`]): per-client
//!   validate-all-before-apply (ownership, sequence, bounds, action-schema
//!   plus exact action-version compatibility, input window); malformed
//!   input is a typed error with no state mutation and the connection
//!   stays alive.
//! - Bounded per-client queues ([`queue`]) with explicit backpressure:
//!   ingress overflow disconnects the offender, egress overflow drops the
//!   oldest stale state the next delta supersedes.
//! - Per-client session records ([`session`]): slot, session id, wire and
//!   tombstone cursors, input validator, and both queues, with whole-record
//!   disconnect leaving no residue.
//! - Per-schema payload codecs ([`codec`]): typed registration of
//!   encode/decode fns for component bytes inside deltas; unknown schemas
//!   fail as resync-required, never panic.
//!
//! # Hardened slices (v0.0.15 WP4)
//!
//! - Connection admission and liveness policy ([`policy`]): configurable
//!   idle deadlines per client enforced on tick (disconnects with
//!   [`DisconnectReason::IdleTimeout`]), and temporary bans with expiry for
//!   peer labels with repeated handshake rejects (refused as
//!   [`RejectReason::TemporarilyBanned`](crate::handshake::RejectReason::TemporarilyBanned)).
//!   Both run on a caller-supplied `u64` clock — no wall-clock reads, so
//!   tests drive policy on a fake clock.
//! - Session counters with bounded label sets ([`metrics`]): exact global
//!   aggregates plus a capped per-client breakdown, evicted on disconnect.
//!   Past the cap new clients count globally but earn no row, so churn
//!   cannot grow memory while aggregates stay available.
//! - Reconnect/resync baseline retention ([`resync`]): a bounded ring of
//!   recent authoritative snapshots consulted with the session-surviving
//!   tombstone log. A reconnect (always a new session) replays its shared
//!   baseline plus retained tombstones when covered, else takes a live
//!   full snapshot — convergence without pretending across a gap.
//! - In-process loopback transport and deterministic fault injection
//!   ([`loopback`]): the same [`transport::NetTransport`] trait over
//!   channels (no sockets/TLS — Quinn coverage stays in
//!   [`transport`] and `tests/session_roundtrip.rs`), plus counter-based
//!   loss/duplication/reorder/latency faults. The resync end-to-end proof
//!   (drop mid-session, reconnect, incremental replay, live deltas after)
//!   and the network-conditions matrix (convergence + no double-apply per
//!   cell, with each fault cell proving its fault fired) run here
//!   deterministically.
//!
//! Dirty-set computation stays in `canary-ecs` via
//! `World::query_changed_since(last-acked tick)` (ADR 0014) — this crate
//! never builds a parallel dirty-flag system. It only owns the log, the
//! identity, and the wire representation those dirty sets are captured into.
//!
//! Explicitly NOT yet implemented: per-delta journal replay (retained
//! snapshots plus tombstones supersede it — see [`resync`]), an unreliable
//! datagram lane (needs measured latency/throughput evidence per ADR 0027),
//! and production identity (certificate pinning remains a proof-only peer
//! policy — see [`transport::QuinnTransport::client`]).

pub mod codec;
pub mod envelope;
pub mod error;
pub mod frame;
pub mod handshake;
pub mod ids;
pub mod input;
pub mod limits;
pub mod loopback;
pub mod mapping;
pub mod metrics;
pub mod policy;
pub mod queue;
pub mod replication;
pub mod resync;
pub mod sequence;
pub mod session;
pub mod tombstone;
pub mod transport;

pub use codec::{identity_codec, DecodeFn, EncodeFn, SchemaCodec, SchemaCodecs};
pub use envelope::NetEnvelope;
pub use error::{DisconnectReason, NetError};
pub use frame::{decode_frame_len, encode_frame, FRAME_HEADER_LEN};
pub use handshake::{
    decide_handshake, decode_hello, decode_reject, decode_welcome, encode_hello, encode_reject,
    encode_welcome, Hello, Reject, RejectReason, Welcome, MAX_CLIENT_LABEL_BYTES,
};
pub use ids::{
    GameVersion, NetEntityId, NetSequence, PluginApiVersion, ProtocolVersion,
    SchemaManifestVersion, SimTick, PROTOCOL_VERSION_1,
};
/// [`InputAck`] wire codec: the per-input round-trip proof. The crate root
/// re-exports exactly this pair; the per-client cursor codec
/// ([`ClientAck`](crate::session::ClientAck)) keeps its own same-named
/// `encode_ack` / `decode_ack` at `crate::session::` — the two ack types
/// are never interchangeable (ADR 0027, implementation delta 1).
pub use input::{
    decode_ack, decode_input, encode_ack, encode_input, ClientInput, InputAck, InputValidator,
    ValidatedInput,
};
pub use limits::{
    NetLimits, DEFAULT_MAX_MESSAGE_BYTES, DEFAULT_MAX_QUEUED_BYTES, DEFAULT_MAX_QUEUED_MESSAGES,
};
pub use loopback::{FaultProfile, FaultySend, LoopbackRecv, LoopbackSend, LoopbackTransport};
pub use mapping::NetEntityMap;
pub use metrics::{ClientCounters, GlobalCounters, NetMetrics, DEFAULT_MAX_TRACKED_CLIENTS};
pub use policy::{
    BanPolicy, ConnectionPolicy, HandshakeGate, IdlePolicy, IdleTracker, ReplicationRegistry,
    DEFAULT_BAN_DURATION_TICKS, DEFAULT_MAX_HANDSHAKE_REJECTS, DEFAULT_MAX_IDLE_TICKS,
    DEFAULT_MAX_TRACKED_PEERS,
};
pub use queue::{BoundedQueue, PushOutcome, QueuePolicy};
pub use replication::{
    decode_delta, decode_snapshot, encode_delta, encode_snapshot, Delta, ReplicatedEntry, Snapshot,
};
pub use resync::{BaselineRetention, BaselineSnapshot, ResyncPlan, DEFAULT_BASELINE_CAP};
pub use sequence::SequenceGate;
pub use session::{ClientId, ClientSession, SessionTable};
pub use tombstone::{LoggedTombstone, Tombstone, TombstoneKind, TombstoneLog};
pub use transport::{NetRecv, NetSend, NetTransport, QuinnTransport, QUIC_ALPN_CANARY_1};
