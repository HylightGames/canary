# Project Status

A precise, itemized status of what's actually done versus planned versus
merely documented — built for scanning, not narrative. For the *why*
behind any of this, follow the links; this document intentionally stays
terse. Update this file whenever status changes; unlike the dated
reviews in [`docs/reviews/`](../reviews/), this is a living document, not
a point-in-time record — the same convention as
[`risk-register.md`](../reviews/risk-register.md).

## Current handoff — `v0.0.15` released, `v0.0.16` cut (2026-10-01)

`v0.0.12` audio is released as tag `v0.0.12`, `v0.0.13` (`CanaryUI` + windowed presentation) is released as tag `v0.0.13`, `v0.0.14` (authored project state and simulation snapshots) is released as tag `v0.0.14`, `v0.0.15` (minimal server-authoritative networking) is released as tag `v0.0.15`, and `v0.0.16` (live collaboration) is cut as tag `v0.0.16`. Continue with `v0.1.0` integration per [`v0.1.0-plan.md`](v0.1.0-plan.md). What `.13` landed: window
presentation with live lifecycle gates (`present_clear.rs`: steady,
minimize/restore, resize→recreate, content-present, safe destruction;
capability-rejection/fatal-error mappings unit-gated), the platform
pointer/focus event slice, `canary-input` capture routing with in-mapper
pass-through, `canary-ui-core` + `canary-ui-egui` (egui 0.36 paint output
through the same RHI pass as the scene), `Runtime::drive_frame` owning
event pump → UI routing → tick → schedule, the migrated headless harness,
and the `ui-game` sample with a live proof record (600/600 presented,
mapped movement + Space/click fire, +1-per-click no-double-fire). ADR
0025 is Accepted; ADR 0024 items 1–5 Accepted, item 6 (typed lifecycle
failure semantics) remains Proposed. R-37 and R-38 are mitigated (see
[risk-register](../reviews/risk-register.md)). R-36 remains open for
richer lifecycle needs such as pause/reload/replacement; those are not
a `.13` gate. `.14` authored project state and simulation snapshots
are released as tag `v0.0.14` — see the `v0.0.14` section below. `.15`
minimal server-authoritative networking is released as tag `v0.0.15` —
see the `v0.0.15` section below. `.16` live collaboration is cut as
tag `v0.0.16` — see the `v0.0.16` section below.

With `.16` cut, the next milestone is `v0.1.0` integration in a small sample game. The detailed sequence
and out-of-scope items are in [`v0.1.0-plan.md`](v0.1.0-plan.md); long-term
editor, visual scripting, and ecosystem work is in
[`future-roadmap.md`](future-roadmap.md).

## `v0.0.1` — Released

- [x] Repository, git history, MIT license
- [x] `CONTRIBUTING.md` (including DCO sign-off requirement)
- [x] `CODE_OF_CONDUCT.md`, `SECURITY.md`
- [x] `GOVERNANCE.md` (succession/bus-factor plan, decision process)
- [x] Issue/PR templates, minimal `CODEOWNERS`
- [x] CI (Linux/macOS/Windows build matrix, `wasm32-wasip2` target check)
- [x] Full `docs/` architecture (vision, architecture, decisions, roadmap,
      development, ui, research, reviews)
- [x] 14 ADRs (`0001`–`0014`; see the [ADR index](../decisions/architecture-decision-records/README.md))
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
- [x] `CHANGELOG.md`, `docs/release-notes/v0.0.1.md`, `RELEASE_CHECKLIST.md`
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

## `v0.0.3` — Implemented; tagged `v0.0.3` (backfill 2026-09-28)

Full detail: [`v0.0.3-roadmap.md`](v0.0.3-roadmap.md). Single focus:
Tier A (sandboxed WASM Component Model) plugin loading.

