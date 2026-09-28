// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Tier A: sandboxed WASM Component Model plugin loading. See
//! `docs/architecture/plugin-system.md#tier-a--sandboxed-wasm-component-model`
//! and `docs/roadmap/v0.0.3-roadmap.md`.
//!
//! What this proves, end to end, against a real Wasmtime engine:
//!
//! - Loading and instantiating a real WASM Component Model artifact,
//!   either freshly compiled ([`crate::WasmPluginLoader::load`]) or a
//!   precompiled (AOT) artifact deserialized back in — see the AOT
//!   tests for the full round trip.
//! - The [`crate::Plugin`] lifecycle (`on_load`/`on_unload`) working through a
//!   component, not just a native dylib.
//! - **Structural** capability enforcement:
//!   [`crate::WasmPluginLoader::load`] only links a capability's host
//!   functions into the instance's [`wasmtime::component::Linker`] when
//!   that capability was actually granted — proven independently for
//!   both `ecs-read`/[`crate::Capability::ReadEcsWorld`] and
//!   `ecs-write`/[`crate::Capability::WriteEcsWorld`], since capability
//!   gating happens per-interface, not per-grant. A component whose
//!   world imports an ungranted capability's interface has nothing to
//!   link against and fails at *instantiation* — before any of its own
//!   code runs — not merely a call that gets rejected.
//! - **A resource budget** ([`ResourceBudget`]), genuinely separate from
//!   capability-based authority: a memory limit (a `memory.grow` past it
//!   fails, doesn't trap — see the memory-budget test) and a fuel
//!   execution budget (an infinite loop traps once it's exhausted — see
//!   the fuel-budget test), both applied to every instance a given
//!   loader creates, not opt-in.
//! - The full first-cut ECS data ABI: `get`/`set`/`has-component`/
//!   `is-valid-entity`, `SCHEMA_ID`-addressed through
//!   [`canary_ecs::World::type_id_for_schema`] (identity) and
//!   [`crate::component_value::CodecRegistry`] (representation) —
//!   see `crate::component_value`'s module docs for why those are two
//!   different mechanisms.
//!
//! See the tests for all of the above proven directly, not merely
//! asserted in this comment.
//!
//! What it does **not** yet cover — real, separately scoped work, not
//! an oversight:
//!
//! - **Anything beyond the two lifecycle callbacks.** Scoped access
//!   to the running [`canary_ecs::World`] exists —
//!   [`WasmComponentPlugin::call_scoped`] loans the active world for
//!   exactly one guest call (ownership loan, no borrow across
//!   `Func::call`, nested loans refused) and
//!   [`crate::WasmPluginLoader::load_scoped`] is the fallible seam for
//!   it — but only for `on_load`/`on_unload`
//!   ([`crate::PluginPhase`]). No per-frame plugin hook, no
//!   editor-panel API.
//!
//! The legacy [`crate::WasmPluginLoader::load`] path (a separately owned
//! `World` moved into the instance) remains for single-owner tests
//! and tooling; it is not the active-game-world path.
//!
//! Layout: one submodule per responsibility — [`budget`](self::budget)
//! for the resource type, [`codec`](self::codec) for the WIT conversion
//! layer, [`slot`](self::slot) for the host slot and grant-checked
//! bodies, [`instance`](self::instance) for the loaded plugin handle,
//! [`wasm_loader`](self::wasm_loader) for construction and linking.
//! The names `lib.rs` re-exports stay `pub`; everything else the
//! submodules share is `pub(crate)` so the test module keeps working
//! unchanged.

mod budget;
mod codec;
mod instance;
mod slot;
mod wasm_loader;

pub use budget::ResourceBudget;
pub use instance::WasmComponentPlugin;
pub use slot::ScopedGrant;
pub use wasm_loader::{PluginOutcome, WasmPluginLoader};

// Re-exported for the test module's `use super::*;` — the production
// submodules import each other by explicit path instead. Only the two
// modules with items the `pub use` list above does not already cover
// need a glob; the rest would be shadowed-unused.
#[cfg(test)]
pub(crate) use codec::*;
#[cfg(test)]
pub(crate) use slot::*;

#[cfg(test)]
mod tests;
