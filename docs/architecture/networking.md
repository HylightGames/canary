# Networking & Multiplayer

Architecture for the planned networking subsystem (see
[`docs/roadmap/v0.1.0-plan.md`](../roadmap/v0.1.0-plan.md)). No `canary-net`
or transport implementation exists yet. The initial networking milestone is
planned for `v0.0.15`, after project state. This is one of the
areas where [`docs/research/engine-comparisons.md`](../research/engine-comparisons.md)
most directly informed the design: retrofitting multiplayer onto an engine
whose ECS and simulation loop weren't designed with replication in mind is
one of the most consistently painful experiences in game development, so
foundational decisions were recorded before implementation rather than
waiting for the transport milestone.

See [ADR 0007](../decisions/architecture-decision-records/0007-networking-and-multiplayer-model.md)
for the decision record.

## Authority model: server-authoritative by default

The default model is server-authoritative: the server owns the true
simulation state; clients send inputs and intents, and may write their own
predicted replica, but cannot commit authoritative state without authority.
They render that local prediction pending server confirmation. This is the
standard model for competitive and cheat-resistant multiplayer (and the
model most existing engines' official
networking add-ons converge on), and it composes cleanly with a
fixed-timestep, ECS-driven simulation (see [physics.md](physics.md)).

Peer-to-peer and listen-server topologies (one player's client also acts as
host) are supported as a *deployment* choice layered on the same
authority model — "the server" doesn't have to mean a dedicated data-center
process, just a process that's authoritative.

## Client prediction & reconciliation

```mermaid
sequenceDiagram
    participant Client
    participant Server

    Client->>Client: Apply local input immediately (predicted)
    Client->>Server: Send input/intent
    Server->>Server: Simulate authoritatively (fixed timestep)
    Server-->>Client: Send authoritative state snapshot
    Client->>Client: Compare predicted vs. authoritative state
    alt Mismatch beyond tolerance
        Client->>Client: Reconcile: rewind to snapshot, replay unacked inputs
    else Match
        Client->>Client: Discard acknowledged predicted state
    end
```

Client-side prediction hides latency for the local player; reconciliation
corrects drift when the server's authoritative outcome disagrees with the
client's local guess. This requires the simulation to be re-runnable
("rewind and replay inputs") which is another reason the ECS's system data-
access declarations (see
[core-runtime.md](core-runtime.md#ecs-architecture)) matter early: a
scheduler that already knows which systems read/write which state is much
closer to being able to snapshot and re-simulate a slice of frames than one
that doesn't.

## Replication is an ECS concept, not a side channel

Components intended for network replication are marked as such
(conceptually, a `Replicated` marker or trait bound). The networking
subsystem can use ECS mutation change detection (see
[core-runtime.md](core-runtime.md#ecs-architecture)), but that alone is
not a replication log: component removal and entity destruction need
durable records, while snapshots and wire output need canonical ordering.
The authority model also distinguishes server-owned truth from client
predicted state; a client may simulate a local prediction but cannot commit
authoritative state without authority (ADR 0021 Amendment 3). These
contracts shape future delta/full-state transport without prescribing the
wire mechanism here.

## Transport: QUIC as the default

**Default transport: QUIC**, via the `quinn` crate — a mature, widely used
(tens of millions of downloads), pure-Rust, async QUIC implementation.
Rationale:

- QUIC natively multiplexes independent streams without head-of-line
  blocking across them, which maps well onto "some game state is reliable
  (inventory changes), some is unreliable-but-frequent (position updates)"
  without hand-rolling that distinction over raw UDP.
- Built-in TLS 1.3 encryption by default, rather than optional/bolted-on
  encryption.
- A pure-Rust implementation keeps the transport layer dependency-simple and
  consistent with the rest of the core (see
  [ADR 0002](../decisions/architecture-decision-records/0002-primary-language-selection.md)).

Raw UDP remains available underneath for subsystems that want unreliable,
unordered, unencrypted datagrams directly (e.g., very latency-sensitive
position updates where an application-level protocol on top of UDP is
preferred to QUIC's stream model) — the transport is a trait boundary like
rendering and physics, not a hard-coded dependency on `quinn` throughout the
engine.

## Rollback netcode: architected for, not required

Client prediction/reconciliation and full rollback netcode (resimulating
several frames of client state after a misprediction) are explicitly outside
the `v0.1.0` requirement; see the
[`v0.1.0 plan`](../roadmap/v0.1.0-plan.md#explicitly-not-required-for-v010).
But because the ECS is designed around explicit system data-access
declarations and a fixed timestep from the start (see
[core-runtime.md](core-runtime.md)), adding rollback support later is
intended to be an additive capability (snapshot/restore + deterministic
re-simulation of a window of frames) rather than a rearchitecture — the same
"bootstrap for the common case, architect for the harder one" pattern used
in [rendering.md](rendering.md) for the RHI.

## Status in this foundation

No `canary-net` crate, transport, or replication marker types exist yet.
Stable component schema identity is implemented. Snapshot APIs, durable
removal/destruction history, canonical ordering, and frame-tagged simulation
input are specified or required by ADRs 0020–0022 but are not implemented.
The first end-to-end proof must use a separate server and client process;
see [`v0.0.15`](../roadmap/v0.1.0-plan.md#v0015--networking).
