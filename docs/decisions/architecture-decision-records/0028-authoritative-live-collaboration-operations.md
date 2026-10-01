# 0028. Authoritative live-collaboration operations

**Status:** Proposed for `v0.0.16`; review after `.14` project-state formats
and `.15` networking are implemented, before accepting collaborative edits.

## Context

[ADR 0013](0013-live-collaboration-server-authoritative-topology.md) chooses
a server-authoritative collaboration topology, but intentionally leaves the
operation protocol, authorization, conflicts, durable history, and recovery
open. Live collaboration must operate on authored project state, not gameplay
replication or a live ECS `World`. It depends on the `ProjectId`, versioned
codecs, and migration boundary delivered by `.14`, and on the versioned
transport delivered by `.15`. `.15` delivers versioned transport only (no
authenticated identity — see the `v0.0.15` release notes and ADR 0027 pt 9);
`.16` WP1 builds the authenticated principal, and dev-provisioned identities
are acceptable for the first proof. Per-entity `LogicalEntityId`,
revision/sequence counters, and permission storage are WP1 build items, not
`.14` reuse.

The proposed concrete contract is detailed in
[`live-collaboration.md`](../../architecture/live-collaboration.md). This ADR
records the cross-cutting rules that need to remain stable when the first
two-client vertical slice is built.

## Proposed decision

1. **The session server is authoritative.** Clients submit edit requests and
   receive canonical accepted operations or explicit rejection results. A
   client-provided actor name, role, or permission claim is never identity
   proof; authorization uses the principal authenticated by the server
   session.
2. **Keep identity and sequence domains separate.** A request carries a
   client-generated operation ID scoped to its authenticated actor, a stable
   authored target ID, an expected target revision, and a versioned operation
   payload. The server assigns the accepted operation sequence and resulting
   project/target revisions. These are not ECS `Entity` handles, `Tick`
   values, network entity IDs, timestamps, or one another.
3. **Use target-scoped compare-and-set conflicts.** Accept an operation only
   when its expected target revision matches current authored state. An
   unrelated edit to another target need not conflict merely because the
   project revision changed. A same-target stale edit receives the canonical
   current target revision/state so the client can refresh and resubmit.
4. **Make accepted operation IDs idempotent.** The key is
   `(authenticated actor ID, client operation ID)`. Repeating the same
   request returns the prior result without applying it twice; reusing the
   key with a different payload is rejected.
5. **Persist before acknowledging.** The changed canonical authored state
   and accepted operation-history record are one logical durable commit.
   The server must not acknowledge or broadcast an operation that failed to
   persist. Rejected requests do not enter accepted history.
6. **Recover through ordered history or a checkpointed snapshot.** A
   reconnecting client supplies its last accepted revision and sequence. The
   server returns a complete retained tail or a canonical current snapshot
   and applicable history checkpoint. A missing sequence is never silently
   skipped.
7. **Start with one narrow operation and three server-owned roles.** The
   first vertical slice replaces the local `Transform` of an existing
   authored entity, subject to `.14` schema and prefab-override rules. Owner
   and editor may make this edit; reader is read-only. Fine-grained ACLs,
   generic component patches, arbitrary plugin-defined edits, and user-facing
   undo/redo are not part of the first slice. Future undo appends a
   compensating operation under the current revision/permission rules.
8. **Bound accepted history.** The first service defines and enforces
   explicit operation, payload, queue, and retained-history limits. Long-term
   compaction and archival policy need usage evidence and are not silently
   implied by the initial format.

## Alternatives considered

**Last-write-wins by client timestamp.** Rejected. Client clocks are not a
trustworthy ordering or conflict policy, and silent overwrites lose authored
work.

**A single global project-revision compare-and-set for every edit.** Rejected
for the first operation. It would reject independent edits to unrelated
objects and create avoidable contention. Target revisions retain a simple
conflict rule while allowing independent changes.

**Peer-to-peer or CRDT-first editing.** Deferred. ADR 0013 already selects
server authority, and the first release needs an explicit durability and
recovery proof. Offline merge and CRDT semantics are a separate decision
requiring measured multi-writer needs.

