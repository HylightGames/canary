# 0007. Server-authoritative networking over QUIC, replication as an ECS concept

**Status:** Accepted

## Context

"Strong multiplayer foundations" is a stated goal, and
[`docs/research/engine-comparisons.md`](../../research/engine-comparisons.md)
consistently surfaces the same lesson across existing engines: multiplayer
support retrofitted onto a simulation/ECS design that didn't anticipate it
is one of the most painful, invasive changes an engine can go through later.
This decision exists to seed the right foundations early (well before Era 4,
when networking is actually built) rather than let the ECS and simulation
loop harden around single-player assumptions first.

## Decision

Full detail in
[`docs/architecture/networking.md`](../../architecture/networking.md).
Summary of the binding decisions:

- **Server-authoritative by default**, with client-side prediction and
  reconciliation for the local player. Peer-to-peer/listen-server topologies
  are a deployment choice layered on the same authority model, not a
  different one.
- **Replication is declared on ECS components**, and the networking
  subsystem observes changes via the ECS's change-detection query filters
  to send deltas — replication is not a parallel, hand-maintained data
  model living beside the ECS.
- **QUIC (via the `quinn` crate) is the default transport**, chosen for
  native stream multiplexing without cross-stream head-of-line blocking,
  built-in TLS 1.3, and a mature, widely-used pure-Rust implementation
  consistent with [ADR 0002](0002-primary-language-selection.md). The
  transport itself sits behind a trait boundary, following the same
  replaceable-subsystem pattern as rendering ([ADR 0004](0004-rendering-abstraction-strategy.md))
  and physics.
- **Full rollback netcode is explicitly out of scope for early milestones**,
  but the ECS's fixed-timestep, explicit-data-access design is chosen now
  specifically so rollback support is an additive capability later rather
  than a rearchitecture.

## Alternatives considered

**Client-authoritative or trust-the-client models.** Rejected as the
default: acceptable for some cooperative/non-competitive genres, but not a
sound default for a general-purpose engine expected to support competitive
multiplayer; server-authoritative can still support cooperative games fine,
while the reverse isn't true.

**Raw UDP as the default transport, hand-rolled reliability layer.**
Rejected as the *default*: QUIC provides multiplexed streams, encryption,
and connection migration out of the box, and a mature Rust implementation
already exists — hand-rolling equivalent functionality would be re-solving a
solved problem. Raw UDP remains available underneath the transport trait for
subsystems that specifically want unreliable, low-level datagrams.

**TCP as the default transport.** Rejected: head-of-line blocking across
independent logical streams (a large asset download blocking a
latency-sensitive position update sharing the same connection) is exactly
the problem QUIC's stream multiplexing solves, and this problem is common
enough in games that defaulting to TCP would just mean reimplementing a
UDP-based solution later anyway.

**Design the ECS without networking in mind, add replication as a bolt-on
system later.** Rejected: this is precisely the pattern
[`docs/research/engine-comparisons.md`](../../research/engine-comparisons.md)
identifies as consistently painful across existing engines. Seeding
replication as a first-class ECS concept now (even though the networking
subsystem itself isn't built until Era 4) costs little today and avoids a
much larger redesign later.

## Consequences

- The ECS's system data-access declarations and change-detection design
  (see [`docs/architecture/core-runtime.md`](../../architecture/core-runtime.md))
  need to account for replication's needs (snapshotting, delta detection)
  from the point they're actually implemented (Era 2), even though
  networking itself lands later (Era 4). This is now formalized rather
  than just anticipated: [ADR 0014](0014-change-detection-as-shared-primitive.md),
  written once `v0.0.2` actually shipped `World::query_changed_since`,
  commits replication to consuming that primitive directly rather than
  designing its own delta-detection from scratch — for component
  *mutation*. ADR 0014 is explicit that it does not yet cover component
  removal or entity destruction (risk R-33): this networking model still
  needs an answer for "the client must be told an entity/component is
  gone," and that answer isn't `query_changed_since` alone. Worth
  keeping in view when Era 4 actually starts, not rediscovering then.
- `quinn` becomes a dependency of the (future) `canary-net` crate, not of
  engine core — consistent with the transport being a replaceable
  subsystem, not a hard-wired dependency.
- Genres/architectures that want a fundamentally different authority model
  (fully deterministic lockstep, for instance) can still build on Canary's
  ECS, but would implement their own authority/replication layer against the
  same underlying change-detection primitives rather than using the default
  `canary-net` subsystem as-is.
