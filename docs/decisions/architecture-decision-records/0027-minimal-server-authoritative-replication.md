# 0027. Start networking with bounded reliable replication and server input authority

**Status:** Proposed for `v0.0.15`; review before stabilizing the first wire
format. Builds on ADRs 0007, 0013, 0021, 0022, and proposed ADR 0026.

## Context

ADR 0007 chooses server authority, ECS-declared replication, and QUIC as the
default transport, but does not specify the first wire protocol. ADR 0022
requires separate version domains and deterministic `SimulationInput`.
`World::query_changed_since` reports mutation but not removal or entity
destruction (R-33); runtime entity handles are not safe cross-process IDs.
The `.15` milestone must prove a real server and client process without
pretending prediction, rollback, or a production authentication system exists.

## Proposed decision

1. **Place networking in a separate `canary-net` subsystem crate.** Keep
   transport implementation behind a Canary-owned trait. The default
   adapter uses the accepted QUIC direction in ADR 0007; platform, core, and
   gameplay code do not name `quinn` types.
2. **Use one reliable ordered QUIC stream for the first proof.** The stream
   carries handshake/control messages, initial state, authoritative deltas,
   and client `SimulationInput`. No datagram lane, prediction, reconciliation,
   or rollback ships in `.15`. A separate unreliable lane requires a measured
   latency/throughput need and its own delivery contract.
3. **Send an initial canonical snapshot followed by sequenced deltas.** Each
   connection establishes an authoritative baseline. Each delta names its
   base sequence and server simulation tick; entries are canonically ordered
   by stable network entity ID and schema ID. The client validates the whole
   delta before applying it. A missing/unknown base causes a full resync,
   not a best-effort partial apply.
4. **Represent removals explicitly.** Replicated component changes,
   component removals, and entity destruction are separate operations.
   Removal/destruction tombstones remain until relevant acknowledgements
   arrive or the server discards the baseline and forces a full snapshot.
   Tombstone retention and snapshot cursors are bounded per client.
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
   QUIC protects the transport, but certificate provisioning, user identity,
   account service, public matchmaking, and production abuse prevention are
   not delivered by this ADR. Treat all decoded messages as untrusted.
10. **Resynchronize after reconnect.** Reconnect uses a fresh authoritative
    snapshot unless the server can prove that the last acknowledged base and
    all subsequent deltas are still retained. A duplicate or reordered
    message is ignored/rejected by its explicit session sequence, never
    applied twice.

The exact message encoding and public Rust type names remain open until a
bounded-decoding spike and consumer API review. The existing state architecture
and this ADR are the records for that decision; no additional planning document
is required.

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
