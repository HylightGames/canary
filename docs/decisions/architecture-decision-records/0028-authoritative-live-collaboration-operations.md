# 0028. Authoritative live-collaboration operations

**Status:** Proposed for `v0.0.16`; review after `.14` project-state formats
and `.15` networking are implemented, before accepting collaborative edits.

## Context

[ADR 0013](0013-live-collaboration-server-authoritative-topology.md) chooses
a server-authoritative collaboration topology, but intentionally leaves the
operation protocol, authorization, conflicts, durable history, and recovery
open. Live collaboration must operate on authored project state, not gameplay
replication or a live ECS `World`. It depends on the stable identities,
versioned codecs, and migration boundary delivered by `.14`, and on the
authenticated server/session transport delivered by `.15`.

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
- Project-state implementation in `.14` must provide stable authored IDs,
  canonical codecs, durable revisions, and the transform/prefab rules needed
  to validate the initial operation.
- The `.15` gameplay networking protocol and `.16` project-edit protocol may
  share transport and authenticated sessions, but not sequence numbers,
  state models, or replication semantics.
- The first proof can remain independent of an editor UI. Editor panels,
  presence, locks, cursors, and workspace UX are consumers of this protocol,
  not prerequisites to validate it.
- The `.16` operation/history API remains Proposed until `.14` and `.15`
  evidence confirms the payload and persistence boundaries. Amend this ADR
  if that evidence changes the contract.
