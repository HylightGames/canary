# 0008. Workspace crates version in lockstep, not independently

**Status:** Accepted (formalizing an existing, already-implemented
choice — see "Context" for why this ADR exists after the fact rather
than before)

## Context

The root [`Cargo.toml`](../../../Cargo.toml) uses `version.workspace =
true` for every crate in the workspace (`canary-core`, `canary-platform`,
`canary-ecs`, `canary-plugin-api`, `canary-runtime`, `xtask`), meaning
all of them share one version number, bumped together, following the
scheme in [ADR 0006](0006-versioning-scheme.md).

This was implemented mechanically while scaffolding the initial
workspace, without ever being written down as a deliberate decision or
having its alternative (independent per-crate SemVer) evaluated on the
record — exactly the kind of undocumented, load-bearing decision
[ADR 0001](0001-record-format.md) exists to prevent. This ADR exists to
correct that, identified during `docs/reviews/2026-08-senior-architecture-review.md`
(Finding 9.1). It formalizes the existing choice rather than changing it,
because the choice itself holds up under scrutiny — see below.

## Decision

All `canary-*` workspace crates version in **lockstep**: one version
number, shared across every crate, bumped together on every release,
following the pre-1.0 scheme in [ADR 0006](0006-versioning-scheme.md).

This is directly precedented by Bevy's own many `bevy_*` crates, which
version in lockstep specifically so that "which versions of these crates
work together" is never a question anyone has to answer — they're the
same number, always.

## Alternatives considered

**Independent per-crate SemVer**, where e.g. `canary-ecs` could reach
`0.3.0` while `canary-platform` is still at `0.1.0`. Rejected: with a
plugin/marketplace ecosystem expected to depend on *multiple* `canary-*`
crates simultaneously (directly or transitively), requiring plugin
authors and downstream tooling to reason about a cross-crate
compatibility matrix is a tax this project doesn't need to impose —
especially pre-1.0, when the crates are expected to evolve together
anyway as parts of one coherent engine, not as independently-useful
libraries with separate release cadences.

**A hybrid** — core crates (`canary-core`, `canary-ecs`, ...) in
lockstep, peripheral/tooling crates independent. Rejected for now: adds a
categorization decision with no current benefit at this crate count (six
crates, all part of the same foundational layer). Worth revisiting if a
genuinely peripheral crate emerges later with a legitimate need to
release fixes on its own cadence independent of the engine's own release
train (a specific third-party asset-format importer is the most likely
future candidate) — that would warrant a new ADR amending this one, not
a silent exception.

## Consequences

- `xtask` (a dev-only tool, never published) and `canary-runtime` (a
  binary, `publish = false`) carry the same engine version number despite
  never reaching crates.io — mildly unusual, but harmless, and it keeps
  "everything in this workspace is one version" a simple, exception-free
  mental model for contributors.
- A future crate that genuinely needs independent versioning is an
  exception to be argued for explicitly, in a new ADR, not a Cargo.toml
  edit made without one — the failure mode this ADR exists to close off.
- This decision composes with [ADR 0006](0006-versioning-scheme.md)
  (the pre1/pre2/.../v0.0.1 scheme) rather than replacing any part of it;
  ADR 0006 defines *what the numbers mean*, this ADR defines *how many
  version numbers the workspace has at a time* (one).
