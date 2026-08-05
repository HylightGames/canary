# Technology Evaluations

The detailed, sourced evaluations behind the ADRs in
[`docs/decisions/architecture-decision-records/`](../decisions/architecture-decision-records/).
Conducted mid-2026; re-verify anything version-specific before relying on it
much later, especially for fast-moving projects (Zig, WASI, wgpu).

## Languages

### Zig

As of mid-2026, Zig had reached version **0.16.0** (April 2026) and remained
explicitly pre-1.0. Multiple independent sources, including commentary
attributed to Zig's own core team, describe the 1.0 delay as deliberate —
the guiding question reported is "what would we regret locking in if we
shipped it today" — with release notes describing an intent to keep
budgeting for breaking changes each release until specific stability bars
(e.g., zero disabled tests on Tier 1 targets) are met. Community and press
speculation places a 1.0 release as plausible later in 2026 or in 2027, but
this was not confirmed as of this research.

**Implication for Canary:** genuinely promising, real production users
exist (TigerBeetle is the most cited example), first-class C interop, and
a compelling "better C" design philosophy — but a language whose own
maintainers describe it as not yet safe to make compatibility promises
about is a poor foundation for a project whose own explicit goal is a
professional, multi-year, stable foundation. See
[ADR 0002](../decisions/architecture-decision-records/0002-primary-language-selection.md).
Revisit if/when Zig reaches a real 1.0.

### Rust game development ecosystem

Bevy (ECS engine) reached **0.19** in June 2026, with 0.18 (March 2026) and
earlier releases showing sustained contributor counts (170–280 contributors
per release cycle) and real commercial shipped titles. Community
comparisons of the broader Rust game-engine landscape (Bevy, Macroquad,
ggez, Fyrox) as of early-to-mid 2026 consistently describe Bevy as the
most-adopted, most actively developed option, with tradeoffs (steep
learning curve if unfamiliar with Rust, no built-in cinematic/sequencer
tooling, harder console porting) that are honestly acknowledged even by
Rust-community sources rather than glossed over.

**Implication for Canary:** validates that "Rust game engine" is a live,
non-theoretical category with real production experience to learn from —
directly supporting [ADR 0002](../decisions/architecture-decision-records/0002-primary-language-selection.md).

## Graphics

### wgpu

A mature, actively maintained, pure-Rust graphics library implementing a
WebGPU-inspired API natively over Vulkan, Metal, DirectX 12, and OpenGL,
plus WebGPU/WebGL2 when compiled to WebAssembly. Used as the actual WebGPU
implementation inside Firefox, Servo, and Deno, and as the rendering
foundation for multiple Rust game engines (Bevy, Fyrox, rend3). Known,
currently-documented gaps as of this research: no multi-GPU (SLI/CrossFire)
support, limited exposure of hardware ray-tracing/mesh-shader extensions
(some support is landing incrementally — mesh shader pipelines were noted
as newly supported on Vulkan, with Metal/DX12 support via passthrough
shaders), and no console NDA'd API backends.

**Implication for Canary:** a strong, low-risk bootstrap choice for the RHI
layer's initial implementation — see
[ADR 0004](../decisions/architecture-decision-records/0004-rendering-abstraction-strategy.md) —
with known, documented gaps that justify keeping the RHI trait boundary
genuinely swappable rather than treating `wgpu` as a permanent dependency.

## WebAssembly / plugin sandboxing

WASI 0.2 ("Preview 2"), which introduced the formal Component Model (typed
interfaces via WIT, enabling components written in different source
languages to compose), stabilized in **January 2024** and was reported as
solidly production-adopted by mid-2026 — Wasmtime is consistently described
as the mature reference runtime implementation, with capability-based
security (a component gets no filesystem/network/clock access unless
explicitly granted) as a from-scratch design property rather than a
retrofit. WASI Preview 3 (adding native async/threading) was in active,
non-final development as of this research — an additive improvement, not a
prerequisite for the capabilities Canary's plugin architecture depends on.

**Implication for Canary:** directly supports
[ADR 0003](../decisions/architecture-decision-records/0003-plugin-and-modding-architecture.md) —
the Component Model's typed, multi-language composition model and
capability-based sandboxing are close to a purpose-built fit for
"language-agnostic, marketplace-safe plugins," rather than a stretch
application of WASM to a problem it wasn't designed for.

## Physics

Three credible native/near-native options were identified:

- **Rapier** (pure Rust, Dimforge) — the most mature, most widely used
  option in the Rust ecosystem, with a stated 2026 roadmap toward
  GPU-accelerated rigid-body simulation via `rust-gpu`.
- **Jolt Physics** (C++, MIT-licensed) — purpose-built for multithreaded
  production game use; ships in Horizon Forbidden West and Death Stranding
  2; adopted by Godot 4.4 as a selectable alternative physics backend,
  which is itself evidence for the "swappable physics backend" pattern
  Canary adopts more broadly.
