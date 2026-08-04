use std::path::PathBuf;

use thiserror::Error;

/// Errors from loading a Tier B native plugin.
#[derive(Debug, Error)]
pub enum PluginError {
    /// The dynamic library at `path` could not be opened (missing file,
    /// wrong architecture, unresolved transitive symbols, ...).
    #[error("failed to load native plugin library at `{path}`: {source}")]
    Load {
        /// The path that was passed to [`crate::NativePluginLoader::load`].
        path: PathBuf,
        /// The underlying `libloading` error.
        #[source]
        source: libloading::Error,
    },

    /// The library at `path` opened successfully but doesn't export the
    /// required [`crate::abi::ENTRY_SYMBOL`] symbol.
    #[error("native plugin at `{path}` does not export `canary_plugin_entry`: {source}")]
    MissingEntryPoint {
        /// The path that was passed to [`crate::NativePluginLoader::load`].
        path: PathBuf,
        /// The underlying `libloading` symbol-lookup error.
        #[source]
        source: libloading::Error,
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
}
