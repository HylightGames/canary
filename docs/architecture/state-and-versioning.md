# Project State, Versioning, and Collaboration

This document formalizes a principle raised during `v0.0.1` and judged
important enough to become part of the architectural core. The
`canary-state` subsystem is not implemented; ECS identity/schema
primitives and a minimal asset loader exist, while rendering and physics
have narrow implemented slices. See
[ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md)
for the founding decision record and
[ADR 0013](../decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md)
for the live-collaboration topology decision that refines it; this
document is the fuller design behind both.
The proposed `.14` data contracts are expanded below and in
[ADR 0026](../decisions/architecture-decision-records/0026-authored-state-and-simulation-snapshot-contract.md);
review them before implementation.

## The problem

Most engines represent a project as a folder of opaque, engine-specific
files: scenes, prefabs, materials, often serialized as binary or
binary-adjacent formats. This has a well-known, expensive consequence:
version control on that project is close to useless. Two people editing
the same scene produces a diff Git can show but not meaningfully merge; a
merge conflict in a binary scene file is not resolvable by reading it,
only by one person's changes winning and the other's being redone by
hand. This is one of the most consistently painful, well-documented
frustrations in team-based game development, and it's a direct
consequence of *not* treating the project as structured data — the same
category of problem [`docs/research/engine-comparisons.md`](../research/engine-comparisons.md)
identifies for multiplayer being bolted on late, and worth avoiding for
the same reason: fixing it requires the data model to have been designed
for it from early on, not retrofitted after thousands of scenes exist.

## The principle

> All important engine state must be explicitly represented, identifiable,
> serializable, and versionable.

