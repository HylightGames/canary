# 0026. Keep authored project state and simulation snapshots separate

**Status:** Accepted for `v0.0.14` (2026-09-28); implemented in `canary-state`.
Supplements ADRs 0012, 0021, and 0022.

## Context

Canary needs persistent project data that people can inspect, diff, and
migrate, and a separate snapshot of the state required to reproduce a
simulation step. Runtime `Entity` handles, asset content hashes, plugin/API
versions, and ECS mutation ticks answer different questions and cannot stand
in for persistent identity, schema identity, authored history, or simulation
state. ADR 0012 establishes the principle but leaves identity, file format,
and migration details proposed. ADR 0021 defines the simulation snapshot
boundary; ADR 0022 requires distinct version domains and explicit snapshot
participation.

## Proposed decision

1. **Ship two state products with separate roots and profiles.** Authored
   project documents preserve project objects, stable references, prefab
   instances/overrides, schema versions, and unknown data. Simulation
   snapshots preserve only declared deterministic runtime state. They may
   share codecs but are never one generic `World` save/load operation.
2. **Keep identity and version domains distinct.** Authored entities use
   stable project identity and assets use `LogicalAssetId`; loading creates
   fresh runtime `Entity` handles and a per-session registry. Typed payloads
   carry `SchemaId + SchemaVersion + EncodingVersion`. Engine release,
   plugin ABI, project document, component schema, snapshot profile, and
   encoding versions are independently validated.
3. **Use canonical, human-diffable authored data.** Authored project files
   are structured text with deterministic ordering by stable object/schema
   IDs. Do not store runtime handles as authored references or depend on ECS
   query/archetype order. The concrete encoding is not selected by this ADR:
   compare candidate formats against nested component data, canonical output,
   unknown-data preservation, human diff/merge, and stable migration support;
   record the result by updating ADR 0012/0026 before writing the first
   stable file boundary.
4. **Preserve what the current build cannot understand.** Unknown schemas
   round-trip as opaque payloads tagged with schema and encoding versions.
   Unknown fields in a recognized schema also survive load/edit/save.
   Missing, explicit null, defaulted, and unknown fields have distinct
   meanings. If preservation is impossible for a specific payload, the
   operation fails explicitly instead of dropping it.
5. **Make migrations explicit and staged.** Migrations are deterministic,
   versioned transformations registered per schema. Select a declared path
   from source to target version and validate every result. Missing or failed
   required steps return typed errors. Load/migrate/save operate on staged
   data; failure never replaces the last known-good project file. Save writes
   a canonical sibling temporary file, flushes it, and atomically replaces
   the prior document where the platform supports atomic replacement.
6. **Track prefab authoring independently from its baked runtime result.** A
   prefab instance stores a stable prefab reference and stable-keyed
   overrides. Explicit instance overrides take precedence over prefab
   defaults. Baking resolves one inheritance level into runtime components
   without destroying the authored instance/override data. Deeper inheritance
   is outside the first `.14` proof.
7. **Snapshot only declared deterministic participants.** A snapshot
   contains canonical simulation entities/components, explicitly registered
   deterministic resources/subsystem state, simulation tick/time, owned RNG
   state, and its schema/profile manifest. Run ID, outer frame index, wall
   time, presentation resources, editor state, OS handles, and transient
   caches are excluded. `SimulationInput` is supplied to `step`; input history
   is separate from the snapshot.
8. **Remap entity references during snapshot restore.** Snapshot entities
   receive canonical snapshot-local identities and an entity-remapping table.
   Restore allocates runtime handles and rewrites registered entity-reference
   fields. Raw `(index, generation)` values are not treated as persistent or
   cross-process identities. Unknown opaque data may round-trip in authored
   documents, but an uninterpretable reference required by a simulation
   snapshot makes that snapshot unsupported rather than silently dangling.
9. **Keep authored changes separate from runtime and collaboration history.**
   Authored change tracking records changes at the `canary-state` edit/save
   boundary, not by reusing `World::query_changed_since`. Accepted
   collaboration operations and their server order are a separate `.16`
   history product.

## Alternatives considered

