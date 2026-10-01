# Live Collaboration: Authored Operations and Session History

This document specifies the proposed `.16` shared-authored-state slice. It
builds on project IDs/codecs from `.14`, QUIC/server authority from `.15`, and
the topology in [ADR 0013](../decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md).
Per-entity stable IDs and revision counters are defined in WP1, not reused
from `.14`.
The first operation and permission policy is recorded in proposed
[ADR 0028](../decisions/architecture-decision-records/0028-authoritative-live-collaboration-operations.md).
Review both before implementing the protocol. This is not a design for
replicating a live gameplay `World` or for shipping the editor.

## Scope and authority

The session server is authoritative for the accepted project state, operation
order, conflicts, and permissions. Clients submit edit requests; they never
write authoritative state directly. The server remains self-hostable. The
first proof has two separate clients and one session process, with no editor
UI dependency. The session admits a single fencing epoch for accepted
operations, and accepted operation sequences are monotonic across server
restarts via the durable store — the server reloads the last assigned
sequence before accepting new ops and never reuses one after restart.
(Evidence: VS Live Share host authority for single-sequencer fencing; Fluid
total-order broadcast for durable monotonic order; Unreal MUE epoch-style
writer fencing.)

Live edits apply to authored project objects through the `.14` stable-ID and
schema/migration boundary. They do not apply gameplay replication deltas to
the authoring model. `Entity`, ECS `Tick`, `NetEntityId`, `ProjectRevision`,
`ObjectRevision`, and `OperationSequence` are distinct domains.

## Proposed request and result contract

An edit request contains:

- a client-generated operation ID scoped to the server-authenticated actor;
- the stable authored target ID and the requested operation kind;
- the target object's expected revision (optimistic concurrency token);
- schema ID/version and the operation payload using `.14` codecs.

The authenticated session supplies actor identity; the server ignores any
client-supplied actor name or permission claim. A request is validated in
this order: bounded decode, protocol/schema compatibility, actor permission,
target existence and expected revision, payload validation, and project
migration/codec rules. Validation completes before project state is mutated.

An accepted result includes the server-assigned operation sequence, new
project and target revisions, and the canonical accepted operation. A
rejection includes a stable reason code and the current target revision; for
a revision conflict, it also includes the canonical current target so the
client can refresh. Error text is diagnostic and not a wire contract.

Idempotency is keyed by `(authenticated actor ID, client operation ID)`.
Repeating an identical request returns its previous result without applying
it twice. Reusing that key with a different payload is rejected. The
idempotency lookup precedes the permission check on purpose: a replay is
the same edit returning, not a new edit, so an identical replay still
returns its prior accept when the actor's role changed after the accept
(replay-after-demotion is a replay, not a fresh submission). Sequence
numbers are monotonically assigned by the server within the project history;
they are not client timestamps or ECS ticks. Idempotency records live
exactly as long as the retained-history window: once an ID is evicted with
its history tail, a replay is treated as unknown and follows the
resync-or-reject path, never a silent reapply.
(Evidence: Figma server-ordering/journal — bounded journal with resync base
after eviction.)

## Conflict policy and initial edit surface

