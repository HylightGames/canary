# 0009. Plugin ABI versioning and forward-extensibility for the Tier B vtable

**Status:** Accepted (decision accepted; not yet implemented in code —
see `docs/reviews/2026-08-senior-architecture-review.md`, Finding 3.1,
which this ADR resolves at the decision level, and the risk register,
R-03, for implementation tracking)

## Context

[ADR 0003](0003-plugin-and-modding-architecture.md) and
[`plugin-system.md`](../../architecture/plugin-system.md#tier-b--trusted-native-c-abi)
establish a stable, `#[repr(C)]`, manual-vtable C ABI for Tier B native
plugins, specifically to avoid the cross-compiler-version instability of
Rust `dyn Trait` fat pointers. As implemented in `v0.0.1-pre1`
(`engine/canary-plugin-api/src/abi.rs`), `PluginHandle`/`PluginVTable` is
a fixed-size struct — a context pointer plus exactly four function
pointers (`name`, `on_load`, `on_unload`, `destroy`) — with no version
field anywhere in it and no mechanism for a host to query for
capabilities beyond those four.

Two needs already named elsewhere in this project's own architecture
would require adding to this struct: state serialization for hot reload
([`scripting-system.md`](../../architecture/scripting-system.md#hot-reload))
and capability introspection
([`plugin-system.md`](../../architecture/plugin-system.md#the-plugin-trait-surface-canary-plugin-api)).
Adding a field to a `#[repr(C)]` struct changes its size and layout. A
plugin compiled against today's layout, loaded by a host built against a
hypothetical future layout with an added field (or the reverse), does not
fail at compile time or even with a clean runtime error — it reads or
writes past where it should, most likely producing a crash or silent
memory corruption deep inside FFI code, in third-party code this project
may not control the source of. A stable ABI meant to last a decade should
fail loudly and immediately on a mismatch, not silently and eventually.

The cost of fixing this is, right now, as close to zero as it will ever
be: no plugin exists in the wild depending on the current layout. Every
day this ships unversioned is a day closer to that no longer being true.

## Decision

Before further plugin-system implementation work continues:

1. **Add an explicit `abi_version: u32` field**, placed first in
   `PluginHandle` (or in a small fixed header preceding it), populated by
   the plugin's `canary_plugin_entry` and checked by
   `NativePluginLoader::load` *before* any other field is trusted.
   A mismatch produces a new, explicit `PluginError::UnsupportedAbiVersion`
   rather than proceeding.
2. **Adopt a `get_proc_address`-style extension-query function** as a
   permanent, additional vtable entry —
   `get_extension: unsafe extern "C" fn(context, name: *const c_char) ->
   *const c_void` — through which a host can ask a plugin for optional,
   named capabilities (state serialization, capability introspection, and
   whatever else is identified later) without ever changing the size or
   layout of the core struct again. This is the same pattern used by
   Vulkan (`vkGetInstanceProcAddr`) and OpenGL for exactly this reason:
   it lets an ABI grow indefinitely through composition instead of
   through mutation of a frozen struct.

Only genuinely breaking changes to the *core* four lifecycle functions
(not merely additive ones) should ever bump `abi_version` and change the
required export symbol name (e.g. `canary_plugin_entry_v2`) — additive
capabilities go through the extension-query mechanism instead, precisely
so that most future evolution doesn't require every existing plugin to
recompile.

## Alternatives considered

**Do nothing; keep the struct exactly as-is indefinitely.** Rejected:
too limiting for a project that has already named concrete future needs
(hot-reload state serialization) that don't fit in the current four
functions.

**Bump a whole new major ABI version (new struct, new entry-point symbol
name) for every future addition, however small.** Rejected as the
default: correct for genuinely breaking changes, but needlessly heavy for
additive ones — forcing every plugin author to recompile against a new
symbol name every time the host gains one new optional capability is a
worse experience than a queryable extension mechanism, for no added
safety.

**Version via `Cargo.toml`/crate semver alone, without an explicit
runtime field.** Rejected: Tier B plugins are not necessarily built with
Cargo or even with Rust (that's the entire point of a C ABI tier — see
[ADR 0002](0002-primary-language-selection.md) and
[ADR 0003](0003-plugin-and-modding-architecture.md)); a runtime-checked
field is the only mechanism that works regardless of what built the
plugin.

## Consequences

- A small, mechanical implementation task (add one field, add one
  function pointer, add one loader check) — not "major implementation
  code" by any reasonable measure — but not yet done as of this ADR;
  tracked in `docs/reviews/risk-register.md` (R-03) as the top
  recommended action before plugin-system work continues.
- `PluginError` gains a new variant (`UnsupportedAbiVersion`), which is
  itself a small, additive change to `engine/canary-plugin-api/src/error.rs`.
- Every future Tier B ABI addition should be evaluated against this ADR:
  "is this additive (goes through `get_extension`) or breaking (bumps
  `abi_version` and, if necessary, the entry-symbol name)?" — that
  question, asked and answered explicitly, is the actual deliverable of
  this ADR, more than the specific field names above.
- This ADR amends, rather than reverses, [ADR 0003](0003-plugin-and-modding-architecture.md);
  Tier B's fundamental trust model and C-ABI choice are unchanged.
