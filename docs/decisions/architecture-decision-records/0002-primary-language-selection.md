# 0002. Rust as the primary implementation language for the engine core

**Status:** Accepted

## Context

Canary needs a systems-level implementation language for the trusted engine
core (Layers 1–3 in [engine-overview.md](../../architecture/engine-overview.md)):
something capable of high-performance, low-level, native-compiled code,
suitable for a large, long-lived, many-contributor open-source codebase, with
a credible cross-platform and graphics-API story, and — because "language-
agnostic" and "multiple programming language support" are stated project
goals — a strong story for exposing its functionality to *other* languages
rather than assuming everyone will simply write Rust.

Candidates evaluated: C++, Rust, Zig, C, and hybrid combinations.

## Decision

**Rust** is the primary implementation language for the engine core.

Key factors:

- **Memory safety without a garbage collector.** For a project explicitly
  optimizing for many outside contributors of varying experience levels (an
  open-source engine, not a closed studio codebase with a fixed senior
  team), the compiler catching use-after-free, data races, and null-deref
  classes of bugs at compile time is a direct, measurable reduction in the
  defect surface a reviewer has to catch by hand. This matters more for
  Canary's contributor model than it would for a studio with a small,
  stable, expert team.
- **First-class WebAssembly target.** Rust's `wasm32-wasip2` target and
  tooling maturity directly enable the two-tier plugin architecture in
  [ADR 0003](0003-plugin-and-modding-architecture.md) — this is not
  incidental to the language choice, it's one of the reasons for it.
- **Tooling that matches "professional-grade" as a day-one property**:
  `cargo` (build + package management + workspaces in one tool, no separate
  build-system decision required — see
  [ADR 0005](0005-build-system-and-tooling.md)), `rustfmt` and `clippy` as
  ecosystem-standard formatting/linting, `cargo doc` for documentation
  generation directly from doc comments.
- **A credible, current gamedev ecosystem**, not just a theoretical one: as
  of mid-2026, `wgpu` (cross-API graphics, backing Firefox's and Servo's
  WebGPU implementations) and Bevy (a data-driven Rust ECS engine on its
  0.19 release, with real shipped commercial titles) demonstrate Rust
  gamedev is a going concern, not a research project. See
  [`docs/research/technology-evaluations.md`](../../research/technology-evaluations.md)
  for the sourcing.
- **C ABI interop remains available** for the trusted native plugin tier
  ([ADR 0003](0003-plugin-and-modding-architecture.md)), so choosing Rust as
  the *host* language does not wall off the C/C++ ecosystem (existing
  physics/audio/middleware libraries remain usable via FFI bindings).

## Alternatives considered

**C++.** The industry-standard choice, and not rejected on capability
grounds — modern C++ is entirely capable of everything Canary needs. Rejected
specifically because: (a) it does not catch memory-safety and data-race bugs
at compile time, which matters disproportionately for a project accepting
contributions from a wide, variable-experience pool rather than a fixed
expert team; (b) its tooling (build systems, package management) is
fragmented across the ecosystem rather than standardized, which cuts against
"professional-grade foundation" as a day-one property; (c) it does not have
as mature a WebAssembly Component Model story as Rust does today, which
would have weakened the case for [ADR 0003](0003-plugin-and-modding-architecture.md).
None of this means C++ is a bad language — it means it's a worse fit for
*this* project's specific contributor model and plugin architecture. C++
remains fully usable for Tier B native plugins and for wrapping existing
middleware.

**Zig.** A genuinely appealing modern systems language — explicit control
flow, no hidden allocations, first-class C interop, and (per research
conducted for this decision) real production users. Rejected **for now**,
specifically because Zig had not reached a 1.0 release as of this
foundation's writing (mid-2026): the language's own maintainers have
explicitly and publicly said they're declining to rush 1.0 specifically
*because* they don't want to lock in details they might regret, and the
project's own release notes describe budgeting for breaking changes each
release. Building a "professional-grade, multi-year foundation" on a
language whose own creators describe it as not yet ready to make stability
promises is a risk this decision explicitly declines to take on. This is a
**timing** judgment, not a permanent one — Zig remains fully viable as a
Tier B native-plugin language today (it compiles to a stable C ABI), and
this decision should be revisited if/when Zig reaches a real 1.0 with a
compatibility commitment. See
[`docs/research/technology-evaluations.md`](../../research/technology-evaluations.md)
for the sourcing behind this assessment.

**C.** Rejected as the primary implementation language on productivity
grounds for a large, modular, multi-year engine — manual memory management
and minimal language-level modularity make it a worse fit for the *engine
core* specifically, though C remains, as it always has been, the de facto
lingua franca for FFI and is fully supported as a Tier B plugin language.

**A hybrid split from the start** (e.g., C++ for the renderer, Rust for
gameplay/ECS). Rejected for this foundation specifically because it would
mean paying the integration cost of two build systems and two safety models
inside the *trusted core* on day one, before there's evidence any specific
subsystem actually needs it. The engine's layered, trait-based subsystem
design (see [engine-overview.md](../../architecture/engine-overview.md))
already allows a future subsystem to be implemented in another language
exposed via the Tier B C ABI, if a concrete, specific need arises later —
this decision defers that cost until (and unless) it's actually justified,
rather than paying it speculatively now.

## Consequences

- Contributors to the trusted engine core need to know Rust. Contributors to
  gameplay scripts, mods, and marketplace plugins do not — see
  [ADR 0003](0003-plugin-and-modding-architecture.md).
- The project inherits Rust's smaller (though growing) gamedev hiring/
  contributor pool relative to C++, and Rust's typically longer compile
  times relative to C — both accepted tradeoffs, not overlooked ones.
- Some third-party middleware (certain console SDKs, some proprietary
  physics/audio libraries) is C/C++-only; these remain usable via FFI, at
  the cost of an `unsafe` boundary that must be carefully reviewed (see
  [`docs/development/coding-standards.md`](../../development/coding-standards.md)).
- This decision should be revisited (as a new ADR, not a silent reversal) if
  Zig reaches 1.0 with a real compatibility commitment, or if a concrete
  subsystem emerges with a strong, specific reason to be implemented outside
  Rust.
