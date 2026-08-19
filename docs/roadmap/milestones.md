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

## `v0.0.1-pre2` — skipped

Not used. The original plan for `-pre2` was the Tier A WASM plugin loader
and/or the archetype ECS migration. Both were instead formally deferred
to `v0.0.2` as part of revising `v0.0.1`'s own definition of done (see
below) — there was no remaining gap between what `-pre1` plus the review
fixes delivered and the (deliberately narrowed) bar for unqualified
`v0.0.1`, so no intermediate `-pre2` was needed.

## `v0.0.1` (unqualified) — **Released**

**Status:** Complete.

Completed by revising this milestone's own definition of done (see
[`v0.0.1-roadmap.md`](v0.0.1-roadmap.md#definition-of-done-for-the-unqualified-v001--revised)
for the full reasoning) and then closing every item that revised
definition actually required:

- **Hardened the two highest-severity findings from the architecture
  review**, both landed with tests proving they work, not just that they
  compile: the Tier B plugin ABI gained an explicit version field and a
  forward-extension hook ([ADR 0009](../decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md)),
  validated by a test that compiles a real C plugin declaring a wrong
  version and confirms it's rejected; and ECS component storage gained
  `Send + Sync` bounds, validated by a compile-time guard test.
- **Fixed the entity-generation wraparound risk** (widened `u32` → `u64`,
  moving it from "plausible over years of real uptime" to "not reachable
  by any realistic runtime").
- **Closed the local/CI tooling gap**: `xtask check` now detects and runs
  `clippy` when available instead of unconditionally skipping it.
- **Audited every ADR and every architecture document against the actual
  implementation**, correcting drift found along the way — including a
  pre-existing doc/code mismatch in `plugin-system.md` (a `register`
  lifecycle hook that was never real) caught only by actually reading the
  trait definition against its own documentation.
- **Revised this milestone's own scope**: archetype ECS storage, the
  parallel job scheduler, the Tier A WASM loader, real windowing, and
  change detection were formally moved to `v0.0.2`+, with the reasoning
  recorded rather than silently applied.
- **Recorded two more foundational architectural commitments**, held to
  the same "document the hard cross-cutting decisions before code
  accumulates around an unexamined assumption" standard as rendering,
  physics, and networking: `CanaryUI` (a native UI abstraction,
  bootstrapped on `egui`, shared between the editor and game UI —
  [ADR 0011](../decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md))
  and explicit, identifiable, versionable project state (a founding
  principle for Git-friendly collaboration and marketplace packages —
  [ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md),
  `Proposed`). Neither is implemented; both are architecture only, per
  this release's own scope discipline.
- **Prepared the release**: version bumped to `0.0.1` across the
  workspace, [`CHANGELOG.md`](../../CHANGELOG.md) finalized,
  [`RELEASE_NOTES_v0.0.1.md`](../../RELEASE_NOTES_v0.0.1.md) written, and
  a [release checklist](RELEASE_CHECKLIST.md) completed and checked
  against reality rather than assumed.

Full detail: [`RELEASE_NOTES_v0.0.1.md`](../../RELEASE_NOTES_v0.0.1.md).

## `v0.0.2` (unqualified) — **Released**

**Status:** Complete.

Single focus, per the release cadence recorded in
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md#release-cadence-one-focused-subsystem-per-00x-target-v010-as-substantially-feature-complete):
the archetype-based ECS migration, exactly as scoped in
[`v0.0.2-roadmap.md`](v0.0.2-roadmap.md).

- **Replaced the `v0.0.1` placeholder** (`HashMap<TypeId, HashMap<u32,
  Box<dyn Any + Send + Sync>>>`) with archetype-table storage: entities
  sharing a component-type signature live together, one packed column
  per component type, matching the target design
  [`core-runtime.md`](../architecture/core-runtime.md#ecs-architecture)
  had always described.
- **Added cached queries and change detection**, the two capabilities
  the placeholder explicitly lacked — closing risk register findings
  R-13 (change detection) and, alongside the item below, R-04
  (component identity).
- **Prototyped [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)
  for real**, moving it `Proposed` → `Accepted`: a `CanaryComponent`
  trait plus a `World::register_component` registry, landed as an
  explicit trait impl rather than a derive macro — a real, recorded
  narrowing of scope from what the ADR originally left open, not
  scope creep in the other direction.
- **Grew `canary-ecs`'s test suite from 6 to 19**, all 6 original tests
  passing completely unmodified against the new storage. The new tests
  specifically target the highest-risk part of an archetype
  implementation — the `swap_remove` row-relocation that happens when a
  non-last row leaves an archetype — with both hand-traced examples and
  a `proptest` running arbitrary operation sequences.
- **A prior attempt at this same migration did not make it into this
  history.** An earlier session did equivalent work — by its own
  detailed account, complete except for one missing test module — but
  ended without that work ever being pushed to `dev`; a sandboxed
  working directory that's never pushed doesn't survive past its own
  session. This was discovered, not assumed, at the start of the
  session that produced the entry above: `dev` was checked directly
  against the GitHub API before any code was written, found to contain
  none of the claimed work, and the migration was built again from the
  documented roadmap and ADRs rather than from an assumption that
  undocumented prior work still existed somewhere. Recorded here in the
  same spirit as everything else in this file: what actually happened,
  not what was expected to have happened.
- **Prepared the release**: version bumped to `0.0.2` across the
  workspace (lockstep, per [ADR 0008](../decisions/architecture-decision-records/0008-workspace-crate-versioning-lockstep.md)),
  [`CHANGELOG.md`](../../CHANGELOG.md) updated,
  [`RELEASE_NOTES_v0.0.2.md`](../../RELEASE_NOTES_v0.0.2.md) written, and
  a [release checklist](v0.0.2-RELEASE_CHECKLIST.md) completed and
  checked against reality rather than assumed.

Full detail: [`RELEASE_NOTES_v0.0.2.md`](../../RELEASE_NOTES_v0.0.2.md).

## Beyond `v0.0.2`

`v0.0.3` — Tier A (sandboxed WASM/Wasmtime) plugin loading — is now
scoped: see [`v0.0.3-roadmap.md`](v0.0.3-roadmap.md). Not started. This
was scoped alongside a broader architecture discussion that produced
[ADR 0014](../decisions/architecture-decision-records/0014-change-detection-as-shared-primitive.md)
(change detection as the shared primitive behind replication, live
collaboration, and hot-reload) — recorded as its own ADR rather than
folded in here, since it's a cross-cutting principle for later releases,
not a `v0.0.3`-specific decision.

See [`future-roadmap.md`](future-roadmap.md) and the era-based narrative in
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md) for
everything beyond that.
