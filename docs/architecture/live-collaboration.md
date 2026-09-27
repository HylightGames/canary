# Live Collaboration: Authored Operations and Session History

This document specifies the proposed `.16` shared-authored-state slice. It
builds on project IDs/codecs from `.14`, QUIC/server authority from `.15`, and
the topology in [ADR 0013](../decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md).
The first operation and permission policy is recorded in proposed
[ADR 0028](../decisions/architecture-decision-records/0028-authoritative-live-collaboration-operations.md).
Review both before implementing the protocol. This is not a design for
replicating a live gameplay `World` or for shipping the editor.

## Scope and authority

The session server is authoritative for the accepted project state, operation
order, conflicts, and permissions. Clients submit edit requests; they never
write authoritative state directly. The server remains self-hostable. The
first proof has two separate clients and one session process, with no editor
UI dependency.

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
it twice. Reusing that key with a different payload is rejected. Sequence
numbers are monotonically assigned by the server within the project history;
they are not client timestamps or ECS ticks.

## Conflict policy and initial edit surface

Use optimistic compare-and-set on the target object's revision. An operation
is accepted only if the submitted expected revision matches the current
revision. An edit to another target can still be accepted after unrelated
project operations; two edits to the same stale target do not silently
overwrite one another. The later request receives a conflict result and may
be resubmitted against the returned state.

The initial `.16` operation is a complete local-transform replacement on an
existing authored entity. It targets a `LogicalEntityId`, carries one
validated `Transform` payload, and is disabled for an entity whose authored
prefab instance disallows a local transform override. This exercises the
stable-ID, schema-codec, permission, and conflict boundaries without
inventing a generic arbitrary-component patch language. The `.14` format
review may refine the field shape before `.16` implementation, but it must
not broaden the first slice to arbitrary code execution or runtime entity
handles.

## Permissions

The first policy has three server-owned roles: project owner, editor, and
reader. Readers may observe the session but cannot submit edits; editors may
submit the `.16` operation for authorized project targets; owners may also
grant/revoke these roles. The initial permission check is server-side and
uses the authenticated session principal plus operation kind/schema and
target. Fine-grained per-entity ACLs, plugin-defined permissions, invitations,
hosted accounts, and payment/organization roles are deferred.

The two-client proof may provision local test identities or explicit
development credentials, but the protocol must carry a server-authenticated
principal. Do not treat a user ID supplied inside an operation as proof of
identity.

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
stable version-lineage data.

On reconnect, a client presents its last accepted project revision and
operation sequence. The server returns the missing accepted tail if it still
has a complete retained range; otherwise it sends a canonical current
project snapshot and the applicable history checkpoint. A stale request is
never applied to a different baseline without revalidation.

The `.16` slice records history but does not ship user-facing undo/redo. A
future undo is a new compensating operation subject to the same permission
and revision checks, not deletion/reordering of accepted history. History
compaction/retention across many sessions is a later storage design; the
first proof is bounded to a small project and an explicit session history
limit.

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
  and identity/version rules selected for `.15`.

## Explicit exclusions

No peer-to-peer authority, CRDT top-level merge, offline operation queue,
arbitrary collaborative component editing, collaborative prefab graph
rewrites, editor UI, presence/cursors, locks, hosted identity service,
unbounded operation history, arbitrary history rewriting, or gameplay
`World` replication is implied by this slice.
