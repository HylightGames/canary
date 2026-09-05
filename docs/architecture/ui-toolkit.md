# UI Toolkit (`CanaryUI`)

Formalizes a decision made while completing `v0.0.1`: **`CanaryUI` exists
as an abstraction from the start, with `egui` as its first backend** —
the same "bootstrap pragmatically, architect for replacement" pattern
already used for physics ([`physics.md`](physics.md), Rapier) and,
previously, rendering (rendering's own bootstrap choice has since moved
past this pattern — see
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md)).
See
[ADR 0011](../decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md)
for the decision record; this document is the fuller design. Nothing
here is implemented in `v0.0.1` — this is architecture for `v0.0.2`+,
written now for the same reason every other subsystem doc in this
project is written ahead of its code.

## The mistake this is designed to avoid

An ambitious engine project that decides to build a complete custom UI
toolkit before it has an engine is a well-known failure pattern: text
shaping, IME support, accessibility, a layout engine, docking, and GPU
rendering for a UI toolkit is, on its own, a multi-year effort — browsers
employ thousands of engineers on exactly this problem. A project that
tries to ship a renderer, physics, a UI framework, a scripting language,
an editor, and networking all before anything is usable ends up, years
later, with none of them finished. Canary's plugin system, ECS, and
rendering strategy all avoid this by depending on a mature existing
library and keeping a replacement path open; `CanaryUI` does the same.

## Two different things, deliberately kept separate

- **`CanaryUI` (the API and architecture)** — starts now, as an
  abstraction. The editor, and eventually games built with Canary, code
  against `canary_ui::Window`, `canary_ui::Button`, and so on — never
  directly against `egui::Window`. This is a small, stable surface:
  widget traits, an event model, layout abstractions, and a theming
  interface, not a full implementation.
- **`CanaryUI`'s implementation (a real UI toolkit)** — text layout, font
  shaping, IME, accessibility, a flex/grid layout engine, widgets,
  animation, focus management, drag-and-drop, docking, GPU rendering —
  is the actual multi-year effort described above, and is explicitly
  **not** undertaken now. `egui` provides all of this as the first
  backend.

```
canary-ui-core            <- the trait/API layer (starts now)
      |
canary-ui-egui            <- the concrete backend (starts when the editor does, v0.0.2+)
      |
    egui
```

A later, fully custom backend replaces only the bottom of this stack:

```
canary-ui-core
      |
canary-ui-native          <- a future custom backend
      |
canary-render (this project's own RHI, per rendering.md)
```

Nothing above `canary-ui-core` — the editor, or a game's HUD/menu code —
needs to change when that swap happens, which is the entire point of
introducing the abstraction now rather than depending on `egui` directly
and hoping a later migration goes cleanly.

## Editor UI and game UI share one technology

This is the part of the design most worth calling out explicitly, because
it's a real differentiator, not just tidiness: **`CanaryUI` is not an
editor-only concern.** The same abstraction that renders an inspector
panel is intended to render a game's inventory screen, HUD, dialogue
system, and menus. Concretely:

```
                CanaryUI (canary-ui-core)
                       |
        --------------------------------
        |                              |
    Editor UI                      Game UI
   (panels, inspector,          (HUDs, menus,
    hierarchy, console)          inventory, dialogue)
        |                              |
        --------------------------------
                       |
                 UI backend (egui today)
```

Practically, this means a game developer building a HUD in Canary is
using the *exact same* widget/layout/theming system a Canary contributor
uses to build an editor panel — one thing to learn, one thing to
document, one thing to optimize, rather than two parallel UI stacks that
happen to share a project name. It also means the editor's own panels are
a real, continuously-exercised stress test of whether `CanaryUI` is good
enough for actual game UI, the same dogfooding argument already made for
the plugin system in [`plugin-system.md`](plugin-system.md#editor-as-a-plugin-host).

## Replaceable, including by third parties

Consistent with "everything replaceable is a trait, not a fork" (see
[`engine-overview.md`](engine-overview.md#the-two-structural-bets-this-engine-makes)),
a `CanaryUI` backend is just a crate satisfying the `canary-ui-core`
traits. The `egui`-backed implementation is the default, not a
privileged special case — a community `canary-ui-native` (a custom
GPU-rendered toolkit), a hypothetical `canary-ui-mobile` (touch-optimized
variant), or a studio-specific backend are all the same kind of thing
architecturally, exactly like an alternative `PhysicsBackend` or RHI
implementation.

## Why `egui` specifically as the first backend

`egui` is a mature, actively developed, pure-Rust immediate-mode GUI
library — a strong first-backend choice on its own merits (multi-year
production track record, permissive licensing, an existing renderer-
agnostic output format: `egui`'s draw output is triangle meshes and
textures, not a GPU-API-specific call, so it doesn't require whichever
RHI backend Canary is using to natively integrate with `egui` itself).
See
[`docs/research/technology-evaluations.md`](../research/technology-evaluations.md#editor-ui-toolkit-evaluated-first-backend-chosen)
for the sourcing. Immediate-mode is a deliberate fit for the tool-panel-
heavy style of an engine editor (inspectors, hierarchies, consoles are
exactly the kind of UI immediate-mode libraries handle well), at the cost
of some of the animation/styling ceiling a retained-mode toolkit would
offer — an accepted tradeoff for a first backend, not a permanent one.

Concretely, `canary-ui-egui` renders `egui`'s output (clipped triangle
meshes + textures) through `canary-render`'s RHI trait directly, not
through the community's `egui-wgpu` integration crate — per
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md),
Canary's own rendering no longer bootstraps on `wgpu`, so `egui`'s
existing `wgpu`-specific integration crate isn't the path here. This is
a small, well-understood integration surface either way (rasterize
textured triangles with a scissor rect per mesh) — not a reason this
choice was made, just a detail worth being accurate about now that the
rendering plan has changed since this document was first written.

## 100% native — no embedded web view

Recorded explicitly since it's a real, sometimes-implicit alternative
other tools take: Canary's editor and game UI are native, compiled Rust
UI, end to end — never an embedded web view (Electron/CEF-style). This
is consistent with, not incidental to, this project's broader
"native compilation, cross-platform by design, AAA-capable" goals
([`docs/vision/project-goals.md`](../vision/project-goals.md)): a web
view is a heavyweight, separate rendering/runtime stack with its own
performance and packaging costs that a native-first engine has no reason
to accept.

## Status in this foundation

Entirely architectural — no `canary-ui-core` crate exists yet. Depends on
the editor's own work starting (Era 5, per
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md)),
which is itself gated on rendering and the plugin system, per
[`docs/roadmap/future-roadmap.md`](../roadmap/future-roadmap.md). See
[`docs/ui/editor-design.md`](../ui/editor-design.md), which this document
supersedes for the specific "which toolkit" question that doc had left
open.
