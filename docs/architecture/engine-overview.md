# Engine Architecture Overview

This document is the map. Each subsystem below has its own detailed document;
this one exists so a new contributor (or a future development session) can
see how the pieces fit before diving into any one of them.

## Layering

Canary is organized into four layers, each depending only on the layer below
it:

```mermaid
graph TD
    subgraph L4["Layer 4 — Content & Tooling"]
        EDITOR["Editor (plugin host)"]
        GAME["Game / Application code"]
        MODS["Community plugins & mods (WASM)"]
    end
    subgraph L3["Layer 3 — Engine Subsystems (replaceable)"]
        REND["Rendering (RHI + render graph)"]
        PHYS["Physics"]
        NET["Networking / Replication"]
        ASSET["Asset pipeline"]
        SCRIPT["Scripting / Plugin runtime"]
    end
    subgraph L2["Layer 2 — Core Runtime"]
        ECS["ECS (World, entities, components, schedule)"]
        JOBS["Job system / scheduler"]
        PLUGINAPI["Plugin trait & loader (native + WASM)"]
        LOG["Logging & diagnostics"]
    end
    subgraph L1["Layer 1 — Platform Abstraction"]
        PLAT["Windowing, input, filesystem, threads, time"]
    end

    L4 --> L3 --> L2 --> L1
```

The rule that keeps this from rotting: **nothing in a lower layer may depend
on a higher layer.** The ECS does not know that a renderer exists; the
platform layer does not know that an ECS exists. Subsystems in Layer 3 talk
to each other, when they must, through the ECS (shared components/resources)
or through explicit, documented interfaces — never through ad hoc globals.

## Subsystem map

| Subsystem | Crate (current or planned) | Document |
|---|---|---|
| Platform abstraction | `canary-platform` | [platform-abstraction.md](platform-abstraction.md) |
| Core runtime (App, logging, error conventions) | `canary-core` | [core-runtime.md](core-runtime.md) |
| ECS | `canary-ecs` | [core-runtime.md](core-runtime.md) |
| Plugin trait & loader | `canary-plugin-api` | [plugin-system.md](plugin-system.md) |
| Scripting / language-agnostic runtime | *(planned: `canary-script`)* | [scripting-system.md](scripting-system.md) |
| Rendering | *(planned: `canary-render`)* | [rendering.md](rendering.md) |
| Physics | *(planned: `canary-physics`)* | [physics.md](physics.md) |
| Networking | *(planned: `canary-net`)* | [networking.md](networking.md) |
| Asset pipeline | *(planned: `canary-assets`)* | [asset-system.md](asset-system.md) |

"Planned" crates are architected in this document set but not implemented in
this foundation — see [`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md)
for exactly what exists today versus what's designed-but-not-built.

## The two structural bets this engine makes

Everything above follows fairly conventional data-oriented engine design.
Two decisions are where Canary actually differs from precedent, and both are
load-bearing enough that the rest of the architecture assumes them:

1. **A two-tier plugin/extension model** — a sandboxed, language-agnostic
   WebAssembly Component tier for mods and marketplace content, and a
   trusted, native C-ABI tier for performance-critical subsystem
   replacement. See [plugin-system.md](plugin-system.md) and
   [ADR 0003](../decisions/architecture-decision-records/0003-plugin-and-modding-architecture.md).
2. **Everything replaceable is a trait, not a `#[cfg]` flag.** Rendering,
   physics, and asset importers are defined as interfaces in Layer 3 with a
   default implementation, so a different implementation is a new crate, not
   a fork. See [ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md)
   for the concrete example (RHI vs. wgpu).

## Threading model, in one paragraph

Canary assumes a job-stealing thread pool, not "one thread per subsystem."
The ECS scheduler (see [core-runtime.md](core-runtime.md)) analyzes system
data-access declarations to build a dependency graph, then hands runnable
systems to the job pool; independent systems (e.g. "AI planning for enemies"
and "particle simulation") run concurrently without either subsystem's code
containing a single explicit thread spawn. The v0.0.1 ECS placeholder does
**not** implement this yet — it runs systems sequentially — but the public
API is shaped so parallelization is additive later, not a breaking rewrite.
See [ADR discussion in core-runtime.md](core-runtime.md#threading--the-job-system).

## How a frame is expected to flow (target design, post-Era 2)

```mermaid
sequenceDiagram
    participant Platform as Platform layer
    participant ECS as ECS scheduler
    participant Game as Game/gameplay systems
    participant Phys as Physics
    participant Net as Networking
    participant Render as Renderer

    Platform->>ECS: Pump input/window events
    ECS->>Game: Run gameplay systems (parallel where possible)
    ECS->>Phys: Run physics step (fixed timestep)
    ECS->>Net: Collect replicated component deltas
    Net-->>ECS: Apply incoming remote state
    ECS->>Render: Extract render-relevant state (read-only snapshot)
    Render->>Platform: Submit frame to RHI / present
```

The "extract" step (ECS → Renderer) is deliberately a read-only snapshot
rather than the renderer querying live ECS state mid-frame — this is the same
pattern used by several modern data-oriented engines (see
[`docs/research/engine-comparisons.md`](../research/engine-comparisons.md))
and it's what allows rendering to run one frame behind simulation on a
separate thread later, without a redesign.

## What this document deliberately doesn't cover

Build tooling lives in [`docs/development/build-system.md`](../development/build-system.md);
repository layout in [`docs/development/repository-structure.md`](../development/repository-structure.md);
editor UI in [`docs/ui/`](../ui/). This document is the *engine* map, not the
project map.
