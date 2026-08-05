# Long-Term Roadmap (Vision Level)

This is a *narrative* roadmap — the story arc of the project across years.
For the concrete, versioned plan, see [`docs/roadmap/`](../roadmap/), which
this document does not try to duplicate. Nothing here is a commitment to a
date; multi-year open-source projects that promise dates tend to either miss
them or cut corners to hit them; neither serves Canary's stated goal of being
a strong foundation.

## Era 1 — Foundation (complete, `v0.0.1`)

Repository, governance, documentation architecture, ADR process, and a
headless, compiling skeleton: logging, a minimal ECS, the plugin trait
surface, and the build/test/CI pipeline. No rendering, no windowing, no
networking. The deliverable of this era is *a foundation other engineers can
build on without re-litigating the basics*. Delivered as `v0.0.1` — see
[`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md) and
[`RELEASE_NOTES_v0.0.1.md`](../../RELEASE_NOTES_v0.0.1.md).

## Era 2 — Core Simulation (in progress)

The ECS grows from the placeholder into the archetype-based,
parallel-scheduled design described in
[`docs/architecture/core-runtime.md`](../architecture/core-runtime.md).
Platform abstraction gains a real windowing/input backend. The WASM
(Tier A) component plugin tier lands. This era proves out "language
agnostic" and "replaceable subsystems" with working code, not just docs.
**The native (Tier B) plugin loader — dynamic library loading with a
versioned C ABI — is already real, shipped in `v0.0.1`**; what's left of
this era is the archetype ECS and the Tier A WASM tier alongside it. See
[`docs/roadmap/v0.0.2-roadmap.md`](../roadmap/v0.0.2-roadmap.md) for the
current, narrower, in-progress slice of this era.

## Release cadence: one focused subsystem per `0.0.x`, target `v0.1.0` as substantially feature-complete

Recorded explicitly after `v0.0.1` shipped, at the project owner's
request: each `0.0.x` release (per the numbering in
[ADR 0006](../decisions/architecture-decision-records/0006-versioning-scheme.md))
should have **one** primary subsystem as its focus — the archetype ECS
migration, then the Tier A WASM loader, then real windowing, and so on —
rather than bundling several into one jump. Smaller, single-focus,
sequential releases are easier to review, test, and course-correct on
than large ones, and match this project's existing preference for
"fewer, higher-quality decisions" over batched scope.

The target this cadence is aimed at: **by `v0.1.0`, substantially all of
the currently-documented architecture (`docs/architecture/`) should be
implemented**, not just designed — rendering, physics, networking, the
full plugin system, the UI toolkit, and the near/medium-term slice of
project-state-and-versioning — such that `v0.1.0` onward is primarily
maintenance and hardening rather than new major subsystems. This is a
target, not a promise with a date attached — the same reasoning
[ADR 0006](../decisions/architecture-decision-records/0006-versioning-scheme.md)
already gives for not calendar-versioning applies here too — but it's
useful for every future `0.0.x` release to be scoped against: does this
slice of work move the project toward "the documented architecture is
real" as directly as possible, without scope creep into things not yet
documented at all.

## Era 3 — Rendering & Content

The RHI trait gets its first real backend (wgpu, per
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md)),
a render graph, and a materials system. The asset pipeline
(`docs/architecture/asset-system.md`) grows a real cook/import step with
hot-reload. This is the era where Canary becomes usable for an actual small
game, even without an editor.

## Era 4 — Networking & Live Systems

Server-authoritative replication over the ECS, transport via QUIC, client
prediction/reconciliation. This era is explicitly informed by how painful it
is to retrofit multiplayer onto engines that didn't design for it — see
[`docs/research/engine-comparisons.md`](../research/engine-comparisons.md) —
so much of its groundwork (replicated component markers, authority model) is
seeded much earlier, in Era 2.

## Era 5 — Editor & Tooling

The editor is built as a first-class plugin host on top of the plugin system
from Era 2, using the panel/workspace model in
[`docs/ui/editor-design.md`](../ui/editor-design.md). This is deliberately
late: an editor built before the engine's plugin API is proven tends to
enshrine that API's early mistakes.

## Era 6 — Ecosystem & Marketplace

The sandboxed WASM plugin tier, proven internally since Era 2, opens to the
public as a community marketplace. Security review tooling, capability
policy UX, and plugin discovery become first-class concerns here, not
afterthoughts bolted onto a trust model that was never designed for
strangers' code.

## Era 7 — Hardening toward `v1.0.0`

API stabilization, backward-compatibility audits, performance passes across
supported platforms, and — if a motivated backer or community emerges for
it — console SDK integration. `v1.0.0` per
[ADR 0006](../decisions/architecture-decision-records/0006-versioning-scheme.md)
means a real compatibility commitment, so this era is intentionally the
slowest and most conservative one.

## What doesn't have an era yet

AI/ML integration (`canary-ai`), a visual scripting graph, and console
platform support are all real, intended parts of Canary's future, but are
deliberately not assigned to a specific era yet — forcing a date onto them
now would be guessing, and this roadmap would rather be honestly vague than
confidently wrong. They're tracked as open items in
[`docs/roadmap/future-roadmap.md`](../roadmap/future-roadmap.md).
