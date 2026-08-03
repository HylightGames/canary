# Milestones

A dated record of progress against [`v0.0.1-roadmap.md`](v0.0.1-roadmap.md).
Where [`CHANGELOG.md`](../../CHANGELOG.md) records *what changed* in
end-user/consumer terms, this document records *milestone-level* progress
for project planning purposes — think of it as the project-management view,
and the changelog as the release-notes view.

## `v0.0.1-pre1` — Foundation

**Status:** Complete.

The founding session for the project. Established:

- Repository, git history, and governance (license, contribution
  guidelines, code of conduct, security policy, issue/PR templates, CI).
- The full documentation architecture (vision, architecture, decisions,
  roadmap, development, UI, research), populated with real content and
  cross-references rather than placeholders.
- Seven initial ADRs recording the foundational technical decisions
  (language choice, plugin architecture, rendering strategy, build
  tooling, versioning scheme, networking model, and the ADR process
  itself).
- A compiling, tested Cargo workspace: `canary-core`, `canary-platform`,
  `canary-ecs`, `canary-plugin-api`, `canary-runtime` (a headless boot
  harness), and the `xtask` build-orchestration crate.
- CI validated across a Linux/macOS/Windows build matrix, plus a
  `wasm32-wasip2` target-compiles check for the plugin API crate (see
  [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml)).

**Scope explicitly deferred to later `-preN` releases within the same
`v0.0.1` milestone** (see
[`v0.0.1-roadmap.md`](v0.0.1-roadmap.md#whats-explicitly-deferred-to-v001-pre2-and-beyond-still-within-this-milestone)
for the full list): Tier A (WASM/Wasmtime) plugin loading, a real windowing
backend, archetype-based ECS storage, the parallel job scheduler, and
change-detection query filters.

## `v0.0.1-pre1` — Senior architecture review

**Status:** Complete (documentation/governance only — see scope note
below).

Conducted immediately after the foundation session above, at the
architect's own request, before further `v0.0.1` development continued.
Produced:

- [`docs/reviews/2026-08-senior-architecture-review.md`](../reviews/2026-08-senior-architecture-review.md) —
  a full critique across repository structure, Rust architecture, plugin
  ABI, ECS strategy, language independence, scripting, marketplace
  architecture, build system, and versioning.
- [`docs/reviews/risk-register.md`](../reviews/risk-register.md) — a
  living tracker of 30 findings (severities Critical through Low).
- Three new ADRs: [0008](../decisions/architecture-decision-records/0008-workspace-crate-versioning-lockstep.md)
  (crate versioning, formalizing an existing but undocumented choice),
  [0009](../decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md)
  (plugin ABI versioning — the single most consequential technical
  finding), and [0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)
  (component identity across the language boundary, marked `Proposed`
  pending prototyping).
- [`GOVERNANCE.md`](../../GOVERNANCE.md) — addressing the single largest
  gap the review found: no succession plan or decision-making process
  beyond "the one current maintainer decides."
- A minimal, honest [`.github/CODEOWNERS`](../../.github/CODEOWNERS).
- A config-only CI fix (removed a `RUSTFLAGS` setting that could fail
  builds over warnings in dependency code this project doesn't control)
  and a DCO clause added to `CONTRIBUTING.md`.
- A real bug caught by actually running the tooling rather than reading
  it: `rustfmt.toml` specified nightly-only options that silently
  weren't applied on the pinned `stable` toolchain, and the already-
  committed code didn't pass its own `cargo fmt --check`. Fixed directly
  (mechanical, whitespace-only; rebuild and full test pass confirmed
  afterward).

**Scope note:** per its own instructions, this review did not modify
Rust source code. Two concrete, small, recommended code changes —
`Send + Sync` bounds on ECS component storage, and the ABI version field
in [ADR 0009](../decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md) —
are documented as the top recommended follow-ups for the next
development session, not yet implemented.

## `v0.0.1-pre2` — planned, not started

Expected focus (see [`future-roadmap.md`](future-roadmap.md) for the
dependency graph this is drawn from): the Tier A WASM plugin loader
(Wasmtime integration) and/or the archetype ECS migration. Which comes
first is an open sequencing question for whoever picks this up next — both
are independent of each other, so this is a legitimate place for a future
session (or a new contributor) to make a real, owned decision rather than
one being dictated here in advance.

## `v0.0.1` (unqualified) — not yet scoped in detail

Will be scoped once enough `-preN` progress exists to know realistically
what's left. See the "definition of done" in
[`v0.0.1-roadmap.md`](v0.0.1-roadmap.md#definition-of-done-for-the-unqualified-v001).

## Beyond `v0.0.1`

See [`future-roadmap.md`](future-roadmap.md) and the era-based narrative in
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md).
