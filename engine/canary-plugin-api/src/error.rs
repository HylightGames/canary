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
}
