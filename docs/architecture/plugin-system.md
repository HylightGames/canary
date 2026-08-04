# Plugin System

This is the subsystem most directly responsible for "language-agnostic,"
"modding-first," "community marketplace," and "replaceable subsystems" as
stated goals. See
[ADR 0003](../decisions/architecture-decision-records/0003-plugin-and-modding-architecture.md)
for the decision record; this document is the architectural detail behind
it.

## The problem with a single native plugin tier

The conventional approach — plugins as native dynamic libraries (`.dll` /
`.so` / `.dylib`) loaded into the host process — has one native ABI (usually
whatever the host language's compiler happens to produce) and one trust
level (full process access). That means:

- "Which languages can write a plugin" is dictated by FFI compatibility with
  the host language's ABI, not by design choice.
- A community marketplace of native plugins is a supply-chain security
  problem: installing a mod means running arbitrary code with the game's
  full privileges (filesystem, network, memory of other plugins).
- There is no meaningful way to sandbox "how much can this plugin touch"
  short of OS-level process isolation, which is heavyweight and awkward for
  something as fine-grained as a gameplay mod.

Canary's answer is to stop treating this as one problem with one mechanism,
and split it into two tiers with different trust models.

## Tier A — Sandboxed, language-agnostic (WebAssembly Components)

**What it's for:** community mods, marketplace plugins, gameplay scripts,
anything from an untrusted or semi-trusted author.

**How it works:** plugins are compiled to WebAssembly and distributed as
[Wasm Components](https://component-model.bytecodealliance.org/) using the
WASI 0.2 ("Preview 2") interface model — a formal, typed interface
description (WIT) rather than a flat set of numeric imports/exports. This
was a deliberate, research-backed choice: WASI 0.2 stabilized in early 2026,
Wasmtime is a mature reference runtime for it, and — critically for
"language-agnostic" — the Component Model is specifically designed so
components written in different source languages can compose through a
shared typed interface. See
[`docs/research/technology-evaluations.md`](../research/technology-evaluations.md)
for the sourcing behind this.

**Sandboxing is capability-based, not a blocklist.** A component gets *no*
ambient authority — no filesystem, no network, no clock — unless the host
explicitly grants it as a typed capability at instantiation time. A
marketplace gameplay mod that only manipulates ECS components it declares up
front never has an *opportunity* to read the filesystem, because it was
never handed that capability, not because a runtime check happened to catch
it.

**Native compilation during builds.** Ahead-of-time compilation (Wasmtime's
`wasmtime compile`, producing a `.cwasm` artifact) turns a component's
bytecode into natively executing machine code as part of the build/publish
step, so shipped games don't pay a JIT-warmup or interpretation cost at
runtime — the "sandboxed" and "native speed" properties are not in tension
here, because the sandbox boundary is the capability model, not an
interpreter loop.

**Source languages supported:** anything with a WASI 0.2 / Component Model
toolchain — Rust, C, C++, Zig, Go (via TinyGo), and a growing set of others.
This is where "multiple programming language support" and "combinations of
languages" (both stated goals) actually get delivered, without the engine
core needing to embed N language runtimes.

## Tier B — Trusted, native (C ABI)

**What it's for:** performance-critical subsystem replacement (a different
renderer backend, a different physics engine, a different asset importer)
by trusted contributors or studios who need zero sandboxing overhead and
full system access. Not the default distribution channel for community
content.

**How it works:** a versioned, stable C ABI (`extern "C"` boundary,
`#[repr(C)]` data), loaded as a native dynamic library via `libloading`.
This tier deliberately does not try to be safe against a malicious plugin —
that is what Tier A is for. It exists because some legitimate use cases
(swapping the renderer, integrating a proprietary physics/audio middleware)
need direct native access that a sandbox would only slow down for no safety
benefit, since the entity doing it is trusted by construction (a studio
shipping their own engine fork, or a reviewed, signed core extension).

**Why not just "unsafe Rust plugins" instead of a C ABI?** A stable C ABI
means Tier B plugins aren't required to be written in Rust, or even in the
same Rust compiler version as the host — which matters for a studio
integrating an existing C++ middleware library, and keeps this tier
consistent with the project's "language-agnostic" goal instead of quietly
becoming "Rust-only, if you're trusted."

## Why two tiers instead of one

| | Tier A (WASM) | Tier B (native C ABI) |
|---|---|---|
| Trust model | Untrusted/semi-trusted by default | Trusted contributors/studios |
| Sandboxing | Capability-based, enforced by the runtime | None — full process access |
| Performance | Near-native (AOT-compiled); some overhead at the host/component boundary | Zero overhead |
| Marketplace-safe | Yes — this is the marketplace tier | No — not the intended distribution path |
| Source languages | Any WASI 0.2 / Component Model target | Anything exposing a C ABI |
| Intended use | Mods, gameplay scripts, community plugins | Subsystem replacement, proprietary middleware |

Collapsing these into one tier forces a bad tradeoff: sandbox everything
(and pay a performance/complexity tax on trusted core-subsystem code that
never needed it) or sandbox nothing (and make a marketplace irresponsible).
Keeping them separate means each tier's design can actually be optimized for
its real trust model instead of splitting the difference.

## The plugin trait surface (`canary-plugin-api`)

Both tiers are exposed to engine code through the same conceptual `Plugin`
trait — lifecycle hooks (`name`, `on_load`, `on_unload`) implemented
identically regardless of tier — so that "load a plugin" is one concept in
the engine's public API even though the two tiers are implemented very
differently underneath. v0.0.1 ships:

- The `Plugin` trait and the `Capability` declaration type (currently
  advisory-only — see [`Capability`](../../engine/canary-plugin-api/src/capability.rs)'s
  own docs for why).
- A working, versioned native (Tier B) loader using `libloading` and a
  hand-rolled, `#[repr(C)]` vtable ABI with an explicit version field and
  forward-extension hook (see [ADR 0009](../decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md)) —
  chosen for this milestone because it requires no async runtime and no
  WASM toolchain to prove the loader mechanism, and the trust/ABI-safety
  boundary, end to end.
- **Not yet implemented:** the Wasmtime-backed Tier A loader. The interface
  types are designed to accommodate it (see doc comments in
  `engine/canary-plugin-api/src/lib.rs`), but wiring in an async WASM
  runtime, a capability-grant UX, and WIT interface definitions is
  substantial scope on its own — deliberately deferred to `v0.0.2`+ rather
  than rushed; see
  [`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md).

## Marketplace and the security model

The community marketplace (Era 6 in
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md)) is
downstream of Tier A working correctly, not a separate effort. Concretely,
the marketplace's job is: capability review (what is this plugin asking to
access, and does that match what it claims to do), distribution, and
discovery — the actual isolation guarantee is provided by the engine's
plugin runtime, not by marketplace review policy. This is the same
"architecture, not policy" preference that shows up throughout
[`docs/vision/design-philosophy.md`](../vision/design-philosophy.md).

## Editor as a plugin host

The future editor (see [`docs/ui/editor-design.md`](../ui/editor-design.md))
is intended to be built on top of this same plugin system — editor panels,
inspectors, and tools are plugins, using the same loader. This is a
deliberate dogfooding decision: if the plugin API isn't good enough to build
the editor's own panels, it isn't good enough for third-party mods either,
and the project will find that out from its own tooling instead of from a
frustrated modder.

## Known limitations

- **Component identity doesn't yet have a language-agnostic form** for
  Tier A plugins to declare capabilities against — see
  [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)
  (`Proposed`). Deferred to `v0.0.2`+ alongside the archetype ECS
  migration and the Tier A loader itself.
- **No plugin manifest format exists yet** (metadata: author, version,
  engine-compatibility range, declared capabilities as data). Needed
  before marketplace tooling (Era 6) can exist without executing a
  plugin just to learn its name. See
  [`docs/reviews/2026-08-senior-architecture-review.md`](../reviews/2026-08-senior-architecture-review.md),
  Finding 3.2, and risk register R-08. Correctly out of scope for
  `v0.0.1` — there is no marketplace to serve yet.
- **"Trusted" (Tier B) currently has no verification mechanism** —
  signing, checksums, or provenance are all unaddressed. Fine for a
  solo-architect foundation; a real supply-chain risk the moment Tier B
  plugins are distributed beyond the person who compiled them. See the
  review, Finding 3.3, and risk register R-09.
- **No engine/plugin compatibility-range declaration mechanism** exists
  beyond the ABI version check below. See the review, Finding 3.4, and
  risk register R-19.

Resolved since the August 2026 review, for `v0.0.1`: the Tier B vtable
now carries an explicit `abi_version` and a `get_extension` hook for
future growth (`engine/canary-plugin-api/src/abi.rs`), and
`NativePluginLoader::load` rejects a version mismatch outright rather
than risking a silent, incompatible read — see
[ADR 0009](../decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md),
which was the single most consequential technical finding of that
review. Validated by a test
(`engine/canary-plugin-api/tests/native_loader.rs`) that compiles a real
C plugin declaring a wrong version and confirms the loader actually
rejects it, not merely that the check compiles.
