# Editor Design (Target — Not Yet Built)

No editor exists in this foundation, and none is planned before Era 5 (see
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md)) — it
depends on the plugin system (Era 2+) and rendering (Era 3) both existing
first. This document describes the target design so that when editor work
starts, it starts from a reasoned plan rather than an improvised one.

## The editor is a plugin host, not a special case

The central design commitment: **the editor is built using Canary's own
plugin system** ([`docs/architecture/plugin-system.md`](../architecture/plugin-system.md)),
the same one third-party mods and tools use. Editor panels (scene
hierarchy, inspector, asset browser, console) are plugins. This is a
deliberate dogfooding decision, not an aesthetic one: if the plugin API
isn't expressive enough to build a scene hierarchy panel, that's a real
limitation the project needs to discover from its own tooling, not from a
frustrated third-party plugin author months later.

Practical consequence: a third-party contributor who wants a custom
inspector for their own component types, or a completely different panel
layout for their studio's workflow, uses the *exact same extension
mechanism* the built-in editor panels use — not a separate, more limited
"editor scripting" API bolted on afterward.

## Professional workflows and beginner accessibility, together

These are usually presented as a tradeoff (a minimal, guided UI for
beginners vs. a dense, powerful one for professionals). Canary's answer is
the same one used for the engine's own modularity: a workspace/panel model
where the *default* layout is simple and guided, and professional-depth
panels (a full material graph editor, a detailed profiler view) are
available but not forced into view for someone who doesn't need them yet.
Because panels are plugins, a "beginner mode" and a "pro mode" are, in
principle, different default panel layouts and default-visible panel sets —
not two different editors to build and maintain.

## Responsive design

The editor should not assume a single fixed window/monitor configuration.
Panel docking should support multi-monitor layouts (common for professional
studios — game view on one monitor, inspector/hierarchy on another) and
should degrade gracefully on smaller single-monitor setups (common for
hobbyists/students) without the tool becoming unusable at a modest
resolution. This is a target requirement, not a specific implementation
plan yet — it will directly shape the UI toolkit decision below, once
that decision is made.

## Customization and plugin-created panels

Beyond built-in panels, third-party plugins can register their own panels
into the same docking/workspace system — a custom level-design tool, a
dialogue-tree editor, a studio-specific asset validator, all appearing and
docking like any built-in panel. This requires the panel/docking system
itself to be designed as a public API from the start, not refactored into
one after the fact once built-in panels already assume undocumented
internal access.

## Visual scripting

The editor is the natural home for a future visual scripting graph editor
(node-based logic authoring). Per
[`docs/architecture/scripting-system.md`](../architecture/scripting-system.md#designer-facing-ergonomics-vs-systems-programmer-ergonomics),
this is explicitly designed to compile to the *same* WASM component
interface as textual scripting languages — the editor's job is to be a
graph-authoring UI over that shared target, not a separate execution engine
to maintain in parallel.

## Collaboration tools

Real-time multi-user editing (Figma-style concurrent scene editing) is
aspirational and explicitly not scoped for any near-term era. This
section originally recorded one near-term implication in passing (prefer
diffable, mergeable scene/asset formats); that idea has since grown into
a full architectural principle and design of its own — see
[`docs/architecture/state-and-versioning.md`](../architecture/state-and-versioning.md)
and [ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md),
which supersede this section for anything beyond "the editor should keep
this in mind."

## UI toolkit: decided at the architecture level, not yet built

Originally left open in this document pending real editor-building
experience. Since resolved, at the architecture level, as
[ADR 0011](../decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md):
a `CanaryUI` abstraction (`canary-ui-core`) exists from the start, with
`egui` — immediate-mode, pure Rust, already integrating naturally with
`wgpu` per [ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md) —
as the first concrete backend, shared between the editor and game-facing
UI rather than built twice. See
[`docs/architecture/ui-toolkit.md`](../architecture/ui-toolkit.md) for the
full design.

What remains genuinely open, and *is* deferred until there's real
editor-building experience to inform it: whether a fully custom,
`canary-render`-backed retained-mode toolkit ever replaces the `egui`
backend, and if so when. `ADR 0011` deliberately doesn't answer that —
only that the abstraction exists now so the question can be answered
later without a rewrite of every panel built in the meantime.
