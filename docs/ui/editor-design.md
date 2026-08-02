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
aspirational and explicitly not scoped for any near-term era — it's
mentioned here because it was part of the founding brief's UI/UX
considerations, and because it has one concrete near-term implication worth
recording now: scene/asset data formats should prefer being diffable and
mergeable (structured text where practical, rather than opaque binary
blobs) specifically so that *even without* real-time collaborative editing,
version-control-based collaboration (multiple people editing scenes on
branches, then merging) stays tractable. This is a preference to keep in
mind when the asset/scene format is actually designed
([`docs/architecture/asset-system.md`](../architecture/asset-system.md)),
not a commitment being made in this document.

## UI toolkit: an open question, not a decision

Unlike the engine subsystems in `docs/architecture/`, this document
deliberately does **not** pick a UI toolkit yet. Two real options surfaced
during research for this foundation:

- **`egui`** (immediate-mode, pure Rust, already integrates naturally with
  `wgpu`) — fast to prototype with, well-suited to the tool-panel-heavy
  style common in game engine editors, but immediate-mode GUIs generally
  trade away some of the polish/customization ceiling of a retained-mode
  UI.
- **A custom retained-mode UI**, giving full control over styling,
  animation, and accessibility, at significantly higher implementation
  cost.

A reasonable path — bootstrap tooling and internal dev UIs with `egui`
early, while treating the shipping editor's final UI toolkit as a decision
made once there's actual editor-building experience to inform it — is
plausible but is **not** being locked in here as an ADR, specifically
because there's no editor-building experience yet to test that assumption
against. This will become an ADR once Era 5 actually starts.
