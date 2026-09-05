# Project Status

A precise, itemized status of what's actually done versus planned versus
merely documented — built for scanning, not narrative. For the *why*
behind any of this, follow the links; this document intentionally stays
terse. Update this file whenever status changes; unlike the dated
reviews in [`docs/reviews/`](../reviews/), this is a living document, not
a point-in-time record — the same convention as
[`risk-register.md`](../reviews/risk-register.md).

## `v0.0.1` — Released

- [x] Repository, git history, MIT license
- [x] `CONTRIBUTING.md` (including DCO sign-off requirement)
- [x] `CODE_OF_CONDUCT.md`, `SECURITY.md`
- [x] `GOVERNANCE.md` (succession/bus-factor plan, decision process)
- [x] Issue/PR templates, minimal `CODEOWNERS`
- [x] CI (Linux/macOS/Windows build matrix, `wasm32-wasip2` target check)
- [x] Full `docs/` architecture (vision, architecture, decisions, roadmap,
      development, ui, research, reviews)
- [x] 14 ADRs (`0001`–`0014`; see the [ADR index](../decisions/architecture-decision-records/))
- [x] Cargo workspace + `xtask` build orchestration
- [x] `canary-core` — `App`/`Subsystem` bootstrap, structured logging,
      error-handling conventions
- [x] `canary-platform` — `Window`/`InputSource` traits + headless impl
- [x] `canary-ecs` — generational-index `World` (placeholder storage;
      `Send + Sync`-bounded; 64-bit generation counter)
- [x] `canary-plugin-api` — `Plugin` trait, native (Tier B) loader,
      **versioned** C-ABI vtable with a forward-extension hook (ADR 0009)
- [x] `canary-runtime` — headless boot harness, runs end to end
- [x] 16 passing tests, including a real cross-language integration test
      (a C plugin compiled with `gcc` at test time) and a version-mismatch
      rejection test
- [x] `cargo build`, `cargo fmt --check`, `cargo test`, `xtask check` all
      clean
- [x] `CHANGELOG.md`, `RELEASE_NOTES_v0.0.1.md`, `RELEASE_CHECKLIST.md`
- [x] Tagged `v0.0.1` (annotated git tag, local)

