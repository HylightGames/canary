# 0006. Versioning scheme: `v0.0.1-preN` → `v0.0.1` → ... → `v1.0.0`

**Status:** Accepted

## Context

A multi-year, pre-1.0 project needs a versioning scheme that (a) is
unambiguous about what's experimental versus stable, (b) doesn't overload
`0.x.y` SemVer conventions in a way that's confusing to newcomers, and (c)
gives contributors and early adopters a clear signal for when a given
milestone is "usable" versus "still in flux."

## Decision

Canary uses the following scheme, specified as a founding project
requirement and recorded here as binding:

- **`v1.0.0`** means mature, stable, production-ready, with a backwards-
  compatibility commitment. Everything before it is, without exception,
  experimental — no pre-1.0 release, however polished it looks, implies a
  compatibility guarantee.
- **Development milestones start at `v0.0.1`**, reached through a sequence
  of pre-releases:
  ```
  v0.0.1-pre1
  v0.0.1-pre2
  v0.0.1-pre3
  ...
  v0.0.1
  ```
  `-preN` releases are for rapid development, architecture experimentation,
  and internal testing — breaking changes between them are expected and not
  considered a compatibility break. The unqualified `v0.0.1` marks that
  milestone's scope as stable enough for public use (though still pre-1.0,
  i.e., still without a long-term compatibility commitment).
- **Subsequent development continues the same pattern**:
  ```
  v0.0.2
  v0.0.3
  ...
  v0.1.0
  ...
  v1.0.0
  ```

This foundation itself is being built as the work that precedes
`v0.0.1-pre1` — see
[`docs/roadmap/v0.0.1-roadmap.md`](../../roadmap/v0.0.1-roadmap.md) for what
that first milestone actually contains.

## Alternatives considered

**Standard SemVer from `v0.1.0` onward, treating all `0.x` as "anything can
break."** Rejected: this is what the scheme above refines, not replaces —
plain `0.x` SemVer doesn't distinguish "actively-churning pre-release" from
"stable enough to build on, just not API-frozen yet," which is a
distinction this project's contributors and early adopters explicitly asked
for.

**Calendar versioning (e.g., `2026.1`).** Rejected: calendar versioning
signals release cadence, not stability, which is the exact opposite of what
this project needs to communicate given its emphasis on "everything before
v1 is experimental."

**Skip pre-releases, go straight to incrementing patch versions
(`v0.0.1`, `v0.0.2`, ...) without a `-preN` phase.** Rejected: this removes
the explicit signal that a given point in history was mid-milestone and
subject to breaking changes, which matters for a project expecting outside
contributors to build against in-progress work (plugin authors, for
instance, need to know whether a given commit is "stable enough to write a
plugin against" or "still actively being redesigned").

## Consequences

- Git tags and `Cargo.toml` versions across the workspace follow this scheme
  exactly; `CHANGELOG.md` is organized by these same version markers.
- No crate in the workspace may claim `v0.0.1` (unqualified) until the scope
  defined in
  [`docs/roadmap/v0.0.1-roadmap.md`](../../roadmap/v0.0.1-roadmap.md) is
  actually met — the version number is a claim about the milestone's
  content, not a counter that increments on a schedule.
- Once `v1.0.0` ships, this ADR's compatibility commitment becomes binding,
  and any breaking change afterward requires a major version bump and its
  own ADR explaining the break.
