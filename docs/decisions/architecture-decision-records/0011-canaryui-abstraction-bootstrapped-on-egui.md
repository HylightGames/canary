# 0011. `CanaryUI`: a UI abstraction from day one, bootstrapped on `egui`

**Status:** Accepted (architecture only — no code as of `v0.0.9`; see
[`docs/architecture/ui-toolkit.md`](../../architecture/ui-toolkit.md) for
the full design). One supporting detail below — "`wgpu` integration
precedent... directly consistent with ADR 0004's choice of `wgpu`" — is
superseded by [ADR 0016](0016-native-rendering-backends.md): rendering
no longer bootstraps on `wgpu`. The actual decision (`egui` as the first
backend) is unaffected — see `ui-toolkit.md`'s "Why `egui` specifically"
section for why, restated without that now-inaccurate detail.

**Reconsidered and reaffirmed, September 2026** (project owner asked
directly whether to build `CanaryUI` from scratch, now or before
`v0.3.0`, for "full control and backwards compatibility"): the
"Consequences" section below already answers the backwards-compatibility
half of that concern — a future native, `canary-render`-backed backend
is a new crate satisfying `canary-ui-core`'s existing traits, not a
rewrite of every panel or game's UI code that used `CanaryUI` in the
meantime, *provided* real consumers go through `canary-ui-core` and
never depend on `egui` directly (the one mistake this ADR calls out
below as the failure mode to avoid). That property was already designed
in; nothing about it needed to change.

The "build it now instead" half is rejected for the same reason the
original "build a complete custom toolkit immediately" alternative was:
a competitive UI toolkit's hard parts (text shaping, layout, hit
testing, styling) are a multi-year effort on their own, and the
project-wide `v0.1.0` bar this was weighed against explicitly accepts
"ugly/debug UI... verbose Rust APIs... limited tooling" as fine, judging
`v0.1.0` on whether a developer can build a real game, not on UI
polish or ownership — see
[`docs/roadmap/v0.1.0-plan.md`](../../roadmap/v0.1.0-plan.md). Building
a from-scratch UI system before `v0.1.0` would be exactly the kind of
feature-rush that plan's own sequencing exists to prevent, spending the
whole `v0.1.0` timeline on one subsystem's ceiling instead of on making
every subsystem exist and interoperate at all.

`v0.3.0` (this project's own "it's pleasant" milestone, not `v0.1.0`'s
"it works") is a real, standing point to revisit this — but by evidence,
not by calendar: reconsider once real friction with `egui` actually
shows up in practice (a specific styling/animation/integration
limitation Canary games or its own tooling hit), or once there's a
second real `canary-ui-core` consumer to design a native backend's trait
conformance against, the same "don't design ahead of a second real
consumer" standard [ADR 0010](0010-component-identity-across-language-boundary.md)
already applies elsewhere in this project. Not before either condition
holds, regardless of how close `v0.3.0` is on the calendar.

## Context

Canary needs UI for two audiences that are usually served by two separate
systems in other engines: the editor (panels, inspectors, tooling) and
games built with Canary (HUDs, menus, dialogue, inventory screens). A
complete, competitive UI toolkit — text shaping, IME, accessibility,
layout, docking, GPU rendering — is a multi-year effort on its own
(browsers employ thousands of engineers on exactly this problem); a
project that tries to build one from scratch before it has a usable
engine is a well-documented way for an ambitious project to never ship.

## Decision

**`CanaryUI` exists as a trait-based abstraction (`canary-ui-core`)
starting now** — before the editor exists, before any concrete backend
is built — and **the first concrete backend is `egui`**, a mature,
actively developed, pure-Rust immediate-mode GUI library with existing
`wgpu` integration precedent, directly consistent with
[ADR 0004](0004-rendering-abstraction-strategy.md)'s choice of `wgpu` as
the initial RHI backend.