- **Avian** (Rust, ECS-native) — younger and less battle-tested than
  Rapier, but architecturally interesting for avoiding a separate
  synchronized "physics world," worth revisiting as it matures.

**Implication for Canary:** supports the trait-based
`PhysicsBackend` design in
[`docs/architecture/physics.md`](../architecture/physics.md), with Rapier
as the pragmatic default and Jolt as the reference example of a trusted,
native (Tier B) subsystem replacement.

## Networking transport

`quinn` (pure-Rust QUIC implementation) shows sustained, heavy real-world
usage (reported in the tens of millions of downloads) and has been actively
maintained since 2018. QUIC's core properties — multiplexed streams
without cross-stream head-of-line blocking, mandatory TLS 1.3 — map well
onto the mixed reliable/unreliable traffic patterns typical of games.

**Implication for Canary:** supports the transport choice in
[ADR 0007](../decisions/architecture-decision-records/0007-networking-and-multiplayer-model.md).

## Editor UI toolkit (evaluated, first backend chosen)

`egui` (immediate-mode, pure Rust) is a credible, low-friction option for
early tooling and dev UIs, with native `wgpu` integration precedent
already established in the ecosystem. Chosen as `CanaryUI`'s first
backend — see
[ADR 0011](../decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md)
and [`docs/architecture/ui-toolkit.md`](../architecture/ui-toolkit.md) —
specifically *because* the abstraction layer means this isn't a
permanent, unquestionable choice, the same posture already taken toward
`wgpu` for rendering.

## Local-first, collaborative state (CRDTs)

Evaluated as candidate backends for the long-term, genuinely-open
"real-time collaborative editing" direction named in
[`docs/architecture/state-and-versioning.md`](../architecture/state-and-versioning.md)
and [ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md) —
**not adopted or implemented; evaluated only.**

- **Automerge** — a pure-Rust-core CRDT library (JSON-like documents:
  maps, lists, text) from Ink & Switch, MIT-licensed, explicitly designed
  for "local-first" software, with a built-in sync protocol and a
  Git-like change-history model — a strong conceptual match for this
  project's own version-control-friendliness goals. Exposed to
  JavaScript via WebAssembly and to other languages via a C API; the
  Rust API itself is described by the project as lower-level and less
  polished than its JS wrapper, since the codebase is currently oriented
  around serving that wrapper.
- **Loro** — a newer, Rust-native (not just Rust-with-a-JS-wrapper) CRDT
  library implementing the Fugue algorithm, benchmarked as the fastest of
  the actively-compared options as of this research, with a more
  compact encoding than Automerge or Yjs. Less ecosystem maturity as a
  tradeoff for being newer.
- **Yjs** — the most widely adopted CRDT library overall (by a wide
  margin in download/star counts), but JavaScript-native with no
  first-party Rust bindings, making it a weaker fit for this project's
  Rust-first core ([ADR 0002](../decisions/architecture-decision-records/0002-primary-language-selection.md))
  than the two Rust options above.

**Implication for Canary:** if the CRDT-based direction is ever chosen
over server-authoritative operation broadcast for real-time collaborative
editing, Automerge or Loro are the credible pure-Rust starting points —
consistent with this project's consistent preference for depending on
mature existing libraries (`wgpu`, Rapier) over reinventing them.

## Localization

**Project Fluent** (`fluent-rs`) — dual MIT/Apache-2.0-licensed, actively
maintained Rust implementation of Mozilla's Fluent localization system.
Chosen over flat key-value formats (gettext `.po`, plain JSON tables) for
[`canary-loc`](../architecture/localization.md) specifically because its
`.ftl` syntax handles pluralization, grammatical gender, and
value-interpolation word order natively — the genuinely hard parts of
correct multilingual UI text, not just string storage. A real supporting
ecosystem exists beyond the core crates (`fluent-templates`, `i18n-embed`
for locale loading/fallback), reducing the amount Canary needs to build
itself.

## References

Sourcing for this document includes: the Zig Software Foundation's own
devlog and news posts (`ziglang.org`); Wikipedia's maintained entries on
Zig and Jai (used only for release-date/version facts, cross-checked
against the project's own sites); Bevy's own release announcements
(`bevy.org/news`) and community sources (This Week in Bevy); `wgpu`'s own
documentation, changelog, and crates.io listing; multiple 2026
WebAssembly/WASI status write-ups (Bytecode Alliance-adjacent and
independent); Dimforge's own 2025-review/2026-goals blog post for Rapier;
the Jolt Physics GitHub repository and Godot's own documentation for Jolt
integration; `quinn`'s own repository and crates.io listing; and, for the
CRDT evaluation, Automerge's own documentation and GitHub repository
(automerge/automerge), the `crdt.tech` implementations index, and
multiple independent 2026 CRDT-library comparison write-ups (Yjs vs.
Automerge vs. Loro). As with the engine comparisons document, treat
version numbers and adoption figures here as a mid-2026 snapshot.
