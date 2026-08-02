# 0003. Two-tier plugin architecture: sandboxed WASM components + trusted native C ABI

**Status:** Accepted

## Context

Canary's stated goals include "language-agnostic development," "multiple
programming language support," "everything built around plugins," "community
marketplace ecosystem," and "modding-first philosophy" simultaneously. A
single native (DLL/so) plugin mechanism — the conventional approach — cannot
satisfy all of these at once: it ties plugin authorship to FFI compatibility
with the host language, and it gives every installed plugin full process
access, which is incompatible with a marketplace of untrusted community
content.

## Decision

Plugins and mods are supported through **two distinct tiers** with different
trust models, detailed in full in
[`docs/architecture/plugin-system.md`](../../architecture/plugin-system.md):

- **Tier A** — sandboxed, capability-based WebAssembly Components (WASI 0.2 /
  "Preview 2" Component Model, run on Wasmtime), ahead-of-time compiled to
  native machine code at build/publish time. This is the default tier for
  gameplay scripts, community mods, and marketplace content, and the tier
  that makes "language-agnostic" concretely true (any language with a
  Component Model toolchain can author a plugin).
- **Tier B** — a trusted, versioned, native C ABI, loaded as a dynamic
  library, for performance-critical subsystem replacement by trusted
  contributors/studios where sandboxing overhead is unjustified because the
  author is already trusted.

## Alternatives considered

**Single native plugin tier (status quo approach).** Rejected: makes a
public marketplace of community content an unacceptable security posture
(every install is unsandboxed code execution), and ties "which languages can
write plugins" to FFI/ABI compatibility rather than design choice.

**Single WASM tier for everything, including trusted subsystem replacement.**
Rejected: forces a sandboxing/performance tax onto trusted, performance-
critical code (a custom renderer backend, a physics engine swap) that never
needed the safety guarantee in the first place, since the entity providing
it is already trusted by construction. Splitting tiers lets each be
optimized for its actual trust model instead of splitting the difference for
both.

**Embed a single scripting language runtime (e.g., Lua) as the sole
extensibility mechanism**, in the style of many existing engines. Rejected:
solves "designer-friendly scripting" reasonably well, but does nothing for
"language-agnostic" (only Lua) or for "replaceable engine subsystems," which
need native-level access a scripting-language sandbox typically doesn't
provide. A Lua-style embedded scripting language remains a *possible future
addition on top of* Tier A (compiled to WASM like anything else) if
community demand emerges, per
[`docs/architecture/scripting-system.md`](../../architecture/scripting-system.md) —
this decision doesn't foreclose it, it just declines to make it the *only*
mechanism.

**WASI Preview 1 instead of the Component Model (Preview 2).** Rejected:
Preview 1 lacks a formal, typed interface system for cross-language
composition — exactly the property "language-agnostic" depends on. Preview
2 (WASI 0.2) stabilized in January 2024 and was confirmed, via research
conducted for this decision, to have solid production-grade runtime support
(Wasmtime as reference implementation) well before this foundation's
writing — see
[`docs/research/technology-evaluations.md`](../../research/technology-evaluations.md).

**Wait for WASI Preview 3 (native async)** before committing to the
Component Model. Rejected: Preview 3 is additive (async/threading) rather
than a prerequisite for the capability-based sandboxing and typed
cross-language composition this decision actually needs; waiting for it
would delay a foundational decision for a benefit (native async host calls)
that can be adopted incrementally later without revisiting this ADR.

## Consequences

- The engine core needs a WASM component runtime (Wasmtime) as a dependency
  once Tier A is implemented — deferred past v0.0.1-pre1, see
  [`docs/roadmap/v0.0.1-roadmap.md`](../../roadmap/v0.0.1-roadmap.md).
- Marketplace/community content is sandboxed by construction, not by review
  policy — review tooling (Era 6) can focus on capability-appropriateness
  and quality, not on "did we catch every way this could touch the
  filesystem."
- There is a genuine host/component call-boundary performance cost for very
  fine-grained, high-frequency script calls; the mitigation (batched calls)
  is documented in
  [`docs/architecture/scripting-system.md`](../../architecture/scripting-system.md#performance-expectations)
  as a design pattern, not treated as an unsolved problem.
- Trusted native (Tier B) plugins have zero sandboxing and full process
  access; this is a deliberate, documented exception, not an oversight, and
  is not the intended distribution path for community/marketplace content.
- This decision directly shapes [ADR 0002](0002-primary-language-selection.md)
  (Rust's WASM tooling maturity was a factor there) and is itself referenced
  by nearly every other architecture document in this repository — it is one
  of the two structural bets the whole engine design rests on (see
  [engine-overview.md](../../architecture/engine-overview.md#the-two-structural-bets-this-engine-makes)).