`CanaryUI` is explicitly **not editor-only**: the same abstraction is
intended to serve game-facing UI (HUDs, menus, inventory, dialogue), so
Canary developers and Canary's own editor authors use one system, not
two. Canary's UI is **100% native** end to end — no embedded web view
(Electron/CEF-style) at any layer.

## Alternatives considered

**Build a complete custom UI toolkit immediately, skip an intermediate
backend.** Rejected: this is exactly the "UI toolkit becomes the project"
failure mode described above — a multi-year commitment paid before there
is an engine to justify it, when a mature, permissively-licensed
alternative (`egui`) already exists and this project's own architecture
(trait boundary + replaceable backend) makes replacing it later a bounded
cost, not a rewrite of everything built on top.

**Depend on `egui` directly, without a `canary-ui-core` abstraction
layer.** Rejected: this is the one mistake this ADR specifically exists
to avoid. Without the abstraction, "the editor uses `egui`" becomes true
of hundreds of call sites across editor panels and, eventually, game UI
code — at which point replacing `egui` later requires touching all of
them, which is precisely the kind of expensive-after-the-fact problem
this project's broader design philosophy
([`docs/vision/design-philosophy.md`](../../vision/design-philosophy.md))
exists to catch before it happens, not after.

**A retained-mode toolkit as the first backend instead of immediate-
mode.** Rejected for the *first* backend specifically: immediate-mode is
a strong fit for the tool-panel-heavy UI an engine editor mostly needs
(inspectors, hierarchies, consoles), and `egui` is the most mature
pure-Rust option in that category. A retained-mode custom backend (with
a higher styling/animation ceiling) remains the plausible long-term
target and is explicitly not ruled out — see
[`docs/architecture/ui-toolkit.md`](../../architecture/ui-toolkit.md).

**Separate UI systems for editor and game, on the premise that their
needs differ enough to justify it.** Rejected: they differ in *content*
(an inspector vs. a health bar) but not in the underlying primitives
(layout, widgets, input, theming) either needs, and one shared system is
a real, stated differentiator this project can credibly claim precisely
because most engines don't do this.

## Consequences

- `canary-ui-core` (traits, no rendering) can be designed and even
  partially built well before the editor itself starts, the same way
  `canary-plugin-api`'s trait surface preceded a WASM runtime to back it.
- The concrete `egui`-backed implementation (`canary-ui-egui` or similar)
  no longer waits on the editor (Era 5) to justify starting it — the
  September 2026 `v0.1.0` plan schedules it as `v0.0.13`
  ([`docs/roadmap/v0.1.0-plan.md`](../../roadmap/v0.1.0-plan.md)),
  specifically so game-facing UI (a HUD, a menu) is real before
  `v0.1.0`, independent of whenever editor work actually starts. This
  ADR's original sequencing assumed the editor would be `CanaryUI`'s
  first real consumer; a game is now expected to be first instead — the
  "editor and games share one system" decision above is unaffected by
  which one exercises it first.
- A future decision to build a fully custom, `canary-render`-backed UI
  toolkit is a new crate satisfying the existing `canary-ui-core` traits,
  not a rewrite of every panel or every game's UI code that used
  `CanaryUI` in the meantime — the concrete payoff of introducing the
  abstraction before the implementation, rather than after.
- This ADR does not resolve the finer details of `canary-ui-core`'s
  actual trait shapes (widget trait signatures, event model specifics) —
  those are implementation-time decisions for whenever `v0.0.2`+ work on
  this actually starts, not speculative API design made without a real
  backend to validate it against yet.

## Implementation status update (September 2026)

The original sequencing reference above predates the sharpened `v0.1.0`
integration plan. `canary-ui-core` and its `egui` backend are still not
implemented as of `v0.0.12`; the game-facing vertical slice is now planned
for `v0.0.13`, before any editor work. The widget/event/backend API should be
designed and validated against that real game consumer. The old `v0.0.2+`
wording is historical, not a current milestone assignment. See
[`docs/roadmap/v0.1.0-plan.md`](../../roadmap/v0.1.0-plan.md).
