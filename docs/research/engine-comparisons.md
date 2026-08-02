# Engine Comparisons

This document informs several architecture decisions and ADRs by looking at
what existing engines do well and poorly. It reflects research conducted
during this foundation's writing (mid-2026); engine landscapes move
quickly, so treat specific figures here as a snapshot, not a permanent
truth, and re-verify before relying on them for a future decision.

## Unreal Engine

**Strengths:** the industry standard for AAA visual fidelity and cinematic
production, with the deepest middleware/console-certification ecosystem of
the three major engines. Epic's 2026 "State of Unreal" announced Unreal
Engine 6, targeting persistent, internet-scale worlds and deeper AI-assisted
tooling, while acknowledging this asks a large, established C++/Blueprint
community to eventually adopt a different (Verse-based) workflow — a
concrete illustration of how disruptive a fundamental language/workflow
shift is even for an engine with Epic's resources, and part of why
Canary's own primary-language decision
([ADR 0002](../decisions/architecture-decision-records/0002-primary-language-selection.md))
was treated as a foundational, hard-to-reverse choice rather than something
to revisit casually later.

**What Canary takes from this:** Unreal's Blueprint + C++ split is, in
effect, a two-tier extensibility model already — a lesson toward Canary's
own two-tier plugin architecture
([ADR 0003](../decisions/architecture-decision-records/0003-plugin-and-modding-architecture.md)),
though Canary's tiers split on *trust/sandboxing* rather than on
*visual-vs-textual authoring*, which is a different (and, we think,
more fundamental) axis — see
[`docs/architecture/scripting-system.md`](../architecture/scripting-system.md)
for why visual scripting is designed to compile to the *same* target as
textual languages rather than being a separate tier.

## Unity

**Strengths:** the largest asset ecosystem and cross-platform/mobile reach
of the three engines, and still the most-used engine by raw developer count
in 2026 surveys, even after well-documented trust damage.

**What Canary takes from this, as a governance lesson more than a technical
one:** Unity's 2023 per-install runtime-fee announcement, and the
multi-year trust erosion that followed even after the policy was walked
back, is the clearest available case study in why Canary's founding brief
insists on a permissive, unambiguous license (MIT) with no reserved
right to change the deal later. This isn't a technical architecture lesson,
but it directly shaped how firmly [`LICENSE`](../../LICENSE) and
[`docs/vision/project-goals.md`](../vision/project-goals.md) treat licensing
terms as non-negotiable rather than as a detail to revisit once the project
has traction.

## Godot

**Strengths:** MIT-licensed, nonprofit-governed (the Godot Foundation),
free at any scale, and the fastest-growing of the three engines by several
2026 measures — including at least one high-profile commercial studio
(MegaCrit, developer of Slay the Spire) moving a project from Unity to
Godot mid-development. Godot 4.4 also added Jolt Physics as a selectable
alternative to its own default physics backend — a real-world precedent for
exactly the kind of swappable-subsystem design Canary commits to more
broadly (see
[`docs/architecture/physics.md`](../architecture/physics.md) and
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md)).

**What Canary takes from this:** Godot's node-and-scene model (versus an
ECS) trades some data-oriented performance for approachability and rapid
iteration — a legitimate, different tradeoff than Canary's archetype-ECS
target design
([`docs/architecture/core-runtime.md`](../architecture/core-runtime.md)).
Canary's bet is that a well-designed plugin/editor layer (see
[`docs/ui/editor-design.md`](../ui/editor-design.md)) can deliver comparable
approachability *on top of* an ECS core, rather than needing a
scene-graph-first model to get there — this is a hypothesis this project is
making, not a settled fact, and is exactly the kind of claim a future ADR
should revisit if the editor work in Era 5 finds otherwise.

## Bevy (Rust ecosystem, not a "big three" engine, but the most directly
relevant precedent)

**Strengths:** a real, actively developed, data-driven ECS engine in Rust —
on its 0.19 release as of mid-2026, with genuine commercial shipped titles
(Tiny Glade being the most visible) and a healthy plugin ecosystem
(`bevy_rapier`, Avian for physics; multiple community rendering
extensions). Bevy's existence and health was a direct input to
[ADR 0002](../decisions/architecture-decision-records/0002-primary-language-selection.md) —
it's concrete evidence that "Rust game engine" is not a purely theoretical
category in 2026.

**What Canary takes from this, and where it deliberately diverges:** Bevy is
explicitly code-only (no built-in visual editor as of this writing), and its
plugin ecosystem, while excellent, is native-Rust-only — a Bevy plugin is a
Rust crate, not a sandboxed, language-agnostic component. Canary's two-tier,
WASM-sandboxed plugin architecture
([ADR 0003](../decisions/architecture-decision-records/0003-plugin-and-modding-architecture.md))
is the most significant architectural point of departure from Bevy's
approach, specifically because "language-agnostic" and "community
marketplace" are stated Canary goals that a Rust-only native plugin model
doesn't satisfy. Bevy remains an excellent point of comparison precisely
*because* it shares Canary's ECS-first, Rust-first starting point — the
divergence is deliberate and specific, not a wholesale rejection of Bevy's
approach.

## The recurring lesson across all four: multiplayer as an afterthought

Across Unreal, Unity, and Godot alike, official or community networking
solutions have historically been layered on top of simulation/scene models
that didn't anticipate replication from the start, leading to well-known
friction (state that's awkward to replicate cleanly, prediction/
reconciliation bolted onto physics that wasn't built with rollback in
mind). This is the single most consistent theme this research surfaced, and
it's why Canary's ECS and networking architecture were designed together
from the start (see
[ADR 0007](../decisions/architecture-decision-records/0007-networking-and-multiplayer-model.md)),
even though the networking subsystem itself is a late-era deliverable.

## References

Findings above draw on 2026 coverage from Epic Games' own "State of
Unreal" announcements, the Bevy project's own release notes and
`bevy.org` news posts, Godot's own documentation (Jolt Physics
integration), and third-party industry analysis (GDC's annual State of the
Game Industry survey, SteamDB engine-tag tracking, and multiple
independent 2026 engine-comparison write-ups). Given how quickly this
market moves, verify current figures directly rather than treating this
document as a permanent citation.
