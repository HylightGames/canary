// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::path::PathBuf;

use thiserror::Error;

/// Errors from loading a plugin, either tier.
///
/// Per `docs/vision/design-philosophy.md`'s "subsystems bind through
/// interfaces, never leak a third-party type" principle, this enum
/// deliberately does **not** expose `libloading::Error`, `wasmtime::Error`,
/// or any other third-party type in its public fields, even though those
/// crates are what actually detect these failures underneath -- callers
/// of this crate's public API should never need to depend on them
/// directly just to match on why a load failed. The underlying error is
/// still available via `#[source]` / `std::error::Error::source()`, which
/// is how `{source}` renders it in the `Display` messages below, without
/// requiring the field's own type to be a third-party one.
#[derive(Debug, Error)]
pub enum PluginError {
    /// The dynamic library at `path` could not be opened (missing file,
    /// wrong architecture, unresolved transitive symbols, ...).
    #[error("failed to load native plugin library at `{path}`: {source}")]
    Load {
        /// The path that was passed to [`crate::NativePluginLoader::load`].
        path: PathBuf,
        /// The underlying loader error, boxed so this crate's public API
        /// doesn't expose which native-loading library produced it.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The library at `path` opened successfully but doesn't export the
    /// required [`crate::abi::ENTRY_SYMBOL`] symbol.
    #[error("native plugin at `{path}` does not export `canary_plugin_entry`: {source}")]
    MissingEntryPoint {
        /// The path that was passed to [`crate::NativePluginLoader::load`].
        path: PathBuf,
        /// The underlying symbol-lookup error, boxed for the same reason
        /// as [`PluginError::Load`]'s `source`.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The plugin at `path` declared an ABI version this host doesn't
    /// support (see `crate::abi::ABI_VERSION` and ADR 0009). Deliberately
    /// rejected rather than proceeding: once the version doesn't match,
    /// nothing else in the plugin's declared vtable can be trusted to be
    /// at the layout this host expects.
    #[error(
        "native plugin at `{path}` targets ABI version {found}, but this host requires version {expected}"
    )]
    UnsupportedAbiVersion {
        /// The path that was passed to [`crate::NativePluginLoader::load`].
        path: PathBuf,
        /// The ABI version the plugin declared.
        found: u32,
        /// The ABI version this host requires (`crate::abi::ABI_VERSION`).
        expected: u32,
    },

    /// The Tier A (WASM) engine or linker could not be constructed. Rare
    /// in practice — this indicates a fundamentally invalid Wasmtime
    /// [`wasmtime::Config`] or an internal bindings-setup failure, not
    /// anything about a specific plugin.
    #[error("failed to set up the Tier A (WASM) plugin engine: {source}")]
    WasmEngineSetup {
        /// The underlying Wasmtime error, boxed for the same reason as
        /// [`PluginError::Load`]'s `source`.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The file at `path` could not be parsed as a valid WASM Component
    /// Model artifact.
    #[error("failed to parse Tier A plugin component at `{path}`: {source}")]
    WasmParse {
        /// The path that was passed to [`crate::WasmPluginLoader::load`].
        path: PathBuf,
        /// The underlying Wasmtime parse error, boxed for the same
        /// reason as [`PluginError::Load`]'s `source`.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The component at `path` could not be instantiated. **This is also
    /// what a missing capability grant surfaces as**: a component whose
    /// world imports an interface this loader didn't link in (because
    /// the corresponding [`crate::Capability`] wasn't granted) fails
    /// here, at instantiation, with an unsatisfied-import error — before
    /// any of the component's own code runs. See
    /// [`crate::WasmPluginLoader::load`]'s doc comment for why that
    /// distinction (structural denial, not a rejected call) is
    /// load-bearing.
    #[error("failed to instantiate Tier A plugin component at `{path}`: {source}")]
    WasmInstantiate {
        /// The path that was passed to [`crate::WasmPluginLoader::load`].
        path: PathBuf,
        /// The underlying Wasmtime instantiation error, boxed for the
        /// same reason as [`PluginError::Load`]'s `source`.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
