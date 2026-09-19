// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine plugin system: trait surface, the native (Tier B)
//! loader, and the sandboxed (Tier A) loader.
//!
//! See `docs/architecture/plugin-system.md` for the full two-tier design.
//! This crate ships:
//!
//! - The [`Plugin`] trait every loaded plugin exposes to engine code,
//!   regardless of tier.
//! - The C-ABI types ([`abi::PluginVTable`], [`abi::PluginHandle`]) a Tier B
//!   native plugin must export, and [`NativePluginLoader`], a working
//!   loader for them.
//! - [`WasmPluginLoader`], the Tier A (WASM Component Model) loader:
//!   component loading (fresh or AOT-precompiled), the [`Plugin`]
//!   lifecycle through a component, a [`ResourceBudget`]
//!   (memory limit, fuel execution budget) applied to every instance,
//!   and the first-cut ECS data ABI ([`ComponentValue`]/
//!   [`ComponentValueCodec`]/[`CodecRegistry`], `get`/`set`/
//!   `has-component`/`is-valid-entity`) — see its module docs for
//!   exactly what's proven and the one piece (safely lending an
//!   already-running [`canary_ecs::World`] scoped access) that's still
//!   more open design question than implementation task.
//! - [`Capability`], the capability declaration type — structurally
//!   enforced by Tier A for `ReadEcsWorld`/`WriteEcsWorld`, advisory
//!   everywhere else (see [`Capability`]'s own docs for the precise,
//!   per-variant breakdown).
//!
//! See `docs/roadmap/v0.0.3-roadmap.md` for what's explicitly out of
//! scope even now: a plugin manifest format, Tier B signing, and safe
//! hot-unloading with full resource reclamation.
//!
//! **Host-only vs. shared surface:** [`abi`], [`Capability`],
//! [`ComponentValue`]/[`ComponentValueCodec`]/[`CodecRegistry`],
//! [`PluginError`], and [`Plugin`] itself have no host-specific
//! dependency and compile for `wasm32-wasip2` (checked in CI —
//! `cargo check -p canary-plugin-api --target wasm32-wasip2`) alongside
//! their native build, since a plugin's own build may want these types
//! directly rather than redefining them against the WIT world. The
//! *loaders* ([`NativePluginLoader`], [`WasmPluginLoader`],
//! [`WasmComponentPlugin`]) are host-only — `libloading` and `wasmtime`
//! are engine-side-only dependencies that never belong in a
//! `wasm32-wasip2` build (`wasmtime` is the *host* runtime that loads
//! and executes a wasm component; it has no meaning running as one), so
//! `mod loader` and `mod tier_a` are `cfg(not(target_family =
//! "wasm"))`-gated below. (`wasmtime-wasi` is deliberately *not* a
//! dependency at all — see this crate's `Cargo.toml`.)

pub mod abi;
mod capability;
mod component_value;
mod error;
#[cfg(not(target_family = "wasm"))]
mod loader;
mod plugin;
#[cfg(not(target_family = "wasm"))]
mod tier_a;

pub use capability::Capability;
pub use component_value::{
    CodecRegistry, ComponentValue, ComponentValueCodec, ComponentValueError, PrimitiveValue,
};
pub use error::PluginError;
#[cfg(not(target_family = "wasm"))]
pub use loader::NativePluginLoader;
pub use plugin::Plugin;
#[cfg(not(target_family = "wasm"))]
pub use tier_a::{ResourceBudget, WasmComponentPlugin, WasmPluginLoader};
