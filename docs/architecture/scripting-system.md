# Scripting System

This document covers gameplay-facing scripting specifically — the
day-to-day loop of "write logic, see it run, change it, see the change"
that a designer or gameplay programmer lives in. It builds on
[plugin-system.md](plugin-system.md); scripting is, architecturally, a
particular *use* of the Tier A (sandboxed, language-agnostic) plugin runtime,
not a separate mechanism.

## Why scripting and plugins share one runtime

Many engines maintain a separate "scripting language" (GDScript, Blueprint,
a custom Lua binding) distinct from their "native extension" mechanism, with
different capabilities, different debugging tools, and different
performance characteristics. Canary deliberately avoids that split: gameplay
scripts *are* Tier A WASM components. The benefits:

- One security/capability model to reason about, not two.
- One debugging/profiling story to build tooling for.
- A script and a "real" plugin can be promoted into each other without a
  rewrite — a prototype gameplay script that turns out to be
  performance-critical is already a WASM component; it just needs
  optimization, not a port to a different system.
- Any language with a Component Model toolchain is a scripting language for
  Canary "for free," which is what actually delivers "language-agnostic
  development" at the gameplay layer, not just at the engine-extension
  layer.

## Hot reload

Fast iteration is non-negotiable for gameplay scripting — nobody should
restart the game to see a tuning change. Target design:

- A script component is recompiled and the running engine swaps the old
  component instance for the new one at a defined safe point (end of frame),
  re-hydrating its declared state from a serialization contract the
  component defines.
- Because Tier A components are sandboxed with explicit capabilities, hot
  reload doesn't risk leaking native resources the way reloading a native
  DLL can (dangling pointers into an unloaded module) — the WASM instance is
  simply discarded and a new one instantiated.
- This is explicitly **not implemented** in v0.0.1; it depends on the Tier A
  loader existing first (see [plugin-system.md](plugin-system.md#the-plugin-trait-surface-canary-plugin-api)).

## Designer-facing ergonomics vs. systems-programmer ergonomics

Not every author of gameplay logic wants to write Rust or C. Canary's answer
is layered rather than picking one audience:

1. **Textual languages compiling to WASM** (Rust, C, C++, Zig, TinyGo, and
   others) for programmers who want a real language with real tooling.
2. **A future visual scripting graph** (node graphs, in the spirit of
   Blueprint or Godot's visual scripting) that compiles down to the *same*
   WASM component interface as the textual languages above — not a
   separate, parallel execution model. A visual graph and a Rust script that
   implement the same behavior should produce interchangeable components.
   This is deliberately deferred past v0.0.1 (see
   [`docs/roadmap/future-roadmap.md`](../roadmap/future-roadmap.md)) — a
   visual scripting UI is an editor feature, and the editor itself is a
   later era (see [`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md)).

Unifying both under one compiled target means engine-side systems (hot
reload, capability review, profiling) are built once and serve both
audiences, instead of the visual scripting layer becoming a second-class,
separately maintained execution path — a common failure mode called out in
[`docs/research/engine-comparisons.md`](../research/engine-comparisons.md).

## Performance expectations

WASM components run ahead-of-time compiled to native code (see
[plugin-system.md](plugin-system.md#tier-a--sandboxed-language-agnostic-webassembly-components)),
so steady-state execution is close to native speed. The realistic overhead
lives at the **host/component boundary** — crossing from engine (native)
code into a component call, and back, has a small, measurable cost. The
architectural implication: hot loops that call into scripts *very*
frequently with tiny amounts of work per call (e.g., a script callback
invoked per-entity, per-frame, for thousands of entities) should be designed
as **batched** calls (hand the script a slice of entities to process in one
call) rather than one call per entity. This is a data-oriented,
ECS-friendly pattern already, not an unusual constraint to design around.

## Data access from scripts

Scripts don't get raw access to the ECS `World`. They declare, via their
component interface (WIT), which component types they read and write; the
host validates that declaration against the capabilities it was granted and
exposes only that data across the boundary. This keeps the same "systems
declare their data access" discipline used by native ECS systems (see
[core-runtime.md](core-runtime.md#ecs-architecture)) consistent all the way
out to sandboxed scripts, rather than scripts being a special, less
disciplined case.

## Status in this foundation

Nothing in this document is implemented in v0.0.1-pre1. It depends entirely
on the Tier A plugin loader (WASM/Wasmtime integration), which is itself
explicitly deferred past this session — see
[plugin-system.md](plugin-system.md#the-plugin-trait-surface-canary-plugin-api)
and [`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md). This
document exists so that when that work starts, it starts from an already-
reasoned design rather than being figured out from scratch under
implementation pressure.
