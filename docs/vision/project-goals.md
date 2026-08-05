# Project Goals

## What Canary is

Canary Engine is an open-source, MIT-licensed game engine built from first
principles rather than as a clone of an existing one. It is a multi-year
effort; this document describes what it is trying to become, not what exists
today. For what exists today, see [`docs/roadmap/milestones.md`](../roadmap/milestones.md).

Canary exists because the three dominant engines — Unreal, Unity, and Godot —
each made reasonable tradeoffs for the era and constraints they were designed
in, and those tradeoffs now show up as friction that a fresh design can avoid.
None of them were built with a sandboxed, language-agnostic plugin substrate,
a single source of truth for "is this decision locked in," or a governance
model immune to a single company's pricing decisions. Canary is an attempt to
make different tradeoffs, deliberately, and to write down *why* at every step
so the reasoning survives contributor turnover — see
[`docs/decisions/architecture-decision-records/`](../decisions/architecture-decision-records/).

## 2D and 3D, games and beyond

Canary is **branded and marketed as a game engine** — that's its primary
audience and the framing every document in this project uses — but
nothing in its architecture is 3D-only or shipping-a-game-only, and that's
deliberate, not incidental. The precedent this follows explicitly: Unreal
Engine is branded as a game engine too, and is simultaneously used for
film and TV virtual production, architectural and product visualization,
automotive design review, broadcast graphics, and simulation — the brand
says "game engine," the actual tool is a general-purpose real-time
content creation platform. Canary aims at the same relationship between
brand and capability, not at being 3D-only or narrowly scoped to shipped
games:

- **2D is first-class, not a 3D engine pretending to support 2D.** The
  physics backend ([`docs/architecture/physics.md`](../architecture/physics.md))
  is chosen specifically because it ships real, separate 2D and 3D
  implementations rather than one 3D system 2D games have to work around.
  The rendering architecture
  ([`docs/architecture/rendering.md`](../architecture/rendering.md))
  treats 2D (orthographic, sprite-based) as a first-class specialization
  of the same render graph, not a bolted-on separate renderer.
- **Non-game real-time use cases are in scope**, not excluded by
  assumption: previz and virtual production, architectural/product
  visualization, simulation, and general real-time 3D or 2D content
  creation and rendering all fit within the same engine, the same way
  they do for the engines Canary is learning from. Nothing in Canary's
  architecture requires "this is a shippable game" as a precondition for
  being useful.

This isn't a promise to specifically build tooling for any one of those
adjacent use cases ahead of the core engine — it's a statement about what
the architecture should never accidentally foreclose. See
[`docs/architecture/physics.md`](../architecture/physics.md) and
[`docs/architecture/rendering.md`](../architecture/rendering.md) for
where this becomes concrete.

## What problems it solves

1. **Modding and marketplace safety are usually an afterthought.** Native
   plugin systems (DLL/so-based) give mods full process access, which means
   "install this mod" is really "run arbitrary code with my privileges."
   Canary's plugin architecture (see
   [`docs/architecture/plugin-system.md`](../architecture/plugin-system.md))
   treats sandboxing as a first-class design constraint, not a marketplace
   review policy layered on top.

2. **Engines pick one implementation language and everyone else adapts.**
   Contributors, tool authors, and modders are locked into whatever the
   engine's scripting binding happens to be. Canary treats "which language did
   you write your plugin in" as an implementation detail behind a stable
   interface, not a fork in the road.

3. **Replacing a subsystem (renderer, physics, importer) usually means
   forking the engine.** Canary's subsystems sit behind trait-based
   interfaces from day one specifically so a studio or contributor can swap
   one out without carrying a fork.

4. **"Professional" and "accessible" are treated as opposing goals.**
   Modularity is the resolution: a solo developer uses a minimal slice of the
   engine; a AAA studio enables the heavier optional subsystems. Nobody pays
   (in complexity, compile time, or binary size) for what they don't use.

5. **AI-assisted development is bolted onto codebases that weren't designed
   for legibility.** Canary treats consistent conventions, exhaustive doc
   comments, and explicit architectural records as tooling for both human and
   AI contributors — see [`docs/vision/design-philosophy.md`](design-philosophy.md#ai-ready-architecture)
   for what "AI-ready" actually commits us to.

## What makes it different

- **Two-tier plugin architecture.** A sandboxed, language-agnostic
  WebAssembly Component tier for mods and marketplace content, and a trusted
  native C-ABI tier for performance-critical subsystem replacement. Most
  engines have one native extension mechanism and treat safety as a review
  process; Canary treats it as an architectural boundary. Details in
  [`docs/architecture/plugin-system.md`](../architecture/plugin-system.md).
- **Decisions are written down before they're load-bearing.** Every
  hard-to-reverse choice gets an
  [ADR](../decisions/architecture-decision-records/) explaining the
  alternatives and why they were rejected — not just the choice that won.
- **Versioning that says what it means.** Everything before `v1.0.0` is
  explicitly experimental — see
  [ADR 0006](../decisions/architecture-decision-records/0006-versioning-scheme.md) —
  so nobody mistakes a `pre` build for a stability promise.
- **Governance that doesn't depend on one company's roadmap.** MIT license,
  no runtime fee, no seat license, no dual-license bait-and-switch reserved
  for later. What's here is what you get.

## What it refuses to compromise on

- **No native, unsandboxed code execution from untrusted plugins**, ever,
  regardless of how much convenience sandboxing costs. See
  [ADR 0003](../decisions/architecture-decision-records/0003-plugin-and-modding-architecture.md).
- **No architectural decision without a written rationale.** "We just did it
  that way" is not an acceptable answer to "why," on a project meant to
  outlive the people currently working on it.
- **No silently breaking the versioning promise.** Once something ships as
  `v0.0.1` (not `-preN`), the compatibility commitment described in
  [ADR 0006](../decisions/architecture-decision-records/0006-versioning-scheme.md)
  applies to it.
- **No pretending a milestone is done when it's a stub.** Where this
  foundation ships a deliberately minimal placeholder (the v0.0.1 ECS,
  for instance), the docs say so explicitly rather than describing aspiration
  as fact. See [`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md).

## Non-goals (for now)

Being explicit about what Canary is *not* trying to do yet prevents scope
creep from quietly becoming the roadmap:

- Canary is not trying to ship a finished visual editor in the first several
  milestones. See [`docs/ui/editor-design.md`](../ui/editor-design.md) for the
  target design and why it's deferred.
- Canary is not trying to out-render Unreal's Nanite/Lumen on day one. The
  rendering architecture (see
  [`docs/architecture/rendering.md`](../architecture/rendering.md)) is built
  to *reach* AAA-grade rendering without requiring it immediately.
  first-party console SDKs (PlayStation, Xbox, Switch) are out of scope until
  there's an engine mature enough, and a community or backer motivated
  enough, to do the NDA'd integration work.