**Arbitrary component mutation or a generic patch language.** Deferred. It
would broaden authorization, schema compatibility, prefab semantics, and
conflict scope before one authored edit has been proven end to end.

**Rewrite or delete history to implement undo.** Rejected. Accepted history
is a durable audit/recovery sequence. Undo should be a later compensating
operation that is validated and ordered like any other edit.

## Consequences

- `.16` must implement server-side authenticated identity, explicit
  authorization and rejection results, idempotency, target revisions,
  durable state/history commit, reconnect recovery, and resource bounds.
- Project-state implementation in `.14` provides `ProjectId` and canonical
  codecs. Per-entity `LogicalEntityId`, revision/sequence counters, and
  permission storage are `.16` WP1 build items, not `.14` reuse. WP1 uses the
  `.14` schema and prefab rules to validate the initial operation.
- The `.15` gameplay networking protocol and `.16` project-edit protocol may
  share transport and authenticated sessions, but not sequence numbers,
  state models, or replication semantics.
- The first proof can remain independent of an editor UI. Editor panels,
  presence, locks, cursors, and workspace UX are consumers of this protocol,
  not prerequisites to validate it.
- The `.16` operation/history API remains Proposed until `.14` and `.15`
  evidence confirms the payload and persistence boundaries. Amend this ADR
  if that evidence changes the contract.
- Open WP1 definition items: the target-ID type (or `entity.<local>` plus a
  rename policy), `ProjectRevision`/`ObjectRevision`/`OperationSequence` and
  their relation to the existing `AuthoredDocument.changes` human log,
  the permission store plus its provisioning path, and `Transform`
  validation plus the prefab-veto location.

## Research addenda (Proposed — contract unchanged, WP1 must define)

A1. **Idempotency records live exactly as long as the retained-history
window.** Once an op ID is evicted with its history tail, a replay of that
ID is not silently reapplied; the server treats it as unknown and follows
the resync-or-reject path (conflict result or snapshot-plus-checkpoint per
rule 6), never a silent re-execution.
(Evidence: Figma server-ordering/journal — bounded journal with resync base
after eviction.)

A2. **Single-writer fencing with restart-surviving sequences.** The session
admits one fencing epoch for accepted operations, and accepted operation
sequences are monotonic across server restarts via the durable store: the
server reloads the last assigned sequence before accepting new ops and never
reuses a sequence after restart.
(Evidence: VS Live Share host authority for single-sequencer fencing; Fluid
total-order broadcast for durable monotonic order; Unreal MUE epoch-style
writer fencing.)

A3. **Trim carries a checkpoint envelope.** Each history trim records
(project revision, last retained op sequence, history checkpoint marker) so
a late joiner or evicted client has a defined resync base; a client behind
the checkpoint takes the snapshot-plus-checkpoint path, never a partial
tail.
(Evidence: Figma journal checkpoint plus Fluid snapshot-over-ordered-history
base.)

