# Long-Term Roadmap (Vision Level)

This is a *narrative* roadmap — the story arc of the project across years.
For the concrete, versioned plan, see [`docs/roadmap/`](../roadmap/), which
this document does not try to duplicate. Nothing here is a commitment to a
date; multi-year open-source projects that promise dates tend to either miss
them or cut corners to hit them; neither serves Canary's stated goal of being
a strong foundation.

## Era 1 — Foundation (where this session ends)

Repository, governance, documentation architecture, ADR process, and a
headless, compiling skeleton: logging, a minimal ECS, the plugin trait
surface, and the build/test/CI pipeline. No rendering, no windowing, no
networking. The deliverable of this era is *a foundation other engineers can
build on without re-litigating the basics*. Tracked in
[`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md).

## Era 2 — Core Simulation

The ECS grows from the pre1 placeholder into the archetype-based,
parallel-scheduled design described in
[`docs/architecture/core-runtime.md`](../architecture/core-runtime.md).
Platform abstraction gains a real windowing/input backend. The native-tier
plugin loader becomes real (dynamic library loading with a versioned C ABI),
and the WASM component tier lands behind it. This era proves out "language
agnostic" and "replaceable subsystems" with working code, not just docs.

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
