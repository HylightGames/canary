# 0027. Start networking with bounded reliable replication and server input authority

**Status:** Accepted for `v0.0.15` (ratified 2026-09-30 against the WP1–WP4
implementation in the `dev` working tree; the four gates, API review, and
commit are still outstanding — see `docs/roadmap/status.md`). Builds on
ADRs 0007, 0013, 0021, 0022, and accepted ADR 0026 (Accepted for v0.0.14).

## Context

ADR 0007 chooses server authority, ECS-declared replication, and QUIC as the
default transport, but does not specify the first wire protocol. ADR 0026
items 2–3 require separate version domains; ADR 0022 requires deterministic
`SimulationInput`.
`World::query_changed_since` reports mutation but not removal or entity
destruction (R-33); runtime entity handles are not safe cross-process IDs.
The `.15` milestone must prove a real server and client process without
pretending prediction, rollback, or a production authentication system exists.

## Proposed decision

1. **Place networking in a separate `canary-net` subsystem crate.** Keep
   transport implementation behind a Canary-owned trait. The default
   adapter uses the accepted QUIC direction in ADR 0007; platform, core, and
   gameplay code do not name `quinn` types. No `quinn` types appear in
   public signatures outside the QUIC adapter.
2. **Use one reliable ordered QUIC stream for the first proof.** The stream
   carries handshake/control messages, initial state, authoritative deltas,
   and client `SimulationInput`. No datagram lane, prediction, reconciliation,
   or rollback ships in `.15`. A separate unreliable lane requires a measured
   latency/throughput need and its own delivery contract.
3. **Send an initial canonical snapshot followed by sequenced deltas.** Each
   connection establishes an authoritative baseline. Each delta names its
   base sequence and server simulation tick; entries are canonically ordered
   by stable network entity ID and schema ID. Three counters stay distinct:
   the wire session sequence carried by each delta's sequence/base-sequence
   fields, the simulation step count carried by `SimStateSnapshot.tick`, and
   the scheduler `Tick` consumed by change detection (risk-register rows
   R-32 and R-33). The dirty set for each delta comes from
   `World::query_changed_since` at the connection's last-acknowledged tick
   (ADR 0014:64–69); tombstones are the complementary removal/destruction
   mechanism for the R-33 gap `query_changed_since` does not cover
   (ADR 0014:110–124). The client validates the whole
   delta before applying it. A missing/unknown base causes a full resync,
   not a best-effort partial apply.
4. **Represent removals explicitly.** Replicated component changes,
   component removals, and entity destruction are separate operations.
   Removal/destruction tombstones remain until relevant acknowledgements
   arrive or the server discards the baseline and forces a full snapshot.
   Tombstone retention and snapshot cursors are bounded per client. The
   tombstone log lives in `canary-net` (one crate per subsystem); dirty-set
   computation stays in `canary-ecs` via `query_changed_since`. Each client
   holds a cursor into the log under a bounded cap, and a client whose
   cursor falls behind a dropped prefix resynchronizes from a fresh
   snapshot under the item-3 gap→resync rule.
5. **Use server-scoped entity identity on the wire.** A `NetEntityId` is
   assigned by the authoritative session and is distinct from runtime
   `Entity`, authored project identity, and content identity. Runtime entity
   handles never cross the wire. Authored IDs may be referenced in project
   collaboration messages but do not replace session network IDs for
   dynamically spawned gameplay entities.
6. **Clients submit logical input, not state writes.** Every input message
   identifies the connection's assigned player slot, the target simulation
   tick/frame, and a versioned action payload from `SimulationInput`. The
   server verifies ownership, schema/action compatibility, bounds, and
   accepted input window before queueing it. It rejects duplicate, stale,
   future-out-of-window, or unauthorized input with typed outcomes. Server
   simulation alone produces authoritative replicated state.
7. **Negotiate distinct compatibility domains.** The handshake carries
   protocol version, game/schema manifest, and any required plugin/API
   versions as separate fields. Reject unsupported protocol or required
   schema versions before applying data. No compatibility is implied merely
   because two builds share an engine release number.
8. **Bound untrusted work and memory.** Set maximum message/payload sizes,
   per-peer queued message/byte limits, in-flight snapshot limits, and
   admission/rate policy before enabling external peers. Backpressure must
   not block the simulation loop. A peer that exceeds a bound is refused or
   disconnected with a typed reason; queues never grow without limit.
9. **Do not infer production authentication from QUIC TLS.** The `.15`
   proof documents how a peer is identified and which peers may connect.
   The `.15` identity artifact is minimal: a session ID binding messages to
   a connection, the assigned player slot from item 6, and a self-hostable
   baseline — a team runs its own session server (ADR 0013:43–52). It is
   explicitly not user identity, certificate provisioning, or matchmaking.
   QUIC protects the transport, but certificate provisioning, user identity,
   account service, public matchmaking, and production abuse prevention are
   not delivered by this ADR. Treat all decoded messages as untrusted.
10. **Resynchronize after reconnect.** Reconnect uses a fresh authoritative
    snapshot unless the server can prove that the last acknowledged base and
    all subsequent deltas are still retained. A duplicate or reordered
    message is ignored/rejected by its explicit session sequence, never
    applied twice.