This is now recorded as a founding principle in
[`docs/vision/design-philosophy.md`](../vision/design-philosophy.md#state-is-explicit-identifiable-and-versionable),
because it isn't really about any one subsystem — it constrains the ECS,
the asset system, the plugin system, and networking all at once, and is
much cheaper to hold as a constraint from the start than to retrofit.

## Runtime, schema, authored, and content identities

The important distinction is that **runtime identity, schema identity,
authored identity, and content identity answer different questions.**

- **Runtime identity** — `canary_ecs::Entity`'s `(index, generation)` pair
  (see [`core-runtime.md`](core-runtime.md#ecs-architecture)) — is fast,
  cache-friendly, and deliberately *not* stable across process restarts.
  It exists to make one running simulation's bookkeeping cheap, and nothing
  about it should change to accommodate the concerns below; doing so would
  compromise the ECS's actual job to serve a concern (persistence) that
  isn't the ECS's to own.
- **Persistent (authored) identity** — a stable identifier (a UUID or
  similar) assigned when an entity or other authored object is *created*
  in a project, surviving renames, saves, reloads, edits, and merges — is
  what version control, collaboration, and the marketplace need. The
  stable authored asset identity is `LogicalAssetId`; it is distinct from
  the current provisional `AssetId` content identifier. Neither registry
  is implemented yet.

A saved scene maps persistent identities to authored state; loading it
into a running `World` allocates fresh runtime `Entity` handles and
associates them with their persistent identity for the duration of that
session. This mapping — not a redesign of `Entity` itself — is where a
future `canary-state` crate's responsibility begins.

The simulation snapshot boundary is separate from authored project state.
Per ADR 0021, a simulation snapshot covers deterministic ECS data,
resources, RNG streams, simulation clocks, and schema versions; it
excludes presentation handles, audio devices, editor state, and temporary
caches. Both products may use versioned codecs, but they do not serialize
the same undifferentiated `World`.

## Scope and sequence

Collapsing "make the project version-control-friendly" and "build
Google-Docs-style real-time collaborative editing" into one effort is a
mistake — they have very different costs and very different urgency. The
`v0.0.14` milestone establishes the project-state foundation; the first
shared-edit proof follows in `.16`, and editor/ecosystem features build on
that evidence later.

### `v0.0.14`: project-state and snapshot foundation

The work packages in the
[`v0.1.0 plan`](../roadmap/v0.1.0-plan.md#v0014--project-state) turn these
contracts into the first `canary-state` crate, built on the current ECS and
asset primitives. They also define a simulation snapshot separately from
authored project files, as described above.

- Author-facing formats (scenes, project manifests) should prefer
  structured, diffable, mergeable text (e.g. a stable-key-ordered
  format) over opaque binary, specifically so that even *without* any
  new tooling, two people editing different parts of the same file
  produce a Git diff/merge a human can actually read and resolve. This
  was already noted in passing in
  [`docs/ui/editor-design.md`](../ui/editor-design.md#collaboration-tools);
  this document is where that gets a real design home.
- Every authored object that might be referenced from elsewhere (an
  entity a script targets, an asset a material references) gets a
  persistent identity assigned at creation time, stored alongside its
  data — before scene formats or persisted asset references make the
  identity expensive to retrofit.
- A persistent-identity registry mapping stable IDs to runtime `Entity`
  handles for the duration of a session, as described above.
- Change tracking at the *authored* level (not to be confused with the
  ECS's existing `World::query_changed_since` filter, which reports
  component mutation at the *runtime* level, per
  [`core-runtime.md`](core-runtime.md#known-limitations) — these are
  related but distinct mechanisms operating at different layers, and
  should not be assumed to be "the same feature" just because both
  involve detecting what changed. Runtime mutation detection still needs
  durable removal and destruction records for replication.)
- Unknown-schema preservation: an object whose component/data schema a
  given engine build or plugin set doesn't recognize (see
  [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)
  on schema identity) is round-tripped rather than silently dropped,
  exactly as the motivating discussion for this document described —
  this only becomes tractable once schema identity is itself stable and
  language-agnostic, which is why this depends on ADR 0010's resolution.

### Proposed `v0.0.14` data contract

The first `canary-state` crate exposes two intentionally distinct products:

| Product | What it preserves | What it excludes |
|---|---|---|
| Authored project state | Project/document identity; authored entities and assets; stable references; schemas and versions; prefab instances and overrides; unknown schema/field payloads | Runtime `Entity` handles, frame/tick counters, transient resources, devices, caches |
| Simulation snapshot | Declared deterministic entity/component/resource state; subsystem state registered for snapshots; simulation clock/tick; owned RNG streams; schema/version manifest | Authored editor metadata, UI state, render/audio handles, OS/window state, job-pool internals, transient caches |

They may use the same component codecs, but they have separate roots,
identity rules, profiles, and compatibility checks. A generic “serialize the
whole `World`” operation is not the contract. The snapshot API remains
`snapshot` / `restore` / `checksum` / `step(SimulationInput)` per ADR 0021.

#### Authored project files

- Authored entities and assets use stable project IDs. Runtime entity handles
  are allocated afresh on load and resolved through a per-session registry;
  neither names, row order, runtime indices, nor content hashes are identity.
- A project document carries a document/encoding version. Every typed payload
  carries a stable schema ID and schema version. Engine release version,
  plugin ABI version, schema version, and encoding version remain distinct
  version domains (ADR 0022).
- Author-facing files are structured, diffable text with canonical ordering
  by stable ID and schema ID. The exact encoding is intentionally not chosen
  without a short comparative spike against nested component data, unknown
  payload round-trip, stable formatting, and merge behavior. Record that
  comparison and its result in ADR 0012/0026 before committing the first
  on-disk format; it does not require another architecture document.
- Unknown schemas are retained as opaque, version-tagged payloads. Unknown
  fields on a recognized schema must also survive load/edit/save. Missing,
  explicitly null, defaulted, and unknown fields have separate semantics;
  decoding a partial known schema must not erase unknown data.
- Migrations are explicit per-schema transformations from a declared source
  version to a declared target version. The selected path is deterministic,
  validates each result, and fails with a typed error if a required step is
  absent or rejects the data. A migration runs on staged data; a failed load
  or save never replaces the last good project file.
- Saves write a complete canonical replacement to a sibling temporary file,
  flush it, and atomically replace the previous file where the platform
  supports that operation. Errors retain the prior file and name the failed
  operation/path. Durability details that vary by filesystem are reported,
  not silently promised as universal.
- Prefab instances retain a stable prefab reference and stable-keyed
  overrides. Explicit instance overrides win over prefab defaults; baking
  resolves the chain into runtime components without destroying the authored
  instance/override representation. The `.14` slice needs one level of
  prefab inheritance only; nested inheritance and arbitrary graph editing
  are not implied.
- Authored change tracking belongs to `canary-state` and is driven by its
  authored edit/save boundary. ECS mutation ticks remain runtime change
  detection. The `.14` change set is not the accepted-operation history
  introduced by collaboration in `.16`.

#### Simulation snapshots

- Snapshot participation is explicit. Each deterministic subsystem/resource
  supplies a versioned snapshot codec or is declared excluded; the state
  layer does not inspect arbitrary Rust values or guess which resources are
  authoritative.
- Snapshot entities use a canonical snapshot-local identity and a remapping
  table for internal entity references. `Entity(index, generation)` is never
  treated as a persistent cross-process ID. Restore allocates runtime
  entities and rewrites registered entity-reference fields through that
  table; an unregistered opaque entity reference is an error, not a dangling
  handle.
- Snapshot ordering is canonical by snapshot identity, schema ID, and
  versioned field key. `checksum` hashes this canonical simulation payload;
  it does not hash UI/render/audio state or promise compatibility between
  incompatible schema manifests.
- Run/frame identity and wall-clock frame duration are not simulation state.
  Logical simulation tick/time, deterministic resources, subsystem state,
  and owned RNG state participate when declared. `SimulationInput` is supplied
  to `step`; input history is a separate replay/network log, not duplicated
  inside every snapshot.
- A snapshot is valid only for the declared profile/schema manifest. Missing
  required snapshot codecs fail before mutating the destination world; restore
  is staged so a partial failure cannot leave half-restored simulation state.

The first implementation must prove both products independently, including
unknown-data preservation, fresh runtime-ID allocation, prefab override
resolution, authored-change tracking, migration rollback, and deterministic
snapshot/restore/checksum on a declared small simulation profile.

### Collaboration and ecosystem work

- Undo/redo and "time-travel debugging" as consequences of an operation
  log, once one exists for the reasons above — valuable, but a
  consequence of the architecture rather than a reason to build it first.
- A real package format for marketplace/plugin content, extending the
  gap already flagged in
  [`docs/reviews/risk-register.md`](../reviews/risk-register.md) (R-08):
  not just plugin metadata, but a package that can bundle authored
  content (entities, assets, scripts) with declared dependencies and,
  notably, **migration rules** for evolving a package's schema across
  versions without breaking projects that already depend on an older one
  — the same problem database schema migrations solve, applied to game
  content. This is not required by `.14` or by the first `.16` shared-edit
  proof; the project-state and schema foundations should make it possible
  later.
- The first, deliberately narrow live-collaboration slice is planned for
  [`v0.0.16`](../roadmap/v0.1.0-plan.md#v0016--live-collaboration), after
  authored project state (`v0.0.14`) and gameplay networking (`v0.0.15`).
  That milestone proves a shared authored edit through an authoritative
  session; it does not include the editor UI or a complete collaboration
  product. This is a planned implementation of the architecture, not a
  change to its authority model. The topology and authority model are
  resolved
  ([ADR 0013](../decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md)):
  **server-authoritative, client–server–client** — not peer-to-peer, and
  not CRDT-based leaderless merge as the top-level architecture. This
  directly reuses the authority model Canary already committed to for
  gameplay networking
  ([ADR 0007](../decisions/architecture-decision-records/0007-networking-and-multiplayer-model.md)):
  clients submit edits as requests, never force state; a session server
  is authoritative over which operations are accepted, their ordering,
  conflict resolution, and permissions. The session server is designed to
  be self-hostable from the start — a team runs it themselves or on
  dedicated hosting — not a mandated centralized service, consistent with
  this project's broader no-lock-in posture.

  Real, mature, pure-Rust CRDT prior art was evaluated as a candidate for
  the top-level architecture and was **not** chosen for that role — see
  [`docs/research/technology-evaluations.md`](../research/technology-evaluations.md#local-first-collaborative-state-crdts)
  for Automerge (Ink & Switch, MIT-licensed, "local-first" software with
  Git-like change history) and Loro (newer, Rust-native, faster). A
  leaderless merge model has no natural place to enforce permissions or
  adjudicate conflicts by policy, which matters more for team/studio
  collaboration than for casual, fully public editing — see
  [ADR 0013](../decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md)
  for the full reasoning. CRDT-style merge algorithms remain a legitimate,
  open candidate for how the *server* reconciles near-simultaneous
  conflicting operations internally — demoted from top-level architecture
  to implementation technique, not discarded.

  The broader collaboration product remains later work. Proposed protocol,
  operation, history, and permission contracts are now recorded in
  [`networking.md`](networking.md),
  [`live-collaboration.md`](live-collaboration.md), and ADRs 0027–0028.
  Review them before implementation; ADR 0013 still decides topology and
  authority only.

## Why this belongs in the architecture set before the subsystem is built

The subsystem documents for rendering, physics, and networking exist for
the same reason: design the hard, cross-cutting
parts once, deliberately, before code accumulates on top of an
unexamined assumption. `canary-state` is arguably higher-leverage than
any single one of those three, because — as the discussion that produced
this document put it — if this is designed correctly, Git-friendliness,
multiplayer editing, marketplace packages, and modding stop being
separate features to build and become natural consequences of one
architecture instead. That's exactly the kind of leverage worth writing
down early, and exactly the kind of subsystem worth *not* rushing into
code before its hardest questions (identity and schema/migration behavior,
plus the operation/permission proposal in ADR 0028 extending ADR 0013) have
been reviewed against `.14`/`.15` implementation evidence.

## Status in this foundation

The identity, authoring, and snapshot contracts are architectural; no
`canary-state` crate exists. The `.14` proposal is ready for review in this
document and ADR 0026. Logical identity allocation, authored codecs,
migration, prefab baking, and simulation snapshot APIs remain planned work;
networking and the first shared-edit slice follow in `.15` and `.16`. See the
[`v0.1.0 plan`](../roadmap/v0.1.0-plan.md) for their work packages and
exit evidence, and [`future-roadmap.md`](../roadmap/future-roadmap.md) for
work after the first collaboration proof.