- [x] Wasmtime `21.0.2` confirmed and pinned as compatible with this
      sandbox's `rustc` 1.75 floor, empirically — see
      [`docs/development/build-system.md#the-rustc-175-sandbox-validation-floor`](../development/build-system.md#the-rustc-175-sandbox-validation-floor)
      (floor retired 2026-09; wasmtime now tracks latest-stable caret,
      48.x as of this writing — see build-system.md History)
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
- [x] `clippy` verification — this sandbox couldn't reach `clippy` when
      `v0.0.3` was implemented; it can now (see `v0.0.4`'s own entry
      below for when and how that changed), and a workspace-wide check
      confirmed `v0.0.3`'s own code is clean

**Explicitly not in `v0.0.3`** (each is its own tracked follow-up, not
an oversight): a plugin manifest format (R-08), Tier B signing (R-09),
safe hot-unloading with full resource reclamation, and safely lending a
Tier A instance scoped access to a `World` already in use elsewhere
(R-34) — see `v0.0.3-roadmap.md` and the risk register for each.

## `v0.0.4` — Implemented; tagged `v0.0.4` (backfill 2026-09-28)

Full detail: [`v0.0.4-roadmap.md`](v0.0.4-roadmap.md). Single focus:
real, `winit`-backed windowing.

- [x] `WinitWindow`/`WinitInput`, alongside (not replacing)
      `HeadlessWindow`/`HeadlessInput`, behind a `winit-backend` Cargo
      feature off by default — confirmed mechanically (`cargo tree`)
      that a build without it pulls in no `winit`/Wayland dependency
- [x] The `pump_app_events` pull/push bridge, documented as a real,
      acknowledged tradeoff rather than a frictionless fit
- [x] `Key` expanded to a realistic keyboard (letters, digits, function
      keys, modifiers, navigation, editing keys, punctuation), modeled
      on physical position
- [x] A real, `#[ignore]`d-by-default integration test against a live
      `Xvfb` display: window creation, several `poll_events()` cycles,
      a real keyboard press synthesized via the X11 XTEST extension,
      and a real ICCCM `WM_DELETE_WINDOW` close signal — run
      automatically in CI's dedicated `windowing-integration` job, not
      just documented as runnable
- [x] Two real `winit` constraints found via direct testing, not
      assumed, and worked around: `EventLoop::new()` panics off the
      main thread (where `cargo test` runs each test), and only one
      `EventLoop` can exist per process, ever — see
      `platform-abstraction.md`'s "Status in this foundation" for the
      full detail
- [x] The `winit`/Wayland pin set re-verified at implementation time,
      per its own "re-verify, don't assume it still holds" caveat — and
      it had drifted: two more pins needed beyond the three found while
      scoping (`build-system.md`)
- [x] `docs/architecture/platform-abstraction.md`'s "Status in this
      foundation" rewritten to match
- [x] `cargo build`/`fmt --check`/`test`/`doc` clean; `clippy` clean —
      **`clippy` became reachable in this sandbox for the first time
      this session** (a real local-capability change, not luck; every
      prior release's checklist had this as an open item). Used to
      confirm `v0.0.4`'s own new code is clean, and separately (own
      commit, not mixed into this release's work) to clear the small
      number of pre-existing warnings in unrelated older code that
      surfaced once `clippy` was finally reachable

**Explicitly not in `v0.0.4`**: rendering (a window with nothing drawn
into it — see `v0.0.6`), gamepad/joystick/IME input, multi-window
support (a real `winit` constraint, not just an unimplemented feature —
see `v0.0.4-roadmap.md`), mobile/console windowing, and a macOS/Windows
equivalent of the real windowing integration test (Linux/`Xvfb`/XTEST
only for now).

## `v0.0.5` — Implemented; tagged `v0.0.5` (backfill 2026-09-28)

Full detail: [`v0.0.5-roadmap.md`](v0.0.5-roadmap.md) and
[ADR 0015](../decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md)
(now `Accepted`). Single focus: `canary-loc` — `LocKey` + Fluent (`.ftl`)
resolution, moved ahead of rendering since it's a founding constraint
that gets cheaper the earlier it's load-bearing.

- [x] `LocKey` + the `key!` macro, validating Fluent's identifier
      grammar at **compile time** — confirmed against `fluent-syntax`'s
      own parser source, not assumed from the spec; a real
      `compile_fail` doctest proves an invalid key genuinely fails to
      build
- [x] `LocaleBundle`, resolving keys against a fallback chain computed
      via `fluent-langneg` — proven against real `.ftl` content, not
      mocked: a plain message, a pluralized message correctly branching
      between CLDR categories, string interpolation, an unavailable
      requested locale falling back to the configured default, and a
      real edge case (default locale configured but with zero
      actually-loadable content) degrading gracefully rather than
      panicking
- [x] The missing-key/fallback `tracing::warn!`/`tracing::debug!` events
      verified to actually fire, not just present in the source — a
      real capturing `tracing` layer built specifically to check this,
      which caught two genuine bugs in the test harness itself (a field
      visitor only capturing the `message` field, and an
      originally-wrong test scenario that exercised locale-negotiation
      fallback instead of the per-key resolution fallback it claimed to
      test) before either test could honestly pass
- [x] The `std::fs`-based placeholder loader
      (`discover_available_locales`/`load_locale_resources`),
      doc-commented as temporary and naming `canary-assets` as its intended
      replacement, deliberately decoupled from `LocaleBundle` (which
      takes any loader closure) so that replacement won't require
      touching `LocaleBundle`'s own logic
- [x] Toolchain pins re-verified at implementation time, not trusted
      from the roadmap's own scoping pass — and found genuinely
      incomplete: two more pins needed (`unic-langid-macros`,
      `unic-langid-macros-impl`, both `=0.9.5`) beyond what scoping
      found, since `canary-loc` uses `unic-langid`'s `macros` feature
      (for `langid!()`), which the scoping-phase spike never exercised
      (`build-system.md`)
- [x] `docs/architecture/localization.md`'s "Status in this foundation"
      rewritten to match
- [x] `cargo build`/`fmt --check`/`test`/`doc` clean; `clippy` clean
      (`-D warnings`, matching CI exactly) across the full workspace
      with `canary-loc` added as a new member

**Explicitly not in `v0.0.5`**: `CanaryUI` integration (no widget API
exists yet), compile-time validation that a key resolves against real
`.ftl` content (only its syntax is checked), `fluent-templates`/
`fluent-fallback`/`i18n-embed`, and any translator tooling (Weblate or
otherwise) — see ADR 0015 for the reasoning behind each.

## `v0.0.6` — Implemented; tagged `v0.0.6` (backfill 2026-09-28)

Full detail: [`v0.0.6-roadmap.md`](v0.0.6-roadmap.md) and
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md).
Single focus: the RHI trait (`canary-render`) and its first backend
(`canary-render-vulkan`, via `ash`) — native per-graphics-API backends,
not a `wgpu` bootstrap, superseding ADR 0004's original backend choice
per direct project direction.

- [x] `canary-render`'s RHI trait (`RenderDevice`/`CommandEncoder`),
      deliberately minimal for this release's actual milestone — not a
      speculatively complete GPU abstraction
- [x] `canary-render-vulkan` implementing that trait, with real resource
      cleanup (an `Rc<ash::Device>` shared across every resource type's
      own `Drop` impl, not deferred to process exit)
- [x] The real milestone: a hard-coded triangle, rendered to a real
      offscreen color target on this sandbox's real `llvmpipe` Vulkan
      device, via a real WGSL shader cross-compiled to SPIR-V through
      standalone `naga` — read back and asserted on specific pixel
      values, not "it compiled" or "it didn't panic"
- [x] `canary-render` confirmed to have zero dependencies at all
      (`cargo tree -p canary-render` shows nothing, not even
      transitively) — checked mechanically, not just claimed
- [x] A real, worthwhile correction caught by actually building rather
      than assumed: ADR 0016's first draft described backend opt-in-ness
      as "a Cargo feature on `canary-render`," which turns out to be
      structurally impossible (`canary-render-vulkan` depends on
      `canary-render` for the trait, so a feature flowing the other way
      would be a literal dependency cycle, which Cargo rejects
      outright). The actual property holds anyway, achieved through
      Cargo's dependency graph rather than a feature flag — corrected in
      ADR 0016, `rendering.md`, and the roadmap rather than left stale
- [x] `cargo build`/`fmt --check`/`test`/`doc` clean; `clippy` clean
      (`-D warnings`, matching CI exactly) across the full workspace
      with both new crates added as members

**Explicitly not in `v0.0.6`**: the render graph, a materials/shader-
variant system, live window presentation (deferred until `v0.0.4`'s
windowing — which has since landed anyway — is wired up to it),
`canary-render-metal`/`canary-render-dx12`/`canary-render-gl`, 2D-specific
rendering, textures/depth-testing/blending/multiple draw calls, and real
GPU hardware validation (this release's automated coverage is
`llvmpipe`-only) — see ADR 0016 and the roadmap for the reasoning behind
each.

## `v0.0.7` — Implemented; tagged `v0.0.7` (backfill 2026-09-28)

Full detail: [`v0.0.7-roadmap.md`](v0.0.7-roadmap.md) and
[`docs/architecture/execution-model.md`](../architecture/execution-model.md).
Single focus: the ECS data-access architecture a scheduler needs,
decided (rather than picked from the previously-open render-graph/
physics/UI/state options) by the September 2026 external review triage
([`docs/reviews/triage/2026-09-review-triage.md`](../reviews/triage/2026-09-review-triage.md)) —
two independent reviews converged on the same gap `core-runtime.md`'s
own "Threading & the job system" section had already implied but never
made load-bearing.

- [x] `Tick(u32)` → `Tick(u64)` — a plain `Ord`-derived `tick > since`
      comparison is only correct if `Tick` never wraps during a
      `World`'s lifetime; at `u32`, a long-lived server advancing the
      tick once per frame at 60Hz wraps in about 2.3 years of continuous
      uptime. Mirrors the same reasoning already applied to
      `Entity::generation`
- [x] Typed resource storage (`World::insert_resource`/`resource`/
      `resource_mut`/`remove_resource`/`contains_resource`/
      `resource_changed_since`) — globally-unique, engine-owned state
      addressed by type, with the same `Tick`-based change detection
      every component column already has
- [x] Multi-component queries: `World::query2`/`query3` (read-only
      archetype-set intersection across 2–3 component types) and
      `World::query2_mut` (one mutable, one shared — the "update `A`
      based on `B`" shape) — deliberately narrow, hand-written methods
      rather than a fully generic `Query<D>` over arbitrary tuples,
      mirroring this crate's own established "manual impl before derive
      macro" precedent ([ADR
      0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md))
