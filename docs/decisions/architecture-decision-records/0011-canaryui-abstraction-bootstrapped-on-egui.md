# 0011. `CanaryUI`: a UI abstraction from day one, bootstrapped on `egui`

**Status:** Accepted (architecture only — no code in `v0.0.1`; see
[`docs/architecture/ui-toolkit.md`](../../architecture/ui-toolkit.md) for
the full design)

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
  is `v0.0.2`+ scope at the earliest, gated on the editor's own work
  starting (Era 5) — not part of `v0.0.1`.
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
