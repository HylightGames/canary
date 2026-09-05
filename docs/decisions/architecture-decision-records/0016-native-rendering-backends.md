# 0016. Rendering backends: native per-graphics-API crates, not a `wgpu` bootstrap

**Status:** Accepted

## Context

[ADR 0004](0004-rendering-abstraction-strategy.md) split rendering into
an RHI trait and a render graph, and chose `wgpu` as the *initial* RHI
implementation specifically to avoid "a huge amount of code, prematurely"
before there was a render graph to justify a custom one — while
explicitly designing the trait boundary so "a fully custom RHI can
replace the `wgpu`-backed one later." That later point is now: direct
project direction is to build the native backends from the start, one
crate per graphics API, matching the same "no privileged built-ins"
pattern [`physics.md`](../../architecture/physics.md) already applies —
a `PhysicsBackend` trait with Rapier and Jolt as separate, equally
un-privileged crates behind it, not one bundled into the abstraction
itself.

Two real technical questions had to be answered before this was
buildable, not just directable, addressed in "Verified, not assumed"
below:

1. Does dropping `wgpu` also mean losing ADR 0004's reason for choosing
   WGSL as the material-authoring shading language — "it's what
   `wgpu`/`naga` natively understand"? If `naga` can't cross-compile WGSL
   to each native backend's expected shader representation *without*
   `wgpu` itself as a dependency, native backends would need either a
   second shading language or a hand-rolled cross-compiler, either of
   which is a real, unplanned cost.
2. Is `ash` (the Vulkan bindings this decision names as the first native
   backend) actually usable, and is a real Vulkan device even
   enumerable, in this project's actual constrained build/test
   environments (the rustc-1.75 sandbox, no real GPU hardware) — the
   same category of question [`platform-abstraction.md`](../../architecture/platform-abstraction.md)
   got wrong once before by assuming rather than testing.

## Decision

**Rendering keeps ADR 0004's two-layer split (RHI trait + render graph)
unchanged.** What changes is the RHI implementation strategy:

