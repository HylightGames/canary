# 0010. Component identity must be stable across the language boundary, not just within the Rust host

**Status:** Accepted (prototyped alongside the `v0.0.2` archetype migration
— see "Resolution" below for what held up and what narrowed; was tracked
as risk R-04 in `docs/reviews/risk-register.md`, now `Mitigated`)

## Context

`canary-ecs` identifies component types using `std::any::TypeId`
(`TypeId::of::<T>()`), both in the `v0.0.1-pre1` placeholder storage and,
implicitly, in the target archetype design described in
[`core-runtime.md`](../../architecture/core-runtime.md#ecs-architecture).
`TypeId` is a Rust-compiler-internal concept. It is not guaranteed stable
even across two separate Rust compilations of what a human would call
"the same type" (different compiler versions, different crate versions),
and it has no meaning whatsoever to code written in a non-Rust language.

This matters because [`plugin-system.md`](../../architecture/plugin-system.md)
and [`scripting-system.md`](../../architecture/scripting-system.md) both
commit — as a foundational, not incidental, part of the "language-agnostic"
vision — to Tier A (WASM Component Model) plugins declaring which
components they read and write, with the host validating that declaration
against granted capabilities. A Tier A plugin written in C, Zig, or any
other Component Model source language participates in the Component
Model's own (WIT-based) type system, which shares no identity space with
Rust's `TypeId` at all. As things stand, there is no mechanism by which
such a plugin could refer to a Canary component type, let alone have that
reference meaningfully checked.

If this is left unresolved until the Tier A loader and the archetype ECS
migration are already underway, the likely outcome is a translation layer
invented under implementation-deadline pressure, retrofitted onto
subsystems that by then already assume `TypeId` is the one true identity
mechanism — exactly the kind of costly-later problem this review process
exists to catch early instead.

## Decision (proposed direction)

Introduce a **stable, language-agnostic schema identity** — a namespaced
string plus a schema version, e.g. `"canary:transform/position@1"` — for
any component type that might ever cross the plugin, replication, or
marketplace boundary. This is deliberately analogous to how WIT
interfaces/worlds are already versioned in the Component Model, and to
how Protocol Buffers/Cap'n Proto assign stable wire identities
independent of any single host language's in-memory representation.

Rust's `TypeId` does not go away — it remains a legitimate, efficient
**host-internal** lookup key, an implementation detail of the archetype
storage's fast path. What changes is that `TypeId` should never be the
thing a plugin, a WIT interface, a replication wire format, or a
marketplace tool observes directly. Concretely, this likely means: a
`#[derive(CanaryComponent)]`-style macro (or an explicit trait impl)
pairing a Rust type with its stable string+version identity; a registry
mapping that identity to the host's `TypeId` at runtime for the fast
path; and Tier A WIT interfaces that reference the stable identity, never
`TypeId`.

## Alternatives considered

**Do nothing; limit Tier A plugins to only using host-defined component
types (no plugin-defined components).** Rejected: undermines
"language-agnostic" and "modding-first" as stated goals — a large part of
what makes modding valuable is plugins introducing *new* game-specific
data, not just operating on data the host already defined.

**Full structural typing** (infer a component's "shape" from its
serialized layout, no nominal identity at all). Rejected for now: more
flexible in the abstract, but a materially weaker foundation for
versioning, tooling, and documentation — a marketplace, an inspector UI,
or a replication diffing tool all benefit from being able to say "this is
schema X, version Y" rather than inferring compatibility structurally.
Nominal identity also matches the precedent this project already accepts
elsewhere (WIT, Protobuf).

## Consequences

- Adds a small amount of ceremony: any component intended to be visible
  to plugins, networking, or the marketplace needs an explicit stable
  identity, not just a Rust type definition. Purely internal,
  host-only components (never replicated, never plugin-visible) can
  reasonably continue to rely on `TypeId` alone.
- This should be prototyped **alongside**, not after, the archetype ECS
  migration ([`core-runtime.md`](../../architecture/core-runtime.md#ecs-architecture))
  — the two designs need to fit together, since the archetype storage
  layout and the schema-identity registry both touch how components are
  looked up.
- `Status: Proposed` rather than `Accepted` is deliberate: this ADR
  establishes the *principle* (component identity must not leak `TypeId`
  across the language boundary) and a *plausible* concrete mechanism, but
  the exact implementation (macro vs. manual trait, registry design)
  should be settled with real prototyping experience during the Era 2
  work this depends on, not decided speculatively from a desk review.
  This ADR should be updated to `Accepted` (or superseded by a more
  specific one) once that prototyping happens — see
  [ADR 0001](0001-record-format.md) for why superseding rather than
  silently editing is the right way to do that when the time comes.

## Resolution (`v0.0.2` archetype migration)

Prototyped alongside the archetype migration, as planned. The core
direction held up: a `CanaryComponent` trait carrying a `SCHEMA_ID`
constant, plus a `World::register_component`/`World::type_id_for_schema`
registry mapping that stable identity to the host's `TypeId` — see
`engine/canary-ecs/src/component_identity.rs` and the `register_component`
family on `World` in `engine/canary-ecs/src/world.rs`.

One open question from "Consequences" is settled, at least for this pass:
**an explicit trait impl, not a `#[derive(CanaryComponent)]` macro.** A
derive remains a fully backward-compatible future addition — it would
only generate the same trait impl written by hand today — but building
proc-macro infrastructure before there's a second real consumer felt
speculative.

What this prototype does **not** cover: any actual Tier A (WASM) consumer
resolving a plugin-declared schema id through this registry to reach host
storage. `World::type_id_for_schema` is the seam that consumer will
eventually use; building it is future work, tracked wherever the Tier A
loader itself gets scoped (see
[`docs/roadmap/v0.0.2-roadmap.md`](../../roadmap/v0.0.2-roadmap.md),
"Explicitly not in v0.0.2" — Tier A loading is out of scope for this
release too).

Status moves to `Accepted` on that basis: the principle and the concrete
registry mechanism both held up under real implementation, not just desk
review.
