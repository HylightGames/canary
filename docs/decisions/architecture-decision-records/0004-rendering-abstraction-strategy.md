# 0004. Rendering Hardware Interface: custom trait, bootstrapped on wgpu

**Status:** Accepted. The RHI/render-graph split below is unchanged.
The specific backend-implementation choice ("bootstrap on `wgpu`, defer
a custom RHI") is superseded by
[ADR 0016](0016-native-rendering-backends.md), which reverses that one
sub-decision — see ADR 0016 for why and what's unchanged.

## Context

Rendering needs to run across Vulkan (Linux/Windows/Android), Metal
(macOS/iOS), and DirectX 12 (Windows), per the "cross-platform by design"
goal, while remaining a replaceable subsystem per "replaceable engine
subsystems." Writing and maintaining native backends for three-plus
graphics APIs is a very large, ongoing engineering investment — one this
foundation explicitly should not spend v0.0.1-era effort on, per the
instruction to build the foundation, not the whole engine.

## Decision

Rendering is architected as **two layers** (full detail in
[`docs/architecture/rendering.md`](../../architecture/rendering.md)): a thin
Render Hardware Interface (RHI) trait that is the *only* layer aware of the
concrete graphics API, and a render graph / high-level renderer that depends
only on the RHI trait.

The **initial RHI implementation is backed by `wgpu`** — a mature, actively
maintained, pure-Rust graphics library already targeting Vulkan, Metal,
DirectX 12, and OpenGL natively (plus WebGPU/WebGL2 on WASM), and already
proven in production by Firefox, Servo, Deno, and multiple Rust game engines
(Bevy, Fyrox, rend3).

The RHI trait boundary is deliberately designed so a **fully custom RHI can
replace the wgpu-backed one later** without touching the render graph or any
gameplay-facing rendering code — this decision commits to bootstrapping
pragmatically, not to `wgpu` as a permanent, unquestionable dependency.

## Alternatives considered

**Write a custom Vulkan-first RHI immediately, skip wgpu.** Rejected for
this foundation: this is exactly the "huge amount of code, prematurely"
this project is explicitly trying to avoid in its first milestone.
Reimplementing what `wgpu` already does well, before there's a render graph
or any content to justify needing capabilities `wgpu` lacks (multi-GPU,
hardware ray-tracing extensions, console APIs), would be effort spent
without evidence it's needed. This remains the eventual target for
performance-critical, AAA-grade rendering and is explicitly not ruled out —
it's sequenced for when the render graph above it actually needs what it
offers.

**Adopt an existing full engine's renderer (e.g., embed Bevy's render
crates) rather than build an RHI at all.** Rejected: this would mean
Canary's rendering architecture is, transitively, Bevy's rendering
architecture, which cuts directly against "analyze what existing engines do
well and badly, make decisions from first principles" — the founding
mandate for this project. Depending on `wgpu` (a graphics API abstraction)
is a different kind of dependency than depending on another engine's
renderer (an opinionated, higher-level design) — the former leaves Canary's
render graph, materials, and RHI trait design fully its own.

**Support only one graphics API initially (e.g., Vulkan-only via
`ash`/direct bindings), defer cross-platform to later.** Rejected: "cross-
platform by design" is a stated, non-negotiable pillar, and `wgpu` delivers
genuine cross-platform coverage today at effectively no extra integration
cost relative to a single-API binding — there's no real savings from
narrowing scope here.

## Consequences

- The renderer (once built, in Era 3) inherits `wgpu`'s current capability
  gaps: no multi-GPU support, limited hardware ray-tracing exposure, and no
  console NDA'd API backends. These are accepted for now and tracked as
  reasons a custom RHI implementation might eventually be justified — see
  [`docs/research/technology-evaluations.md`](../../research/technology-evaluations.md).
- Materials/shaders default to WGSL (what `wgpu`/`naga` natively understand),
  avoiding a second shading language to maintain, per
  [`docs/architecture/rendering.md`](../../architecture/rendering.md#materials--shaders).
- Because the render graph depends only on the RHI trait, a future decision
  to build a custom RHI is a new crate implementing that trait, not a
  rewrite of the render graph, materials system, or any gameplay-facing
  rendering API — this is the concrete payoff of layering the decision this
  way rather than depending on `wgpu` directly throughout.