**Each graphics API gets its own crate implementing Canary's RHI trait
directly against that API's native bindings — no `wgpu` (or any single
abstraction library) as a required intermediary.** Concretely:
`canary-render-vulkan` (via `ash`), with `canary-render-metal`,
`canary-render-dx12`, and `canary-render-gl` (OpenGL 4.x core plus
WebGL2/GLES for older hardware and web targets) named as the same
pattern's later crates — sequencing for those is a future roadmap
decision, not fixed here. None of these is structurally privileged over
the others or over a third-party alternative: the RHI trait is the only
contract, the same guarantee `physics.md`'s `PhysicsBackend` trait
already gives ("engine and gameplay code call the trait, never the
concrete backend crate"). A backend crate's public surface must not leak
its native API's types (`ash::vk::*`, etc.) past the RHI trait boundary —
the same "backends must never expose third-party types in public APIs"
discipline this project already holds physics backends to.

**Each backend is opt-in via a Cargo feature on the top-level
`canary-render` crate; a game build only compiles and links the
backend(s) it enables.** A Windows-only game that enables only `dx12`
never pulls in `ash`, `metal`, or a GL loader — "nothing hardcoded, so
it's not in the build if unused" applies to graphics backends exactly
the way it already applies to everything else this project has decided
this way (plugins, physics, and now this).

**WGSL remains the material-authoring shading language, cross-compiled
via `naga` used standalone — not through `wgpu`.** `naga` is
architecturally a separate crate from `wgpu` (`wgpu` itself is a
consumer of `naga`, not the other way around), and this was confirmed
directly rather than assumed — see "Verified, not assumed." This
preserves ADR 0004's actual reason for choosing WGSL ("avoiding a second
engine-specific shading language to maintain") without requiring the
runtime dependency this ADR removes.

## Alternatives considered

**Keep `wgpu` as the shipped default, and additionally offer native
backends as opt-in alternatives.** This was the shape ADR 0004 left
open ("can replace... later," implying `wgpu` stays the default until
then). Rejected now on direct project direction: a `wgpu`-shaped
long-term default doesn't fit "it's just API, that way it can be
replaced for anything the dev wants" if the intended default is itself a
single third-party abstraction rather than Canary's own thin trait with
equal-footing implementations behind it. Keeping `wgpu` as *one more*
backend crate alongside the native ones remains genuinely open — nothing
in this ADR forecloses a `canary-render-wgpu` crate existing later for
whoever wants the broadest single-crate coverage — it's simply not
Canary's own first-party starting point anymore.

**Build a custom cross-compiler instead of `naga`, to fully drop the
`gfx-rs`-adjacent ecosystem.** Rejected: `naga` is a mature, independent
shader IR and cross-compiler (SPIR-V, MSL, HLSL, GLSL backends, WGSL
front end) maintained by the same team that builds `wgpu` but not
coupled to it as a runtime dependency — confirmed directly, not assumed.
Rewriting this, with zero present evidence `naga`'s translation is
insufficient for Canary's needs, is exactly the kind of premature,
unjustified effort ADR 0004 already argued against in a different
context.

**Lead with a different first native backend (Metal or DirectX 12
instead of Vulkan).** Rejected for the *first* one specifically: Vulkan
alone covers Linux, Windows, and Android — three of Canary's real target
platforms in one backend — and `ash` needed zero version pins against
this project's rustc-1.75 floor, the smoothest single verification of
any dependency checked so far. Metal is the clear second (native Apple
support, avoiding a Vulkan→Metal translation layer like MoltenVK); GL/
WebGL and DirectX 12 are lower-priority for now since Vulkan already
covers Windows adequately and DirectX's main draw is Windows-specific
tooling (PIX, etc.) rather than reach — genuinely open for later, not
fixed here, consistent with `future-roadmap.md`'s "don't assign fake
specificity" discipline applied to anything beyond the immediate next
step.

## Verified, not assumed

- **`naga` works fully standalone, without `wgpu` as a dependency,
  under this project's rustc-1.75 floor.** A scratch crate depending on
  bare `naga` (not `wgpu`) needed exactly two pins — `naga = "=22.1.0"`
  (newer majors ship a `rust-version` above 1.87) and `indexmap =
  "=2.11.4"` (the same pin already documented for Wasmtime in
  [`build-system.md`](../../development/build-system.md#the-rustc-175-sandbox-validation-floor) —
  reused here, not rediscovered). Everything else `naga` needs
  (`hashbrown`, `bit-set`, `bit-vec`) resolved on its own once those two
  were set.
- **A real functional test, not just a compile check**: one WGSL source
  (a vertex+fragment pair with an interpolated color attribute) was
  parsed, validated, and cross-compiled through `naga`'s standalone API
  to all four target representations — real SPIR-V words, real MSL,
  real HLSL, and real GLSL source, each a nonempty, structurally correct
  output. This is the concrete evidence behind "WGSL stays the one
  shading language" above, not an assumption carried over from ADR 0004's
  `wgpu`-specific context.
- **`ash` (the crate `canary-render-vulkan` will bind to) compiles
  clean under rustc 1.75 with zero pins** — `ash = "0.38"` resolved and
  built with no edition or MSRV wall encountered, the cleanest
  dependency check of this entire pass.
- **A real Vulkan device is enumerable in this sandbox, not just
  theoretically buildable against.** `mesa-vulkan-drivers` (the
  `lavapipe`/`llvmpipe` software Vulkan ICD) installed cleanly, and
  `vulkaninfo` confirmed a real `PHYSICAL_DEVICE_TYPE_CPU` device
  (`llvmpipe`, Vulkan API version 1.4.318, Mesa 25.2.8) is enumerable —
  meaning instance/device creation, buffer/pipeline work, and offscreen
  rendering can genuinely be built *and tested* here, not merely
  compiled. This extends
  [`platform-abstraction.md`](../../architecture/platform-abstraction.md)'s
  existing overturned "no real rendering testable here" assumption from
  general software rendering specifically to Vulkan.

## Consequences

- `canary-render`'s public API (the RHI trait) is the only thing engine
  and render-graph code ever call — the same guarantee `physics.md`
  already documents for `PhysicsBackend`, now stated for rendering too.
- The render-graph and materials-system work ADR 0004/`rendering.md`
  describe is unaffected by this ADR — it depends only on the RHI trait,
  which hasn't changed shape here, only which crate(s) implement it.
- `docs/architecture/rendering.md`'s "Backend choice" section is rewritten
  to match (see that document); ADR 0004's status line now points here
  for the current backend-implementation plan.
- Building even a minimal Vulkan backend (buffers, a pipeline, one
  offscreen draw) is real, substantial engineering — likely more so than
  any prior `0.0.x` release's single-focus scope. See
  [`v0.0.6-roadmap.md`](../../roadmap/v0.0.6-roadmap.md) for how this is
  bounded: an offscreen "hello triangle" proof, not the render graph,
  materials system, or a live window presentation (deliberately not
  blocked on `v0.0.4` real windowing landing first, for the same reason
  `v0.0.5` didn't wait on `CanaryUI`/`canary-assets`).
- Metal, DirectX 12, and OpenGL/WebGL backends remain real, intended,
  un-sequenced future work — tracked, not promised on a timeline, per
  `future-roadmap.md`'s existing discipline.
