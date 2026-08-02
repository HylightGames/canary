//! Canary Engine plugin system: trait surface and the native (Tier B)
//! loader.
//!
//! See `docs/architecture/plugin-system.md` for the full two-tier design.
//! This crate ships:
//!
//! - The [`Plugin`] trait every loaded plugin exposes to engine code,
//!   regardless of tier.
//! - The C-ABI types ([`abi::PluginVTable`], [`abi::PluginHandle`]) a Tier B
//!   native plugin must export, and [`NativePluginLoader`], a working
//!   loader for them.
//! - [`Capability`], the (currently advisory-only) capability declaration
//!   type — see its own docs for why it isn't enforced yet.
//!
//! **Not yet implemented**: the sandboxed WASM (Tier A) loader. See
//! `docs/roadmap/v0.0.1-roadmap.md`.

pub mod abi;
mod capability;
mod error;
mod loader;
mod plugin;

pub use capability::Capability;
pub use error::PluginError;
pub use loader::NativePluginLoader;
pub use plugin::Plugin;
