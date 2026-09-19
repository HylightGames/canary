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
- [x] `clippy` verification — this sandbox couldn't reach `clippy` when
      `v0.0.3` was implemented; it can now (see `v0.0.4`'s own entry
      below for when and how that changed), and a workspace-wide check
      confirmed `v0.0.3`'s own code is clean

**Explicitly not in `v0.0.3`** (each is its own tracked follow-up, not
an oversight): a plugin manifest format (R-08), Tier B signing (R-09),
safe hot-unloading with full resource reclamation, and safely lending a
Tier A instance scoped access to a `World` already in use elsewhere
(R-34) — see `v0.0.3-roadmap.md` and the risk register for each.

## `v0.0.4` — Implemented, not yet tagged

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

## `v0.0.5` — Implemented, not yet tagged

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

## `v0.0.6` — Implemented, not yet tagged

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

## `v0.0.7` — Implemented, not yet tagged

Full detail: [`v0.0.7-roadmap.md`](v0.0.7-roadmap.md) and
[`docs/architecture/execution-model.md`](../architecture/execution-model.md).
Single focus: the ECS data-access architecture a scheduler needs,
decided (rather than picked from the previously-open render-graph/
physics/UI/state options) by the September 2026 external review triage
([`docs/decisions/2026-09-review-triage.md`](../decisions/2026-09-review-triage.md)) —
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

## `v0.0.8` — Implemented, not yet tagged

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

**`v0.0.9` is implemented on `dev`, not yet tagged** — all three
parts landed: real delta-time + wall-clock `App::run`;
`canary-transform` (`Transform`/`GlobalTransform`/`Parent`/
`Children` + scheduler-registered hierarchy propagation, 23 tests);
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
Still open past `v0.0.9`: the RHI upgrades the bridge deliberately
defers (push constants/uniforms, depth/culling, buffer updates,
materials past the texture-only slice, swapchain/presentation), the
camera component, and the App-level scheduler. Mesh assets and the
texture-only slice have since landed in `v0.0.10` (see below); the
rest stays open — all `v0.0.10+` scope.

## `v0.0.10` — Implemented, not yet tagged

Full detail: [`v0.0.10-roadmap.md`](v0.0.10-roadmap.md) and
[ADR 0018](../decisions/architecture-decision-records/0018-asset-handles-and-synchronous-loading.md).
Single focus: minimal asset loading — real files from disk feed the
renderer `v0.0.9` built.

- [x] New crate `canary-assets`, depending only on `canary-ecs` plus
      loading libraries (`sha2 0.10`, `gltf` without default features
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

## Full architecture-to-implementation map

Every documented subsystem, and where it actually stands. "Documented"
means a real design exists in `docs/architecture/`; "Implemented" is
about working code in `engine/`.

| Subsystem | Documented | Implemented | Notes |
|---|---|---|---|
| Repository/governance | ✅ | ✅ | `v0.0.1` |
| Engine core (`canary-core`) | ✅ | ✅ | `v0.0.1` |
| Platform abstraction | ✅ | ✅ | Traits + headless + real `winit` backend (behind the `winit-backend` feature, off by default); `v0.0.4` |
| ECS | ✅ | ✅ | Archetype-based, cached queries, change detection; `v0.0.2`. Multi-component queries, typed resources, `Tick(u64)`; `v0.0.7` |
| Scheduler (`canary-scheduler`) | ✅ | ✅ | `SystemAccess` + stage-based `Schedule`; real concurrent read-only stages, writes always solo (concurrent disjoint writes still open); `v0.0.8` |
| Transform + hierarchy (`canary-transform`) | ✅ | ✅ | Single always-3D `Transform` (ADR 0017), `GlobalTransform` propagation via `canary-scheduler`; implemented on `dev`, not yet tagged |
| Plugin system — Tier B (native) | ✅ | ✅ | Versioned ABI (ADR 0009), `v0.0.1` |
| Plugin system — Tier A (WASM) | ✅ | ✅ | Component loading, structural capability enforcement, resource budget, ECS data ABI; `v0.0.3`. Scoped-`World`-access still open (R-34) |
| Rendering | ✅ | ✅ | RHI trait + native per-API backends (ADR 0016, superseding ADR 0004's `wgpu` bootstrap); Vulkan first, hello-triangle proven; `v0.0.6`. ECS-driven rendering via the `canary-render-ecs` bridge (CPU-bake, propagation-then-bake schedule, pixel-tested, spinning-cube rewritten on it); `v0.0.9`. File-loaded meshes through the unchanged RHI plus a single-texture sampling slice (additive trait methods, UVs on `Float32x2`), spinning-cube off `box.glb`; `v0.0.10` |
| Localization (`canary-loc`) | ✅ | ✅ | ADR 0015 (Accepted); `.ftl`/Fluent, `LocKey` type; `v0.0.5` |
| Physics | ✅ | ❌ | Designed (2D+3D via Rapier); not yet scheduled |
| Networking | ✅ | ❌ | Designed (server-authoritative, QUIC); not yet scheduled |
| Scripting system | ✅ | ❌ | Depends on Tier A |
| Asset system | ✅ | ✅ | Minimal loading primitive (`AssetId`/`AssetHandle<T>`/`AssetStore<T>`/`AssetError`, sync GLB + PNG loaders, checked-in fixtures); cooking, cache, hot reload, importers-as-plugins, materials, and further formats all deferred; `v0.0.10` |
| `CanaryUI` (UI toolkit) | ✅ | ❌ | ADR 0011; abstraction layer could start independent of a backend |
| Project state & versioning (`canary-state`) | ✅ | ❌ | ADR 0012 (`Proposed` for identity/package format) |
| Live collaboration | ✅ | ❌ | ADR 0013 (`Accepted` — topology only; protocol/permissions unresolved) |
| Editor | ⚠️ Partial (vision-level) | ❌ | Era 5; blocked on plugin system + rendering + `CanaryUI` |
| CLI/headless operation (editor) | ✅ (principle recorded) | N/A yet | No editor exists to apply it to; proven in spirit by `canary-runtime`/`xtask` today |
| 2D/3D & non-game applicability | ✅ (vision + physics + rendering) | N/A | Positioning + architectural constraint, not a standalone feature |

Legend: ✅ done/exists · ⚠️ partial · ❌ not started · N/A not
applicable as a binary done/not-done item.
