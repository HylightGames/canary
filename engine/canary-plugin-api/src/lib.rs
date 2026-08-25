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

pub mod abi;
mod capability;
mod component_value;
mod error;
mod loader;
mod plugin;
mod tier_a;

pub use capability::Capability;
pub use component_value::{
    CodecRegistry, ComponentValue, ComponentValueCodec, ComponentValueError, PrimitiveValue,
};
pub use error::PluginError;
pub use loader::NativePluginLoader;
pub use plugin::Plugin;
pub use tier_a::{ResourceBudget, WasmComponentPlugin, WasmPluginLoader};