**Serialize the entire `World` with one format.** Rejected. A `World` can
contain presentation handles, caches, and implementation-specific resources
that are neither project-authored data nor deterministic simulation state.
It also conflates authored identity with ephemeral runtime handles.

**Use runtime entities or content hashes as persistent IDs.** Rejected.
Runtime entities change across loads, while content hashes change when asset
contents change. Neither expresses the authored identity required for stable
references and collaboration.

**Silently drop unknown data or migrate it to defaults.** Rejected. A build
without a plugin or newer schema would destroy user-authored data on a
load/save cycle. Unknown data must be preserved or the write refused.

**Let schema migrations mutate the source file in place.** Rejected. A failed
migration or interrupted write must leave recoverable last-known-good data.

**Choose an on-disk encoding from convenience alone.** Deferred. JSON, TOML,
RON, or another candidate must be evaluated with the actual project schema,
unknown-value round trip, canonical ordering, and merge behavior. The format
choice is the one explicit decision gate before stable file implementation;
the required comparison belongs in the existing architecture/ADR records.

## Consequences

- `.14` implements identity registries, project codecs/migrations, prefab
  override resolution, authored change tracking, and simulation snapshots as
  separate contracts.
- `canary-state` owns both products but cannot infer serialization support
  for arbitrary ECS resources; deterministic participants register explicit
  codecs or remain outside the snapshot profile.
- Persistence and snapshot tests must prove canonical ordering, unknown-data
  retention, fresh runtime identity allocation, staged failure behavior, and
  deterministic restore/checksum.
- Stable wire compatibility remains a networking concern. `.15` may reuse
  codecs where semantics match but defines its own replication envelope and
  authority rules.
- This does not define arbitrary nested prefab graphs, undo/redo history,
  package distribution, or a collaboration protocol.

## Encoding selection (recorded 2026-09-28, closes the gate in item 3)

Authored/project files use canonical pretty JSON with deterministic
`BTreeMap` key ordering. Simulation snapshot and checksum payloads use
postcard 1.x. Both products share one serde codec layer carrying
`SchemaId + SchemaVersion + EncodingVersion` envelopes; unknown fields
and schemas are preserved as version-tagged `serde_json::Value`
payloads. Canonical checksums hash the canonical serialized snapshot
bytes, never an in-memory representation. Engine code encodes through
the `canary-state` codec traits; `serde_json`/postcard assumptions do
not spread beyond that crate. The format is the `v0.1.0` decision but
the versioned envelopes leave room for a future format/version
migration without rewriting project state or asset systems.

Comparison behind the selection: JSON is the reference implementation
for `#[serde(flatten)]` unknown-field catch-alls, fully self-describing
(`deserialize_any` works, so opaque payloads round-trip), and
`BTreeMap` + pretty printing is byte-deterministic. TOML was rejected:
table-arrays are merge-hostile for nested component lists, there is no
null (missing vs null must be hand-encoded), and `Datetime` admits
multiple representations. RON was rejected despite Bevy's precedent
(`.scn.ron` chose it for terse Rust-like enums): its `#[serde(flatten)]`
support is best-effort with a long restriction list (string-keys-only
in flattened maps, no `RawValue` in flattened contexts), `ron::Value`
improvement is blocked upstream, and it is not self-describing — and
unknown-field preservation is a hard `.14` requirement, exactly RON's
weak spot. Bevy is also the cautionary tale on identity: it keys scene
components by runtime type paths, which this contract already rejects
in favor of `SchemaId`. Godot/Unity's text-author/binary-runtime split
matches this contract's two-product shape. postcard beat bincode on its
stable documented wire format and `no_std`-clean design (relevant to
the `wasm32-wasip2` Tier A story); canonical-ness on both sides comes
from the same single rule (ordered maps in, deterministic bytes out).

## Revisit conditions

- If the format comparison shows no candidate can preserve unknown payloads
  and stable diffs without a bespoke syntax, record a new decision before
  implementing a custom format.
- If simulation snapshot requirements cannot remap entity references without
  stable per-game snapshot identities, amend this contract before exposing
  snapshot bytes to networking.
- If the first real project requires multiple prefab inheritance levels,
  define cycle detection and override conflict semantics before extending the
  one-level `.14` model.