Use optimistic compare-and-set on the target object's revision. An operation
is accepted only if the submitted expected revision matches the current
revision. An edit to another target can still be accepted after unrelated
project operations; two edits to the same stale target do not silently
overwrite one another. The later request receives a conflict result and may
be resubmitted against the returned state. Per-property last-write-wins
within one accepted payload is deferred, not rejected: a possible future
refinement inside the target revision check, not today's rule and not ruled
out.
(Evidence: Godot synchronizer per-property replication as a future option,
not today's choice.)

The initial `.16` operation is a complete local-transform replacement on an
existing authored entity. It targets a `LogicalEntityId` (target-ID type to
be defined in WP1), carries one validated `Transform` payload, and is
disabled for an entity whose authored prefab instance disallows a local
transform override (decided: the veto is the `canary-state`
`transform_override_allowed` gate, invoked by `canary-collab`; see
ADR 0028 WP1 selection 3). This exercises the
stable-ID, schema-codec, permission, and conflict boundaries without
inventing a generic arbitrary-component patch language. The `Transform`
validation owner is `canary-collab`: stage 6 numeric validation
(finite floats, normalized quaternion; zero/negative scale is content)
lives in `canary-collab` with numeric semantics from
`canary-transform`, followed by the canonical state `SnapshotValue`
gate in the accept path (see ADR 0028 WP1 selection 6). The `.14` format
review may refine the field shape before `.16` implementation, but it must
not broaden the first slice to arbitrary code execution or runtime entity
handles.

## Permissions

The first policy has three server-owned roles: project owner, editor, and
reader. Readers may observe the session but cannot submit edits; editors may
submit the `.16` operation for authorized project targets. Role
assignment itself is provisioning-time-only in the first slice, not an
in-protocol operation: owners do not grant/revoke roles over the wire.
Provisioning rejects two credentials claiming the same actor with a typed
`DuplicateActor` host error instead of last-credential-wins — the first
binding stands — and a rejected provisioning leaves the persisted
permission file untouched
(`engine/canary-collab/src/permissions.rs`).
The initial permission check is server-side and
uses the authenticated session principal plus operation kind/schema and
target. Fine-grained per-entity ACLs, plugin-defined permissions, invitations,
hosted accounts, and payment/organization roles are deferred.

The two-client proof may provision local test identities or explicit
development credentials, but the protocol must carry a server-authenticated
principal. Do not treat a user ID supplied inside an operation as proof of
identity.

### Credential requirements and rotation runbook (`.16`)

Each pre-shared credential MUST be at least 128 bits of entropy from a
cryptographic random source (e.g. 16 random bytes, hex- or
base64-encoded), unique per credential. The submit/sync paths
distinguish `Unauthenticated` (unknown credential) from `Forbidden`
(reader submitting an edit) with no in-protocol rate limit or lockout
in `.16`: low-entropy or reused credentials are guessable across the
distinguishable failure, so entropy is the only brute-force defense at
this layer. Hosts SHOULD front the session with connection-level rate
limiting; backoff/lockout is explicitly out of scope for `.16`.

There is no in-protocol grant/revoke in `.16`: rotation and revocation
are operator file-and-restart steps, never wire operations.

- Rotation (credential swap, same actor and role): add the new
  credential string mapped to the same actor, remove the old string,
  and restart (or reopen) the session. A new string for a known actor
  provisions normally; only two *different strings colliding on one
  actor in the same provisioning map* fails as `DuplicateActor`.
- Revocation (remove access): delete the credential string from the
  provisioning config and restart. The actor entry persists in the
  permission file but no credential resolves to it, so it cannot
  authenticate.
- Demotion/promotion (role change for a known actor): edit the role in
  the persisted permission file directly, then restart. The file wins
  for known actors over reprovisioned config
  (`engine/canary-collab/src/permissions.rs:109-110`
  — "Config never demotes or removes a persisted role: the file wins
  for known actors"), so changing the config map alone does not move
  an already-persisted role.

## History, persistence, and recovery

Accepted operations are append-only history records with actor, client
operation ID, target, expected revision, accepted sequence, resulting
revision, schema version, and canonical payload. Rejected requests are
observable results but do not change project state or accepted history.
Operation history is separate from ECS change-detection ticks and from
networking's gameplay replication sequence.

The server persists the updated authored state and accepted operation record
as one logical transaction before acknowledging/broadcasting acceptance. A
storage failure rejects the operation and leaves the previous good project
revision recoverable. Saving a project writes the canonical authored base
plus its accepted operation history for the retained version line, as
required by ADR 0013. Project revisions and accepted operation sequences are
stable version-lineage data. Decided (ADR 0028 WP1 selections 2 and
4): the accepted-operation history is an additive in-document `history`
section that supersedes the legacy human log as the ordering truth —
it neither extends `AuthoredDocument.changes` nor keeps a sidecar,
genesis ignores `changes.len()`, and accept never appends to the
human log (no mirroring, no dual-write).

On reconnect, a client presents its last accepted project revision and
operation sequence. The server returns the missing accepted tail if it still
has a complete retained range; otherwise it sends a canonical current
project snapshot and the applicable history checkpoint. A stale request is
never applied to a different baseline without revalidation. Each history
trim carries a checkpoint envelope — project revision, last retained op
sequence, history checkpoint marker — so a late joiner or evicted client has
a defined resync base and never receives a partial tail.
(Evidence: Figma journal checkpoint plus Fluid snapshot-over-ordered-history
base.)

The `.16` slice records history but does not ship user-facing undo/redo. A
future undo is a new compensating operation subject to the same permission
and revision checks, not deletion/reordering of accepted history. History
compaction/retention across many sessions is a later storage design; the
first proof is bounded to a small project and an explicit session history
limit.

The behind-horizon snapshot path carries a 1 MB ceiling as the
`.16`-and-`.1.0` contract (`MAX_SNAPSHOT_BYTES` in
`engine/canary-collab/src/wire.rs`): a canonical snapshot past the ceiling
fails typed (`CollabError::TooLarge`), never truncated, while a snapshot
that cannot be serialized fails as the host-side
`CollabError::SnapshotEncode` (both on the `Session::sync` path in
`engine/canary-collab/src/session.rs`). Small proof projects sit orders of
magnitude below the ceiling; the revisit trigger is the first real project
whose canonical snapshot exceeds 512 KiB, which reopens the ceiling toward
chunked snapshot transfer rather than a silent bump.

Servability is decided on the fully-encoded reply (encode-then-gate), not
on content size alone: the content ceiling above admits the snapshot, but
the framed `SyncResponse` (length prefixes plus the checkpoint envelope)
must itself encode within `MAX_RESPONSE_BYTES`, or the serve fails typed
(`CollabError::TooLarge`), never truncated. Over a tagged frame the same
failure answers as a `TooLarge` reject under the echoed tag
(`engine/canary-collab/src/session.rs::handle_frame_body`), never a
generic `Malformed` — size failures stay typed on every path.

The accept path clones the whole document per operation — O(project bytes),
fine for the small `.16` proof projects. The clone-ceiling revisit trigger
is p99 accept latency on a 1 MB project exceeding 2 ms, measured on real
hardware before any structural-sharing or incremental-candidate work
(`engine/canary-collab/src/session.rs`, stage 9).

### Accepted trust and parsing deferrals (`.16`)

Two boundaries are knowingly deferred, not overlooked:

- Project and permission files are parsed with no depth/size budget
  (`serde_json` unbounded recursion on hostile input). Accepted for `.16`
  because both files are local, operator-owned input read at open and
  provisioning time; revisit with a bounded parse once project files cross
  a trust boundary.
- Sync clients trust the session server: snapshots and history tails are
  accepted without a signature. Accepted for the `.16` single-operator
  model, where server and clients share one operator; revisit with signed
  checkpoints when more than one operator can serve.

## Acceptance evidence

- Two independent clients connect to a session server and read the same
  project baseline.
- An authorized owner/editor changes one entity's local transform; the server
  persists, sequences, and broadcasts the accepted operation; both clients
  converge on the same authored state and project revision.
- A reader's edit is rejected. A nonexistent target and invalid schema
  payload are rejected without mutation.
- Two edits against the same old target revision produce one accepted result
  and one explicit conflict; a retry against the current revision is
  accepted. An edit to a different target is not rejected solely because the
  project revision advanced.
- Repeating an identical operation ID is idempotent; reusing the ID with a
  different payload is rejected.
- Reconnect catches up from retained history or receives a full snapshot
  plus the history checkpoint; it never silently skips a sequence gap.
- Server persistence failure does not broadcast or acknowledge an edit that
  was not durably accepted.
- The protocol remains independent of UI/editor code and uses the transport
  and version rules selected for `.15`.

## Explicit exclusions

No peer-to-peer authority, CRDT top-level merge, offline operation queue,
arbitrary collaborative component editing, collaborative prefab graph
rewrites, editor UI, presence/cursors, locks, hosted identity service,
unbounded operation history, arbitrary history rewriting, or gameplay
`World` replication is implied by this slice.
