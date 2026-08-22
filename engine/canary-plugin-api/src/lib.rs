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
//! loader, and a first slice of the sandboxed (Tier A) loader.
//!
//! See `docs/architecture/plugin-system.md` for the full two-tier design.
//! This crate ships:
//!
//! - The [`Plugin`] trait every loaded plugin exposes to engine code,
//!   regardless of tier.
//! - The C-ABI types ([`abi::PluginVTable`], [`abi::PluginHandle`]) a Tier B
//!   native plugin must export, and [`NativePluginLoader`], a working
//!   loader for them.
//! - [`WasmPluginLoader`], a first slice of the Tier A (WASM Component
//!   Model) loader — see its module docs for exactly what it does and
//!   does not yet cover; it is not the full `v0.0.3` scope.
//! - [`Capability`], the capability declaration type. Tier A enforces it
//!   structurally (see [`WasmPluginLoader::load`]); Tier B does not yet
//!   — see [`Capability`]'s own docs.
//!
//! **Not yet implemented**: the full Tier A ECS data ABI, a resource
//! budget, and AOT compilation. See `docs/roadmap/v0.0.3-roadmap.md`.

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
pub use tier_a::{WasmComponentPlugin, WasmPluginLoader};