- [x] `docs/architecture/execution-model.md` — the design document these
      implement a first cut of, including the "five invariants"
      (ownership, access, time, identity, side-effects) adopted from the
      second review's closing argument
- [x] `canary-ecs`'s first `unsafe` code (`Archetype::column_pair_mut`,
      needed for `query2_mut`'s simultaneous mutable+shared column
      access), reasoned through explicitly rather than reached for by
      default — flagged prominently in `execution-model.md` ("On
      `unsafe`") since
      [`coding-standards.md`](../development/coding-standards.md#unsafe-code)
      names `canary-ecs` as outside the two boundaries where `unsafe` is
      expected
- [x] `cargo build`/`fmt --check`/`test`/`clippy -D warnings` clean
      across the full workspace, with and without `winit-backend`,
      re-verified against this sandbox's `rustc`/`cargo` 1.91 (see
      [`build-system.md`](../development/build-system.md#the-rustc-175-sandbox-validation-floor))
      after every change

**Explicitly not in `v0.0.7`**: the scheduler/job-stealing system itself
(the entire point of this release is its prerequisite, not the thing
itself), command buffers and events (nothing parallel exists yet to need
either from), first-class time types (`WallClock`/`FrameTime`/
`SimulationTime` — no frame loop sophisticated enough to need them yet),
a fully generic `Query<D>` trait or query-composed filters, mechanical
enforcement of the five invariants, and mixed-mutability query shapes
beyond `query2_mut`'s one-mutable-one-shared — see the roadmap doc for
the reasoning behind each.

## `v0.0.8` — Implemented; tagged `v0.0.8` (backfill 2026-09-28)

Full detail: [`v0.0.8-roadmap.md`](v0.0.8-roadmap.md) and
[`docs/architecture/execution-model.md`](../architecture/execution-model.md#the-scheduler).
Single focus: the scheduler `v0.0.7`'s access-model work was explicitly
the prerequisite for — picked over the render graph/physics/`CanaryUI`/
`canary-state` options `v0.0.7` had left open, since the access-model
work would otherwise sit unused for a release.

- [x] New crate `canary-scheduler`, depending only on `canary-ecs`'s
      public API (no privileged access to `World`'s internals)
- [x] `SystemAccess` — a system's declared reads/writes, component and
      resource types tracked separately, built as an explicit chainable
      declaration rather than inferred from a system function's
      signature
- [x] `Schedule` — greedily batches systems, in registration order,
      into stages (one or more read-only systems, or exactly one
      system that writes anything), running multi-system stages'
      systems concurrently via `std::thread::scope`
- [x] Real, measured concurrent execution for read-only stages — proven
      with a timing-based test and a concurrency counter, not just
      exercised
- [x] `cargo build`/`fmt --check`/`test`/`clippy -D warnings` clean
      across the full workspace, with and without `winit-backend`,
      after adding the new crate

**Explicitly not in `v0.0.8`**: concurrent execution of two *write*
systems, even with provably-disjoint access (found during
implementation to need a substantially larger `unsafe` undertaking than
this release's scope justifies — see the roadmap doc), automatic access
inference from a system's signature, wiring `Schedule` into
`canary-core`'s `App`/`Subsystem` tick loop, a persistent work-stealing
thread pool, and command buffers/events (this scheduler's specific
design still means neither has a live race to prevent yet) — see the
roadmap doc for the reasoning behind each.

## `v0.0.9`+ — decided: the `v0.1.0` plan

Superseded by direct project-owner instruction (September 2026): the
"genuinely undecided" framing this section previously had is resolved.
`v0.1.0`'s bar is now "a competent developer could write a real, small
game directly against Canary's runtime" — full reasoning in
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md#v010-sharpened-integration-not-a-checklist),
concrete dependency-ordered sequence in
[`docs/roadmap/v0.1.0-plan.md`](v0.1.0-plan.md). Short version:
`v0.0.9` (real-time `App`/`Subsystem` loop, `Transform`, ECS-driven
rendering) → `v0.0.10` (asset loading) → `v0.0.11` (physics, 2D first)
→ `v0.0.12` (audio) → `v0.0.13` (`CanaryUI`) → `v0.0.14` (project state)
→ `v0.0.15` (networking) → `v0.0.16` (live collaboration) → `v0.1.0`
(a real sample game proving the whole set actually integrates). The
editor, marketplace, and beginner-friendly tooling remain explicitly
deferred past `v0.1.0`, unchanged from this document's prior framing.

**`v0.0.9` is implemented on `dev`; tagged `v0.0.9` (backfill 2026-09-28)** — all three
parts landed: real delta-time + wall-clock `App::run`;
`canary-transform` (`Transform`/`GlobalTransform`/`Parent`/
`Children` + scheduler-registered hierarchy propagation with a
quiet-tick skip plus `despawn_subtree` removal, 42 tests);
and ECS-driven rendering, also on `dev`. The rendering half is a new
`canary-render-ecs` bridge crate (`Renderable` + `extract_scene` +
CPU-bake to a `BakedFrame` resource + `draw_baked_frame` through the
unchanged RHI, zero RHI churn), wired propagation-then-bake into
`canary-runtime`'s `EcsSubsystem::tick` via `Schedule`, proven by
three `#[ignore]`-gated offscreen pixel tests
(`engine/canary-render-ecs/tests/render_ecs_readback.rs`: distinct
colors, moved-entity redraw, empty-scene clear), with
`examples/spinning-cube` rewritten on the bridge (root + six face
entities, quaternion spin, GIF output kept). Full design record in
[`docs/architecture/rendering.md`](../architecture/rendering.md#v009-the-ecs-to-render-bridge-canary-render-ecs).
The `canary-runtime` harness advances its `World` tick once before each
scheduled simulation run, matching ADR 0021's runner-owned tick rule and
allowing transform propagation to skip quiet runs.
Still open past `v0.0.9`: the RHI upgrades the bridge deliberately
defers (push constants/uniforms, depth/culling, buffer updates,
materials past the texture-only slice, swapchain/presentation), the
camera component, and the App-level scheduler. Mesh assets and the
texture-only slice have since landed in `v0.0.10` (see below); the
rest stays open — all `v0.0.10+` scope.

## `v0.0.10` — Implemented; tagged `v0.0.10` (backfill 2026-09-28)

Full detail: [`v0.0.10-roadmap.md`](v0.0.10-roadmap.md) and
[ADR 0018](../decisions/architecture-decision-records/0018-asset-handles-and-synchronous-loading.md).
Single focus: minimal asset loading — real files from disk feed the
renderer `v0.0.9` built.

- [x] New crate `canary-assets`, depending only on `canary-ecs` plus
      loading libraries (`sha2 0.11`, `gltf` without default features
      plus `utils` only, `png`): `AssetId` (opaque SHA-256 over file
      bytes plus `LOADER_VERSION`, layout documented as provisional),
      `AssetHandle<T>` (generational index plus generation, mirroring
      `Entity`), `AssetStore<T>` (generational slots living as an ECS
      resource; stale handles resolve to `None`, never panic), and
      `AssetError` (typed failures with path context)
- [x] Synchronous path-based GLB mesh loader (`Mesh` with positions,
      stored-but-unused normals/UVs, and indices; triangle-mode only,
      index bounds and attribute consistency validated at load) and PNG
      texture loader (`Texture` RGBA8-normalized, deeper samples
      downsampled to the high byte, enforced decode budget), with
      checked-in hash-stable fixtures (`quad.glb`, `box.glb`,
      `rgba2x2.png`, `rgba16-2x1.png`), known-value assertions, and
      negative controls proving every rejection path returns `Err`
- [x] File-loaded meshes through the unchanged RHI (`MeshRenderable`,
      index-to-soup expansion at the bridge, scheduled mesh bake);
      `spinning-cube` loads its faces from `box.glb`, hardcoded arrays
      deleted
- [x] Minimal bounded RHI texture addition (`TextureDescriptor`,
      `create_texture`, `create_textured_pipeline`,
      `CommandEncoder::set_texture`; UVs on the existing `Float32x2`;
      single-texture fragment sampling) with a Vulkan backend
      implementation; `TexturedRenderable` plus `BakedTexturedFrame`
      through the bridge; `canary-render` still holds zero
      dependencies, checked mechanically with `cargo tree`
- [x] Seven `#[ignore]`-gated offscreen pixel tests (the three soup
      tests verbatim, two mesh tests, two texture tests including the
      negative control that fails on the untextured pipeline), plus the
      Vulkan hello-triangle test — all green on real ICDs
- [x] No third-party types in any `canary-assets` public signature,
      checked via `cargo doc` with zero warnings
- [x] `canary-loc` placeholder migration explicitly deferred, not
      forced: the attempt stopped at the API survey, since
      `canary-assets` exposes no raw file-byte or string primitive to
      rewire a `.ftl` text loader onto, and adding one would bend the
      asset API — the `std::fs` placeholder stands per its own
      delete-and-replace contract
- [x] `cargo build`/`fmt --check`/`test`/`doc` clean; `clippy` clean
      (`-D warnings`, matching CI exactly) across the full workspace,
      with and without `winit-backend`, plus the `wasm32-wasip2` check;
      no new toolchain pins needed (caret requirements on `sha2`,
      `gltf`, `png` build clean on a current stable toolchain)

**Explicitly not in `v0.0.10`**: async/background loading,
filesystem watching/hot reload, a cache directory, cooked formats and
any `xtask cook` step, importers-as-plugins, materials, depth testing,
blending, buffer/texture updates, swapchain/presentation, second
mesh/texture formats, mipmaps, and sRGB handling past
normalize-to-RGBA8 — see the roadmap doc for the reasoning behind
each.

## `v0.0.11` — Implemented; tagged `v0.0.11` (backfill 2026-09-28)

Full detail: [`v0.0.11-roadmap.md`](v0.0.11-roadmap.md) and
[ADR 0019](../decisions/architecture-decision-records/0019-physics-backend-lineup.md).
Single focus: 2D physics — real simulated bodies move real ECS
`Transform`s, drawn through the renderer `v0.0.9`/`v0.0.10` built.

- [x] New crate `canary-physics`, depending on `canary-ecs`,
      `canary-scheduler`, and `canary-transform` plus `glam`/
      `thiserror` and `rapier2d` (composition upward through
      `canary-runtime`'s `EcsSubsystem`; nothing render-side knows
      about it)
- [x] Object-safe, leak-free `PhysicsBackend` trait (`create_body` /
      `attach_collider` / fixed-only `step` / `sync_transform` /
      `remove_body` / `set_velocity` / `apply_impulse` /
      `set_gravity` / `gravity` / `body_count`): no rapier or
      `nalgebra` type in any public signature, checked via `cargo
      doc` plus grep; stale handles report `None`/`false`, creation
      and stepping fail with typed `PhysicsError`
- [x] Minimal components: `RigidBody` (dynamic, fixed,
      position-kinematic), `Collider` (ball, cuboid, capsule, every
      scalar validated finite and positive), `Velocity`,
      `GravityScale`, `LockedAxes`, one `ColliderMaterial`, and the
      `PhysicsConfig` resource (gravity plus ADR 0019
      dimension/backend selection, `dimension = "2d"` / `backend =
      "rapier"`, both `#[non_exhaustive]`)
- [x] Private `RapierBackend` over rapier2d (`"0.35"`, locked at
      `0.35.3`): generational slot map, per-step gravity forwarding,
      `reset_forces` hygiene, boundary finiteness checks, sync-side
      NaN guard that skips instead of poisoning; default features
      only (no `parallel`, no `serde-serialize`)
- [x] Fixed-step system (`PhysicsClock` accumulator plus
      `SimulationTime`, distinct from ECS `Tick`; `FrameDelta`
      resource through the existing `tick(dt)` seam, no `App`
      redesign): at most four `1/60` s steps per tick, leftover
      dropped by the spiral guard; sync writes x/y plus z-rotation
      only, preserving plane depth, off-axis swing, and scale
- [x] First-position registration (`register_physics_step` before
      propagation, soup, mesh, and textured bakes), pinned both
      directions: fresh global when first, provably stale global
      when reversed
- [x] Box2D measured against (3.2.0 via `boxdd`, throwaway harness
      that never became a dependency): 0.053 vs 0.060 ms at 100
      boxes, 0.171 vs 0.205 ms at 300 (rapier faster 1.13–1.19x);
      no re-decision, rapier stands per ADR 0019, no Box2D backend
      shipped
- [x] Game proof (`engine/canary-render-ecs/tests/
      physics_game_proof.rs`): ground, falling box, scripted
      kinematic platform as z-pinned quads through the unchanged
      soup bake (zero RHI churn) — 4 headless tests in the normal
      suite, 3 pixel tests `#[ignore]`-gated for real Vulkan ICDs
      with an unstepped-pipeline negative control
- [x] Determinism scoped to single-machine repeatability (same
      steps, bit-identical trajectory), proven at the bit level —
      explicitly not cross-platform
- [x] 66 `canary-physics` tests green, `cargo build`/`fmt
      --check`/`test`/`doc` clean; `clippy` clean (`-D warnings`,
      matching CI exactly) across the full workspace, with and
      without `winit-backend`, plus the `wasm32-wasip2` check where
      applicable; CI green including the rendering-integration job

**Explicitly not in `v0.0.11`**: joints, scene queries,
velocity-kinematic bodies, trimesh/heightfield/polyline colliders,
sustained-force APIs (later physics milestones); 3D physics — Jolt
(canonical) and Rapier3D (alternative) arrive with the 3D release,
post-`v0.1.0`; a Box2D backend (measured, beaten, not shipped); a
real 2D/orthographic camera (deferred with the renderer's camera
work); `App`-level scheduler redesign and ECS rework (explicitly
refused — the subsystem self-steps); input-driven control (the
platform is pose-scripted, no input system yet) — see the roadmap
doc for the reasoning behind each.

## `v0.0.12` — Released as tag `v0.0.12` (2026-09-28)

Full detail: [`v0.0.12-roadmap.md`](v0.0.12-roadmap.md) and
[ADR 0023](../decisions/architecture-decision-records/0023-audio-bootstrap-rodio-behind-custom-trait.md).
Single focus: audio playback — a game-state change makes a sound,
through Canary's own trait with a rodio bootstrap behind it.

- [x] New crate `canary-audio`, depending on `canary-ecs`,
      `canary-scheduler`, `canary-transform`, and `canary-assets`
      (`Sound`) plus `thiserror` and `rodio` (composition upward
      through `canary-runtime`; nothing render-side knows about it)
- [x] Object-safe, leak-free `AudioBackend` trait (play/stop/
      pause/resume/volume on source handles, listener pose, per-tick
      pump): no `rodio`/`cpal`/`symphonia` type in any public
      signature, checked via `cargo doc` plus grep; unknown handles
      retry, pump/volume failures never fail the tick
- [x] Minimal components: `AudioSource` (game-owned trigger intent),
      `AudioListener`, system-owned voice table (no field written
      from both sides), and the `AudioConfig` resource (master
      volume plus `#[non_exhaustive]` backend selection)
- [x] Private rodio backend (`"=0.22.2"`, exact pin; decode-only
      `hound` + `lewton` features, Symphonia/MPL-2.0 absent from the
      graph, verified): Canary-owned distance attenuation (rodio's
      `SpatialPlayer` avoided per its open upstream bug); no-device
      construction/pump degrades typed, never panics (headless-proven)
- [x] Trigger system (game-state transitions drive the backend,
      removal/despawn reaps voices with zero orphans), pinned both
      directions plus a stub-backend game proof asserting exact call
      order
- [x] Sound-asset prerequisite: `Sound` + WAV/Ogg Vorbis loaders in
      `canary-assets` (bit-exact fixtures, budgets, confinement)
- [x] 33 `canary-audio` tests green, `cargo build`/`fmt
      --check`/`test`/`doc` clean; `clippy` clean (`-D warnings`,
      matching CI exactly) across the full workspace, with and
      without `winit-backend`, plus the `wasm32-wasip2` check where
      applicable

**Explicitly not in `v0.0.12`**: DSP graph, buses, HRTF/Doppler,
gapless-music guarantees, streaming sources, FMOD/Wwise bindings,
WASM output proof, and the custom in-house engine (retained as the
long-term default per ADR 0023 — it replaces the private backend,
never the trait) — see the roadmap doc for the reasoning behind
each.

## `v0.0.14` — Released as tag `v0.0.14` (2026-09-29)

Single focus: authored project state + deterministic simulation
snapshots as two separate products sharing one codec vocabulary. Work
packages per [`v0.1.0-plan.md`](v0.1.0-plan.md#v0014--project-state);
contract in [ADR 0026](../decisions/architecture-decision-records/0026-authored-state-and-simulation-snapshot-contract.md)
(Accepted; encoding selection recorded 2026-09-28) and
[`state-and-versioning.md`](../architecture/state-and-versioning.md#status-in-this-foundation).

- [x] WP1 (design package): version domains, unknown-vs-missing
      semantics, canonical ordering, atomic persistence, and
      change-tracking-vs-tick separation specified before code. The
      stable `LogicalAssetId` registry stays future work — asset
      references resolve through a caller-supplied function (`$asset`
      markers; `StateError::AssetUnresolved` on miss).
- [x] WP2 (authored state): `engine/canary-state/src/authored.rs`
      (canonical pretty JSON, one-level prefab tables with per-field
      object merge, change log, atomic save, staged load),
      `spawn_plan.rs` (`SpawnPlan::from_document`: dep-free prefab bake
      that never rewrites the document), `value.rs`
      (`SnapshotValue::from_json`/`to_json`), `migration.rs` (linear
      chains + `migrate_fields` bridge), `error.rs`
      (`PlacementFailed`, `UnresolvableEntityRef`).
      `engine/canary-runtime/src/authored_spawn.rs`
      (`AuthoredSpawner`/`SpawnDecoder`/`StagedInsert`): staged
      two-phase spawn — full validation before the first `World::spawn`.
- [x] WP3 (simulation boundary):
      `engine/canary-state/src/snapshot.rs` (envelopes, SHA-256
      checksum, `SimStateSnapshot` reserved record, atomic
      `save/load_snapshot` helpers) and
      `engine/canary-runtime/src/simulation_snapshot.rs`
      (`SnapshotRegistry` capture/checksum/restore,
      `Simulation::step` with owned `OwnedRng` stream + `SimClock`
      resource, `SimComponent`/`SimResource` participation seams,
      presentation exclusion). Restores are staged
      validate-before-mutate; RNG/clock persist via the reserved
      sim-core record with no wire-format break (the golden-bytes test
      still pins the pre-sim-state encoding).
- [x] WP4 (fixtures/proof, engine side): migration + unknown-data
      fixtures, same-seed determinism across save/restore,
      presentation-exclusion sentinels (including byte scans of the
      capture payload), interrupted-write recovery on both products,
      capture→restore→recapture byte stability (scoped to the LIFO
      free-stack discipline), and the phase-two re-decode purity proof.
      Docs side (this file, `state-and-versioning.md`, risk/triage
      notes) closed by the docs lane in the same change.

**Explicitly not in `v0.0.14`**: the `LogicalAssetId` registry, nested
prefab inheritance, cross-history byte identity (claimed only for
identical declared state), transactional restore against impure
decoders, the durable removal/destruction log replication needs (R-33,
a `.15` item), and any wire-compatibility promise (networking's
concern per ADR 0026).

Full scope: [`docs/release-notes/v0.0.14.md`](../release-notes/v0.0.14.md).

## `v0.0.15` — Released as tag `v0.0.15` (2026-09-30)

Single focus: minimal server-authoritative networking per
[`v0.1.0-plan.md`](v0.1.0-plan.md#v0015--networking). Design record in
[ADR 0027](../decisions/architecture-decision-records/0027-minimal-server-authoritative-replication.md)
(Accepted 2026-09-30, code-match basis) and
[`networking.md`](../architecture/networking.md#status-in-this-foundation).

- [x] WP1 (boundary): `engine/canary-net` created + workspace member;
  `src/ids.rs` (version/identity/counter newtypes), `transport.rs`
  (`NetTransport` + `QuinnTransport`, ALPN `canary-1`, DER-bytes
  constructors, Quinn pinning tests), `frame.rs`/`envelope.rs`
  (length-prefix framing, SHA-256 checksums), `limits.rs`/`error.rs`
  (bounds, typed errors/disconnects)
- [x] WP2 (representation): `src/replication.rs` (canonical
  snapshot/delta, base-sequence basing, validate-all-before-apply),
  `tombstone.rs` (`TombstoneLog`, bounded + ack-gated),
  `mapping.rs` (`NetEntityMap`), `policy.rs` (`ReplicationRegistry`),
  `codec.rs` (`SchemaCodecs`); `engine/canary-ecs/src/replication.rs`
  (`Replicated` marker) + `column.rs` (`Tick::get()`/`from_raw()`)
- [x] WP3 (session): `src/handshake.rs` (typed Hello/Welcome/Reject +
  `TemporarilyBanned`), `input.rs` (slot/window/sequence validation,
  `InputAck`), `session.rs` (`SessionTable`, `ClientAck`, whole-record
  disconnect), `queue.rs` (bounded, Disconnect vs DropOldest), `sequence.rs`
  (`SequenceGate`)
- [x] WP4 (hardening/proof): `src/policy.rs`
  (`ConnectionPolicy`/`IdleTracker`/`HandshakeGate`, fake `u64` clock),
  `metrics.rs` (capped per-client rows), `resync.rs`
  (`BaselineRetention` + `ResyncPlan`; no per-delta journal, supersede
  rationale documented), `loopback.rs` (fault injection, resync proof,
  5-cell matrix), `tests/session_roundtrip.rs` (separate-process QUIC
  proof over real TLS/ALPN)

**Explicitly not in `v0.0.15`**: per-delta journal replay, an unreliable
datagram lane, client prediction/reconciliation/rollback, production
identity/matchmaking, cross-build wire compatibility — see ADR 0027.

Full scope: [`docs/release-notes/v0.0.15.md`](../release-notes/v0.0.15.md).

## `v0.0.16` — Released as tag `v0.0.16` (2026-10-01)

Single focus: live collaboration per
[`v0.1.0-plan.md`](v0.1.0-plan.md#v0016--live-collaboration). Design record in
[ADR 0028](../decisions/architecture-decision-records/0028-authoritative-live-collaboration-operations.md)
(Accepted, code-match basis) and
[`live-collaboration.md`](../architecture/live-collaboration.md).

- [x] WP1 (operation contract): ADR 0028 WP1 selections closed (target-ID
      type, additive in-document history/version-lineage model, prefab-veto
      owner, numeric-validation owner); stable `(actor, client-op-id)`
      idempotency, server-assigned sequences, target-scoped compare-and-set
      conflicts, resync-or-reject recovery
- [x] WP2 (edit surface): one operation — complete local-transform
      replacement on an existing authored entity (`op.rs`, numeric
      validation owned by `canary-collab`); owner/editor/reader roles
      provisioned out-of-protocol (`permissions.rs`, fencing epoch)
- [x] WP3 (authoritative session): `engine/canary-collab/src/session.rs`
      (stages 1–11, durable commit before ack/broadcast, ordered outbox)
      composed in `engine/canary-runtime/src/collab_session.rs` over
      `canary-net` framing; two-client proof over the transport plus
      restart fencing (epoch bump, no sequence reuse or double apply)
- [x] WP4 (recovery/limits): retained-history tail or checkpointed-snapshot
      resync, 1 MB snapshot ceiling with typed `TooLarge` (content gate +
      encoded-reply gate, typed on every path), clone-ceiling revisit
      trigger (p99 over 2 ms on a 1 MB project), break-it suites
      (`canary-collab/tests/break_it.rs`,
      `canary-runtime/tests/collab_break.rs`)

**Explicitly not in `v0.0.16`**: peer-to-peer authority, CRDT merge,
offline operation queues, generic component patches, prefab-graph rewrites,
editor UI, presence/cursors, locks, hosted identity, unbounded history,
history rewriting, gameplay `World` replication — see the exclusions in
`live-collaboration.md`.

Full scope: [`docs/release-notes/v0.0.16.md`](../release-notes/v0.0.16.md).

## Full architecture-to-implementation map

Every documented subsystem, and where it actually stands. "Documented"
means a real design exists in `docs/architecture/`; "Implemented" is
about working code in `engine/`.

| Subsystem | Documented | Implemented | Notes |
|---|---|---|---|
| Repository/governance | ✅ | ✅ | `v0.0.1` |
| Engine core (`canary-core`) | ✅ | ✅ | `v0.0.1` |
| Consumer runtime composition | ✅ | ✅ | `canary-runtime` library owns the active `World`, `RunContext`, scoped Tier A `on_load`/`on_unload` calls (commit `e256a61`; R-34 mitigated), and the `.13` frame driver: `Runtime::drive_frame` owns event pump → UI routing → tick → schedule → extract, with the headless binary migrated onto it. Tick/`sim_time` advance only on simulation passes (`begin_sim_pass`; R-38 mitigated). ADR 0024 items 1–5 Accepted; item 6 (lifecycle failure semantics) stays Proposed; R-36 open for future pause/reload/replacement semantics |
| Platform abstraction | ✅ | ✅ | Traits + headless + real `winit` backend (behind the `winit-backend` feature, off by default); `v0.0.4` |
| Input and simulation boundary | ✅ | ✅ | Platform normalizes keyboard transitions plus pointer position/buttons, pointer-leave, and focus-loss; `canary-input` maps those to named actions and produces frame-tagged `SimulationInput` (one local player, digital only). UI-first capture routing with in-mapper per-binding pass-through and intent delivery at the next simulation boundary are implemented (`drive_input_frame`); see [`input-and-simulation.md`](../architecture/input-and-simulation.md) and ADR 0025 (Accepted) |
| ECS | ✅ | ✅ | Archetype-based, cached queries, change detection; `v0.0.2`. Multi-component queries, typed resources, `Tick(u64)`; `v0.0.7` |
| Scheduler (`canary-scheduler`) | ✅ | ✅ | `SystemAccess` + stage-based `Schedule`; real concurrent read-only stages, writes always solo (concurrent disjoint writes still open); `v0.0.8` |
| Transform + hierarchy (`canary-transform`) | ✅ | ✅ | Single always-3D `Transform` (ADR 0017), `GlobalTransform` propagation via `canary-scheduler`; implemented; tagged `v0.0.9` (backfill 2026-09-28) |
| Plugin system — Tier B (native) | ✅ | ✅ | Versioned ABI (ADR 0009), `v0.0.1` |
| Plugin system — Tier A (WASM) | ✅ | ✅ | Component loading, structural capability enforcement, resource budget, ECS data ABI; `v0.0.3`. Active-World scoped lifecycle access is implemented for `on_load`/`on_unload` (R-34 mitigated); no per-frame hook |
| Rendering | ✅ | ✅ | RHI trait + native Vulkan backend; ECS-driven CPU bake and file-loaded mesh/texture sampling are proven. Window presentation with live lifecycle gates (steady/minimize/restore/resize→recreate/content/destruction in `present_clear.rs`; capability-rejection and fatal-error mappings unit-gated) and same-RHI scene/UI window rendering (`ui-game`, 600/600 presented) are implemented on `dev`. Depth/general materials remain open |
| Localization (`canary-loc`) | ✅ | ✅ | ADR 0015 (Accepted); `.ftl`/Fluent, `LocKey` type; `v0.0.5` |
| Physics | ✅ | ✅ (2D slice) | `PhysicsBackend` trait + private rapier2d 0.35.3 backend, fixed-step system with spiral guard, first-position registration, game-plus-pixel proof; determinism is single-machine repeatability; `v0.0.11`. 3D (Jolt canonical, Rapier3D alternative) still direction, post-`v0.1.0` |
| Networking | ✅ | ✅ (`.15` slice) | First profile Accepted in ADR 0027; `canary-net` (transport, replication, tombstone log, session, resync, loopback + separate-process proof) shipped as tag `v0.0.15`. See the `v0.0.15` section above |
| Scripting system | ✅ | ❌ | Depends on Tier A |
| Asset system | ✅ | ✅ (minimal) | `AssetId`/`AssetHandle<T>`/`AssetStore<T>` with sync GLB/PNG/WAV/Vorbis loaders; `AssetId` is provisional content identity. Stable `LogicalAssetId`, cooking, cache, hot reload, importers-as-plugins, and broader formats remain future work; `v0.0.10`/`.12` |
| Audio | ✅ | ✅ (bootstrap) | `AudioBackend` trait + private rodio 0.22.2 backend (decode-only MIT/Apache features, headless default), `AudioSource`/`AudioListener` + trigger system using propagated `GlobalTransform` poses; host must insert a device backend for audible output. Custom engine stays the long-term default per ADR 0023; `v0.0.12` |
| `CanaryUI` (UI toolkit) | ✅ | ✅ (first-game slice) | ADR 0011 plus first-game contract in [`ui-toolkit.md`](../architecture/ui-toolkit.md); `canary-ui-core` (backend-neutral traits) + `canary-ui-egui` (egui 0.36 adapter, tessellate → RHI soup + scissor) implemented on `dev`, with same-window, same-RHI game HUD and shared input capture proven by `ui-game`. Full theming/layout/shaping remain future work |
| Project state & versioning (`canary-state`) | ✅ | ✅ (`.14` slice) | Product boundary and migration/snapshot rules in [`state-and-versioning.md`](../architecture/state-and-versioning.md) and ADR 0026 (Accepted); `canary-state` implements canonical-JSON project files (deterministic order, unknown preservation, one-level prefabs, change log, atomic save/staged load), linear migration chains with a `migrate_fields` bridge, postcard snapshots with SHA-256 checksums, and dep-free prefab bake (`SpawnPlan::from_document`); `canary-runtime` composes staged spawn (`AuthoredSpawner`) and the simulation boundary (`SnapshotRegistry` capture/checksum/restore, `Simulation::step`, `SimClock`, RNG/clock via the reserved sim-core record with no wire break). Presentation exclusion, migration/unknown fixtures, and interrupted-write recovery are proven; released as tag `v0.0.14` |
| Live collaboration | ✅ | ✅ (`.16` slice) | ADR 0028 (Accepted) + [`live-collaboration.md`](../architecture/live-collaboration.md); `canary-collab` (op, actor, permissions, wire, session) composed in `canary-runtime` (`CollabSessionHost`) over `canary-net` framing; two-client + restart/reconnect proof; cut as tag `v0.0.16`. See the `v0.0.16` section above |
| Editor | ⚠️ Partial (vision-level) | ❌ | Post-`v0.1.0`; build on the validated consumer runtime, project-state formats, `CanaryUI`, plugin lifecycle, and windowed rendering. See [`future-roadmap.md`](future-roadmap.md) |
| CLI/headless operation (editor) | ✅ (principle recorded) | N/A yet | No editor exists to apply it to; proven in spirit by `canary-runtime`/`xtask` today |
| 2D/3D & non-game applicability | ✅ (vision + physics + rendering) | N/A | Positioning + architectural constraint, not a standalone feature |

Legend: ✅ done/exists · ⚠️ partial · ❌ not started · N/A not
applicable as a binary done/not-done item.