A4. **Per-property LWW is deferred, not rejected.** Finer-grained
last-write-wins within one accepted operation payload remains a possible
future conflict refinement inside the target revision check — not today's
target-scoped compare-and-set, and not ruled out by the rejected
client-timestamp LWW alternative above.
(Evidence: Godot synchronizer per-property replication as a future option,
not today's choice.)

## WP1 selections (amendment, `.16` build — closes the open definition items)

WP1 resolves the four open items listed under Consequences above in
six selections as follows (validation ownership and the veto location
are recorded separately, as are history placement and the no-dual-write
rule). The contract above is unchanged; these are the selections the
`.16` implementation builds:

1. **Target ID: `LogicalEntityId` is the validated `entity.<local>`
   suffix; no renames in `.16`.** The local section name after `entity.`
   is the identity — validated at construction (non-empty, no `.`/`/`
   whitespace, `prefab` reserved), never built from an unchecked string
   (no `From<String>`), and validated again on deserialization. A rename
   is a delete-plus-create, never a silent retarget. Implemented in
   `canary-state` (`revisions.rs`), keeping the leaf free of
   collaboration concepts.
2. **History is an additive in-document section, not a sidecar.** The
   `history` section (`DocumentHistory`: project revision, next sequence,
   per-target revisions, bounded retained tail, checkpoint, evicted
   count) rides the same canonical JSON and the same atomic save as the
   authored state it versions, so one durable commit covers both. It is
   `#[serde(default)]` and skipped while empty: files that never saw
   collaboration are byte-identical to pre-history files, legacy files
   load with genesis counters, and the envelope stays `canary.project`
   version 1, encoding 1. Genesis is project 0 / all-targets 0 /
   next-sequence 1 regardless of `changes.len()` — the legacy human log
   neither seeds nor mirrors the operation counters.
3. **The veto check is a `canary-state` gate invoked by `canary-collab`.**
   `AuthoredDocument::transform_override_allowed` reuses the one-level
   prefab resolve/bake rules: prefab-free entities are editable; a prefab
   instance is vetoed when its prefab reference is missing, malformed,
   unknown, chained, nested, or supplies the transform the instance never
   declared its own override for. The fallback stands as specified: had
   the veto proven unshippable, the slice would have shipped
   allow-all-plus-log with a risk-register entry — it did not come to
   that, and no such entry was needed.
4. **No dual-write: the machine history is the ordering truth; the
   human `changes` log is untouched by accept.** The accept path
   assigns the sequence/revision into the candidate via
   `history.accept`, then persists state plus history in one atomic
   write, and never calls `record_change`
   (`engine/canary-collab/src/session.rs:437-467`). Genesis is
   project 0 / all-targets 0 / next-sequence 1 regardless of
   `changes.len()`: the human log neither seeds nor mirrors the
   operation counters, and accepted operations do not append to it
   (`engine/canary-state/src/revisions.rs:619-629`,
   `genesis_ignores_the_human_change_log`). This resolves the
   reviewer-raised "missing dual-write" as an explicit selection, not
   an omission: dual-write is rejected, and the governing wording is
   this ADR (lines 174-176 above) plus
   `docs/architecture/state-and-versioning.md:183-184`.
5. **Permission-store shape: a separate server-side atomic-JSON file,
   epoch 1 at provisioning, file-wins, provisioning-time-only
   roles.** `PermissionStore` (roles plus fencing epoch) lives in its
   own JSON file written with atomic-replace semantics (sibling temp
   file, flush, rename), not in the project document; a missing file
   provisions at epoch 1, a restart bumps the epoch, and the file wins
   for known actors (config never demotes or removes a persisted
   role). Roles are provisioned out-of-protocol from pre-shared
   credentials; there is no in-protocol grant/revoke in `.16`
   (`engine/canary-collab/src/permissions.rs:46-128`,
   `engine/canary-collab/src/lib.rs:30-34`).
6. **Numeric-validation ownership: `canary-collab` owns it, with
   numeric semantics from `canary-transform` plus the canonical
   `SnapshotValue` gate.** Stage 6 runs `TransformPayload::validate`
   (finite floats, normalized quaternion within `QUAT_NORM_EPSILON`;
   zero/negative scale is content) in `canary-collab`
   (`engine/canary-collab/src/op.rs:45-65`); stage 8 then runs the
   payload through the canonical snapshot-value codec so the durable
   record is provably canonical, not merely finite
   (`engine/canary-collab/src/session.rs:418-431`).
7. **Snapshot ceiling: 1 MB is the `.16`-and-`.1.0` contract, typed
   not truncated.** The behind-horizon sync snapshot is bounded by
   `MAX_SNAPSHOT_BYTES` (1 MiB, `engine/canary-collab/src/wire.rs:49`):
   a snapshot past the ceiling fails typed
   (`CollabError::TooLarge`), and an unserializable snapshot fails
   host-side (`CollabError::SnapshotEncode`) — both on the
   `Session::sync` path (`engine/canary-collab/src/session.rs:556-566`),
   never a truncated reply. The revisit trigger is the first real
   project whose canonical snapshot exceeds 512 KiB (chunked snapshot
   transfer wins over a silent bump).
