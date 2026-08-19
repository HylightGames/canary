# 0013. Live collaboration is server-authoritative (client–server–client), not peer-to-peer or CRDT-based

**Status:** Accepted (topology and authority model decided; wire
protocol, operation schema, and permission-model specifics remain
implementation-time work — the same posture already taken by
[ADR 0007](0007-networking-and-multiplayer-model.md) for gameplay
networking, which shipped as architecture long before implementation)

## Context

[ADR 0012](0012-project-state-as-a-versionable-graph.md) named real-time
collaborative editing as a genuinely open question with two credible
directions — server-authoritative operation broadcast, or CRDT-based
leaderless merge — and deliberately declined to choose between them
without implementation experience to ground the choice in. The project
owner has since given clear, specific direction on this question,
resolving the *topology and authority* half of it (not the full
implementation) with enough precision to record as a decision rather
than leave open any longer.

## Decision

Real-time collaboration uses a **server-authoritative, client–server–
client topology** — not direct peer-to-peer, and not leaderless CRDT
merge as the top-level architecture. Every collaborator connects to a
session server; a client's edits are submitted as **requests** (proposed
operations), never unilaterally applied as authoritative state on other
participants' copies. The server is authoritative over:

- **which operations are accepted**
- **operation ordering**
- **conflict resolution**
- **permissions**

This is not a new authority philosophy invented for collaboration — it's
the *same* one [ADR 0007](0007-networking-and-multiplayer-model.md)
already established for gameplay networking (server-authoritative,
clients send intent/requests rather than force state), applied to a
second, related purpose. That reuse is deliberate and worth stating
plainly: a contributor who understands why gameplay networking works
this way already understands why collaboration does too.

**The session server is not assumed to be a centralized, vendor-run
service.** Consistent with this project's broader no-lock-in posture (MIT
license, no reserved right to change the deal later —
[`docs/vision/project-goals.md`](../../vision/project-goals.md)), it must
be self-hostable: a team can run it themselves (a single process behind a
forwarded port) or on dedicated hosting, exactly like a self-hosted
dedicated game server already is under ADR 0007's model. A first-party
hosted option may exist someday, but self-hosting is the baseline this
architecture is designed around, not an afterthought bolted onto a
cloud-first design.

**Live collaboration and version-control-friendliness are two
consequences of one architecture, not two separate systems.** The
real-time operation stream flowing through the session server is
ephemeral and live; at whatever cadence the project is actually
persisted, that same accepted-operation history is what gets serialized
into the diffable, mergeable format already committed to in
[`docs/architecture/state-and-versioning.md`](../../architecture/state-and-versioning.md) —
which is exactly what makes the resulting history look clean in
GitHub, GitLab, and self-hosted or proprietary equivalents, as intended.

## Alternatives considered

**CRDT-based leaderless merge as the top-level architecture** (the other
direction ADR 0012 named). Rejected as the default, for a reason specific
to this project's audience: a leaderless merge model has no natural place
to enforce *permissions* (who may edit what) or to adjudicate conflicts
according to policy a human or team can reason about — both of which
matter more for professional, studio, and team collaboration than for a
casual, fully public, anyone-can-edit document. A server-authoritative
model gives collaboration an explicit place to put both. This doesn't
waste the CRDT research already recorded in
[`docs/research/technology-evaluations.md`](../../research/technology-evaluations.md#local-first-collaborative-state-crdts) —
it demotes CRDT-style merge algorithms from "the top-level architecture"
to "a candidate technique the *server* could use internally to reconcile
near-simultaneous conflicting operations before deciding what to accept,"
which remains a legitimate, open implementation question within the
server-authoritative model this ADR establishes.

**True peer-to-peer, no server at all.** Rejected for the same reason
true P2P is already rejected as the default for gameplay networking in
ADR 0007: no natural authority for conflict resolution or permissions,
and no natural single point that decides "this is the accepted sequence
of operations" — which a clean, version-controllable history requires
*something* to provide.

## Consequences

- A future `canary-state`/live-collaboration implementation depends on a
  session-server component, self-hostable in the same spirit as a
  dedicated multiplayer server already described in
  [`docs/architecture/networking.md`](../../architecture/networking.md).
- Permission modeling (who may accept whose operations; roles; scopes) is
  real, newly-explicit scope this decision introduces — flagged here as
  genuinely undesigned yet, not silently assumed away.
- This ADR resolves the topology/authority question from
  [ADR 0012](0012-project-state-as-a-versionable-graph.md); it does
  **not** resolve the wire protocol, the operation schema, or permission-
  model specifics, which remain future implementation-time design work —
  the same posture as every other "architecture decided, not yet built"
  ADR in this project.
- [ADR 0012](0012-project-state-as-a-versionable-graph.md)'s status line
  is updated to cross-reference this ADR; its own text is left
  otherwise unchanged, since the parts of it this ADR doesn't touch
  (persistent identity, package format, migration rules) remain exactly
  as open as ADR 0012 originally left them.
- [ADR 0014](0014-change-detection-as-shared-primitive.md), written once
  `v0.0.2` shipped `World::query_changed_since`, names this ADR's
  operation-log-is-the-diff commitment as one of the things that
  primitive is meant to serve, for the ECS-resident slice of project
  state this ADR covers — it doesn't change anything decided here, it
  formalizes the mechanism the eventual implementation is expected to
  share with replication and hot-reload rather than reinvent.
