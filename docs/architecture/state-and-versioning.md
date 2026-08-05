# Project State, Versioning, and Collaboration

This document formalizes a principle raised during `v0.0.1` and judged
important enough to become part of the architectural core this release
exists to establish, even though — like rendering, physics, and
networking — nothing here is implemented yet. See
[ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md)
for the founding decision record and
[ADR 0013](../decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md)
for the live-collaboration topology decision that refines it; this
document is the fuller design behind both.

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

## Two identities that must not be conflated

The single most important technical distinction this document adds to
what motivated it: **runtime entity identity and persistent/authored
identity are different concerns, and conflating them would be a mistake.**

- **Runtime identity** — `canary_ecs::Entity`'s `(index, generation)` pair
  (see [`core-runtime.md`](core-runtime.md#ecs-architecture)) — is fast,
  cache-friendly, and deliberately *not* stable across process restarts.
  It exists to make one running simulation's bookkeeping cheap, and nothing
  about it should change to accommodate the concerns below; doing so would
  compromise the ECS's actual job to serve a concern (persistence) that
  isn't the ECS's to own.
- **Persistent (authored) identity** — a stable identifier (a UUID or
  similar) assigned when an entity, asset, or other authored object is
  *created* in a project, surviving renames, saves, reloads, edits, and
  merges — is what version control, collaboration, and the marketplace all
  actually need, and does not exist anywhere in Canary today.

A saved scene maps persistent identities to authored state; loading it
into a running `World` allocates fresh runtime `Entity` handles and
associates them with their persistent identity for the duration of that
session. This mapping — not a redesign of `Entity` itself — is where a
future `canary-state` crate's responsibility begins.

## Layered scope: near-term, medium-term, long-term

Collapsing "make the project version-control-friendly" and "build
Google-Docs-style real-time collaborative editing" into one effort is a
mistake — they have very different costs and very different urgency.
This document deliberately separates them:

### Near-term (informs `v0.0.2`+ data-format decisions, doesn't require a new crate yet)

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
  data — cheap to require now, before any scene format or asset
  pipeline exists to retrofit it into later.

### Medium-term (a real `canary-state` crate; depends on the archetype ECS migration and the asset pipeline both existing)

- A persistent-identity registry mapping stable IDs to runtime `Entity`
  handles for the duration of a session, as described above.
- Change tracking at the *authored* level (not to be confused with the
  ECS's own future change-detection query filters, which serve
  networking replication and hot reload at the *runtime* level, per
  [`core-runtime.md`](core-runtime.md#known-limitations) — these are
  related but distinct mechanisms operating at different layers, and
  should not be assumed to be "the same feature" just because both
  involve detecting what changed).
- Unknown-schema preservation: an object whose component/data schema a
  given engine build or plugin set doesn't recognize (see
  [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)
  on schema identity) is round-tripped rather than silently dropped,
  exactly as the motivating discussion for this document described —
  this only becomes tractable once schema identity is itself stable and
  language-agnostic, which is why this depends on ADR 0010's resolution.
- A real package format for marketplace/plugin content, extending the
  gap already flagged in
  [`docs/reviews/risk-register.md`](../reviews/risk-register.md) (R-08):
  not just plugin metadata, but a package that can bundle authored
  content (entities, assets, scripts) with declared dependencies and,
  notably, **migration rules** for evolving a package's schema across
  versions without breaking projects that already depend on an older one
  — the same problem database schema migrations solve, applied to game
  content.

### Long-term (genuinely later; depends on the medium-term layer existing and proving itself first)

- Undo/redo and "time-travel debugging" as consequences of an operation
  log, once one exists for the reasons above — valuable, but a
  consequence of the architecture rather than a reason to build it first.
- Real-time collaborative editing ("live share"). Explicitly **not**
  scoped to any near or medium-term milestone — this is about the
  *architecture*, decided ahead of the implementation, the same posture
  taken toward rendering, physics, and networking throughout this
  project. The topology and authority model are resolved
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

  What remains genuinely open: the wire protocol, the operation schema,
  and permission-model specifics — real design work for whenever
  `canary-state` implementation actually starts, not decided here.

## Why this belongs in `v0.0.1`'s documentation even though nothing here is built

Every other major subsystem doc in this project (rendering, physics,
networking) exists for the same reason: design the hard, cross-cutting
parts once, deliberately, before code accumulates on top of an
unexamined assumption. `canary-state` is arguably higher-leverage than
any single one of those three, because — as the discussion that produced
this document put it — if this is designed correctly, Git-friendliness,
multiplayer editing, marketplace packages, and modding stop being
separate features to build and become natural consequences of one
architecture instead. That's exactly the kind of leverage worth writing
down early, and exactly the kind of subsystem worth *not* rushing into
code before its hardest questions (identity, merge semantics, the
CRDT-vs-authoritative choice) have real design attention.

## Status in this foundation

Entirely architectural. No `canary-state` crate exists. Depends on the
archetype ECS migration and the asset pipeline (both `v0.0.2`+) before
medium-term scope becomes buildable — see
[`docs/roadmap/future-roadmap.md`](../roadmap/future-roadmap.md).