**Not in `v0.0.1`, by design** (see
[`v0.0.1-roadmap.md`](v0.0.1-roadmap.md#definition-of-done-for-the-unqualified-v001--revised)):
archetype ECS storage, parallel job scheduler, Tier A (WASM) plugin
loading, real windowing, change detection, rendering, physics,
networking, the editor, `CanaryUI` implementation, `canary-state`
implementation.

## `v0.0.2` — Released

Full detail: [`v0.0.2-roadmap.md`](v0.0.2-roadmap.md). Single focus: the
archetype ECS migration.

- [x] Archetype-based component storage, replacing the `v0.0.1`
      `HashMap<TypeId, HashMap<u32, Box<dyn Any + Send + Sync>>>` placeholder
- [x] Cached queries over archetype tables (replacing the linear scan)
- [x] Change-detection query filters, designed as part of this migration
- [x] A first cut at stable component schema identity (ADR 0010) —
      landed as an explicit trait impl (`CanaryComponent`) plus a
      registry, not a derive macro; see the ADR's "Resolution" section
- [x] Existing `canary-ecs` tests passing against the new storage — all 6
      passed completely unmodified; 13 new tests added (19 total)
- [x] ADR 0010 updated to `Accepted`
- [x] `core-runtime.md`'s "Known limitations" section updated to match

**Explicitly not in `v0.0.2`** (each gets its own later release instead):
the parallel job-stealing scheduler, Tier A WASM plugin loading, real
windowing, rendering, physics, networking, `CanaryUI`, `canary-state`.

## `v0.0.3` — In progress, nearly complete

Full detail: [`v0.0.3-roadmap.md`](v0.0.3-roadmap.md). Single focus:
Tier A (sandboxed WASM Component Model) plugin loading.

- [x] Wasmtime `21.0.2` confirmed and pinned as compatible with this
      sandbox's `rustc` 1.75 floor, empirically — see
      [`docs/development/build-system.md#the-rustc-175-sandbox-validation-floor`](../development/build-system.md#the-rustc-175-sandbox-validation-floor)
- [x] Component loading (fresh and AOT-precompiled), the `Plugin`
      lifecycle through a component
- [x] Structural capability enforcement, proven independently for
      `ecs-read`/`ReadEcsWorld` and `ecs-write`/`WriteEcsWorld`
- [x] A resource budget (memory limit, fuel execution budget), proven
      via a real fuel-exhaustion trap and a real over-budget
      `memory.grow` failure — not merely wired through unverified
- [x] The full first-cut ECS data ABI: `get`/`set`/`has-component`/
      `is-valid-entity`, `SCHEMA_ID`-addressed through the `v0.0.2`
      identity registry plus a new `ComponentValueCodec`/
      `CodecRegistry` for representation
- [x] `docs/architecture/plugin-system.md` and
      [ADR 0003](../decisions/architecture-decision-records/0003-plugin-and-modding-architecture.md)
      updated to match
- [ ] `clippy` verification — same open item every release has hit in
      this sandbox; see the release checklist once this is cut

**Explicitly not in `v0.0.3`** (each is its own tracked follow-up, not
an oversight): a plugin manifest format (R-08), Tier B signing (R-09),
safe hot-unloading with full resource reclamation, and safely lending a
Tier A instance scoped access to a `World` already in use elsewhere
(R-34) — see `v0.0.3-roadmap.md` and the risk register for each.

## `v0.0.4`+ — sequenced, not deeply scoped yet

Per the release cadence in
[`long-term-roadmap.md`](../vision/long-term-roadmap.md#release-cadence-one-focused-subsystem-per-00x-target-v010-as-substantially-feature-complete),
one focus per release. `v0.0.5` is scoped and detailed below; beyond it,
later releases are intentionally not detailed yet, per
[`future-roadmap.md`](future-roadmap.md)'s own "don't assign fake
specificity" discipline:

1. **Real windowing (`winit`-backed `canary-platform`).** Scoped in
   [`v0.0.4-roadmap.md`](v0.0.4-roadmap.md); not yet implemented.
2. **Localization (`canary-loc`).** Scoped in
   [`v0.0.5-roadmap.md`](v0.0.5-roadmap.md) and
   [ADR 0015](../decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md) —
   deliberately moved ahead of rendering, since it's a founding
   constraint ("no hardcoded user-facing text") that gets cheaper the
   earlier it's load-bearing; see that roadmap's "Why this replaces
   what was pencilled in as `v0.0.5`" for the full reasoning. Proven
   standalone, ahead of `CanaryUI`/`canary-assets` existing to consume
   it. Not yet implemented.
3. Beyond this point, ordering is genuinely undecided among rendering
   bootstrap, physics, networking, `CanaryUI`'s `egui` backend, and
   `canary-state`'s medium-term scope — see
   [`future-roadmap.md`](future-roadmap.md) for the dependency graph
   rather than a false ordering here.

## Full architecture-to-implementation map

Every documented subsystem, and where it actually stands. "Documented"
means a real design exists in `docs/architecture/`; "Implemented" is
about working code in `engine/`.

| Subsystem | Documented | Implemented | Notes |
|---|---|---|---|
| Repository/governance | ✅ | ✅ | `v0.0.1` |
| Engine core (`canary-core`) | ✅ | ✅ | `v0.0.1` |
| Platform abstraction | ✅ | ⚠️ Partial | Traits + headless only; real `winit` backend is `v0.0.4`+ |
| ECS | ✅ | ✅ | Archetype-based, cached queries, change detection; `v0.0.2` |
| Plugin system — Tier B (native) | ✅ | ✅ | Versioned ABI (ADR 0009), `v0.0.1` |
| Plugin system — Tier A (WASM) | ✅ | ✅ | Component loading, structural capability enforcement, resource budget, ECS data ABI; `v0.0.3`. Scoped-`World`-access still open (R-34) |
| Rendering | ✅ | ❌ | Designed (incl. 2D-as-specialization); ordering vs. physics/networking/`CanaryUI` undecided — see `future-roadmap.md` |
| Localization (`canary-loc`) | ✅ | ❌ | ADR 0015; `.ftl`/Fluent, `LocKey` type; `v0.0.5` |
| Physics | ✅ | ❌ | Designed (2D+3D via Rapier); not yet scheduled |
| Networking | ✅ | ❌ | Designed (server-authoritative, QUIC); not yet scheduled |
| Scripting system | ✅ | ❌ | Depends on Tier A |
| Asset system | ✅ | ❌ | Not yet scheduled |
| `CanaryUI` (UI toolkit) | ✅ | ❌ | ADR 0011; abstraction layer could start independent of a backend |
| Project state & versioning (`canary-state`) | ✅ | ❌ | ADR 0012 (`Proposed` for identity/package format) |
| Live collaboration | ✅ | ❌ | ADR 0013 (`Accepted` — topology only; protocol/permissions unresolved) |
| Editor | ⚠️ Partial (vision-level) | ❌ | Era 5; blocked on plugin system + rendering + `CanaryUI` |
| CLI/headless operation (editor) | ✅ (principle recorded) | N/A yet | No editor exists to apply it to; proven in spirit by `canary-runtime`/`xtask` today |
| 2D/3D & non-game applicability | ✅ (vision + physics + rendering) | N/A | Positioning + architectural constraint, not a standalone feature |

Legend: ✅ done/exists · ⚠️ partial · ❌ not started · N/A not
applicable as a binary done/not-done item.
