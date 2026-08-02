# UX Principles

These are the principles the future editor (and, where relevant, in-engine
developer-facing tools like log/profiler output) should be judged against.
Like [`editor-design.md`](editor-design.md), this is target guidance for
work that hasn't started yet.

## Errors should teach, not just report

An error message's job is to get the user to a fix, not just to describe
the failure. Where practical, engine and tooling error messages should
state: what went wrong, why (if knowable), and what to do next — the same
standard Rust's own compiler diagnostics are well-regarded for. This applies
as much to a failed plugin capability grant as to a missing asset file.

## Defaults should be good enough to never touch

A new project's default panel layout, default project settings, and
default build configuration should be usable without immediately requiring
customization. Customization should exist for when someone *wants* to
change something, not to compensate for a bad default. This is the same
philosophy as the engine's modularity: power is opt-in, not a prerequisite
for getting started.

## Consistency over novelty in interaction patterns

Where an interaction pattern already has a strong, familiar convention
(drag-and-drop for docking panels, standard keyboard shortcuts for
undo/redo/save, a properties inspector that looks like a properties
inspector), the editor should follow it rather than inventing a novel
pattern for its own sake. Novelty should be reserved for places where
Canary is solving a problem existing tools don't have a convention for yet
(the plugin-created-panel model, for instance) — not applied uniformly as a
stylistic choice.

## Progressive disclosure

Complexity should be revealed as it's needed, not presented all at once.
A beginner opening the editor for the first time and a studio technical
artist configuring a complex material graph are both real users, and
neither should be the one the default experience is designed against — see
[`editor-design.md`](editor-design.md#professional-workflows-and-beginner-accessibility-together)
for how the panel/plugin model is intended to make this concrete rather
than aspirational.

## Feedback should be immediate wherever technically possible

Hot reload for scripts and assets
([`docs/architecture/scripting-system.md`](../architecture/scripting-system.md#hot-reload),
[`docs/architecture/asset-system.md`](../architecture/asset-system.md#hot-reload))
is a UX principle as much as a technical feature: the gap between changing
something and seeing its effect is one of the largest levers on how it
*feels* to work in an engine, and is worth real engineering investment
specifically because of that.

## Accessibility is a requirement, not a stretch goal

Keyboard navigability, sufficient color contrast (including a
colorblind-safe default palette, not just a high-contrast one), and
scalable UI text are requirements for the editor's eventual UI toolkit
decision ([`editor-design.md`](editor-design.md#ui-toolkit-an-open-question-not-a-decision)),
not a nice-to-have to revisit after the fact. Baking this into the toolkit
decision criteria now costs nothing today and avoids a much harder
retrofit later.

## Collaboration and version control shouldn't fight each other

Per [`editor-design.md`](editor-design.md#collaboration-tools), even before
real-time collaborative editing exists, the editor's authored formats should
support the un-glamorous but essential UX of "two people edited the same
scene on different branches and now need to merge it" — a source of
disproportionate pain in engines whose scene formats are opaque binary
blobs. This is a UX principle expressed through a technical constraint,
which is exactly the kind of connection this document exists to make
explicit rather than leave implicit in an architecture doc where it might
get treated as a purely technical concern.