The exact public Rust type names remain open until a
bounded-decoding spike and consumer API review, but the wire encoding
discipline is not: it follows the `.14` codec discipline — the canonical
ordering rule (sorted entries in, deterministic bytes out) and postcard 1.x
binary encoding — with net-owned envelope types rather than the literal
`.14` state types (authored documents, simulation snapshots, and wire
messages are separate formats, and version domains stay separate per
ADR 0026 items 2–3). Every wire message travels inside a `NetEnvelope`
(protocol version + session sequence + SHA-256 checksum + opaque payload);
snapshots carry a `SnapshotEnvelope` (simulation tick + SHA-256 over the
canonical bytes); component schemas are named by stable string ids mapped
through `SchemaCodecs`. A replication envelope is always
constructed around replicated payloads; snapshot bytes never go on the
wire verbatim. `NetEntityId` (item 5) supplies the stable per-game
snapshot identity ADR 0026:160–161 conditions snapshot-byte exposure on,
discharging that revisit gate for the replication path. The existing
state architecture and this ADR are the records for that decision; no
additional planning document is required.

## Implementation deltas (WP3/WP4, ratified with the code 2026-09-30)

1. **Two acknowledgement types with separate encodings.** `InputAck`
   (`engine/canary-net/src/input.rs`: `input_seq` + `applied_tick`) is the
   per-input round-trip proof — the client advances its input cursor only
   when the ack echoes what it sent. `ClientAck`
   (`engine/canary-net/src/session.rs`: `last_applied` + `tombstone_cursor`
   + `last_acked_tick`) is the per-client cursor advancement the server's
   reclamation and dirty-set horizons move on. Each has its own
   `encode_ack`/`decode_ack` pair (`crate::input::` vs `crate::session::`;
   the crate root re-exports the input pair); they are never interchangeable.
2. **Outbound sequencing is caller-managed.** `encode_delta` and
   `NetEnvelope::seal` take explicit `sequence`/`base_sequence` values; the
   crate owns gates (`SequenceGate`) and cursors, never an allocator. Drivers
   and tests mint sequences with local counters.
3. **Per-delta journal replay is deliberately not implemented.** Deltas
   between the shared baseline and the live tip are superseded by the
   retained snapshots plus tombstones — a newer full state plus the removal
   log converges the same map with less bookkeeping
   (`engine/canary-net/src/resync.rs`). A workload that proves snapshot
   transfer too costly for its resync rate may justify a delta journal as
   follow-up work; nothing in the API precludes adding one beside the
   snapshot ring.
4. **Loopback proves session contracts, not QUIC/TLS.**
   (`engine/canary-net/src/loopback.rs`). Fault injection models what the
   session layer observes above a reliable stream (gaps, duplicates,
   reorders); QUIC/TLS behavior stays covered by the transport tests and the
   separate-process proof (`engine/canary-net/tests/session_roundtrip.rs`),
   which runs a real server child process over loopback QUIC with real TLS
   pinning and ALPN negotiation.
5. **Resync reuses `ClientAck` with `last_applied` informational.** A
   reconnect is always a new session (new session id, fresh record — old
   wire sequences are meaningless there); the client requests resync with a
   `ClientAck` whose `last_applied` rides along unread while the server
   plans from `last_acked_tick` + `tombstone_cursor` only
   (`engine/canary-net/src/loopback.rs`, resync proof).
6. **Ban/idle policy fail-open edges are contractual.**
   (`engine/canary-net/src/policy.rs`). Zero `max_consecutive_rejects` or
   zero `max_tracked_peers` disables banning (rejects still returned, never
   escalated); past the label cap the least-recently-seen label is evicted
   (fail-open for the evicted label only); `saturating_*` arithmetic means a
   backward clock delays enforcement but never false-triggers it.

## Alternatives considered

**Start with unreliable datagrams plus a custom reliability/ordering layer.**
Rejected for `.15`. It would introduce loss recovery, sequencing, retransmit,
and fragmentation behavior before the basic authority and state model is
proven. QUIC is already the accepted default.

**Use a reliable QUIC stream and an unreliable datagram lane immediately.**
Deferred. It may be needed for high-rate position data, but the first sample
does not need prediction or high-frequency lossy updates. Adding it now would
double the delivery contracts and obscure the snapshot/removal proof.

**Send a generic serialized `World` or raw runtime entity handles.**
Rejected. A `World` contains non-replicated and presentation state, while
runtime entity indices are process-local. Replication is an explicit
component/session identity boundary.

**Trust client-authored component deltas.** Rejected. Clients submit input;
the server validates it and alone creates authoritative state. Direct client
state writes contradict ADR 0007.

**Implement prediction/rollback as part of the first transport proof.**
Deferred. It depends on a proven snapshot profile, deterministic input, and
simulation re-execution. The `.15` goal is a bounded authoritative path, and
the `.1.0` sample does not require prediction.

**Treat encrypted transport as sufficient peer authentication.** Rejected.
TLS protects bytes on the connection; it does not decide who the peer is or
what that peer may do. The initial proof must state its limited identity
policy and cannot claim production readiness.

## Consequences

- `.15` requires stable session network IDs, replication opt-in, canonical
  iteration, and bounded tombstone history in addition to `.14` project and
  snapshot codecs.
- One ordered transport lane simplifies initial delivery but can head-of-line
  block large snapshots; bounded snapshots and resync are accepted first-step
  behavior. Add lanes only with measured evidence.
- A reconnect may transfer a full state snapshot; no persistent acknowledgement
  history is promised beyond the active session's bounded retention.
- The network wire format, project file format, component schema format, and
  plugin/API versions are related but separately versioned.
- Separate-process tests are mandatory. In-process loopback mocks alone do
  not satisfy the milestone.

## Revisit conditions

- A measured workload that misses its latency budget on the ordered stream
  may justify an unreliable datagram lane with explicit loss, sequencing, and
  resync semantics.
- A real public deployment must add reviewed identity, certificate, abuse,
  and authorization policy before Internet-facing release.
- Prediction or rollback requires a separate design for fixed-step replay,
  snapshot cost, input acknowledgement, and reconciliation.
