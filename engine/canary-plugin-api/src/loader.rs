use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::path::Path;

use libloading::{Library, Symbol};

use crate::abi::{PluginEntryFn, PluginHandle, ENTRY_SYMBOL};
use crate::error::PluginError;
use crate::plugin::Plugin;

/// A thin safe wrapper around a raw [`PluginHandle`]: responsible only for
/// calling through the vtable, and for calling `destroy` exactly once, on
/// drop. Deliberately doesn't own a [`Library`] itself, so it can be
/// exercised in tests without a real dynamic library — see the `tests`
/// module below. [`LoadedNativePlugin`] adds `Library` ownership on top of
/// this.
struct RawPlugin(PluginHandle);

impl Plugin for RawPlugin {
    fn name(&self) -> String {
        // SAFETY: `self.0` was populated either by a Tier B plugin's own
        // `canary_plugin_entry` (via `NativePluginLoader::load`) or, in
        // tests, by a fixture upholding the same contract: `vtable.name`
        // is a valid function pointer taking `context`, and (per the ABI
        // contract in `crate::abi`) returns either null or a
        // null-terminated, static-duration, UTF-8 C string.
        unsafe {
            let ptr = (self.0.vtable.name)(self.0.context);
            if ptr.is_null() {
                String::from("<unnamed native plugin>")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }

    fn on_load(&mut self) {
        // SAFETY: see `name` above.
        unsafe { (self.0.vtable.on_load)(self.0.context) }
    }

    fn on_unload(&mut self) {
        // SAFETY: see `name` above.
        unsafe { (self.0.vtable.on_unload)(self.0.context) }
    }
}

impl Drop for RawPlugin {
    fn drop(&mut self) {
        // SAFETY: the ABI contract in `crate::abi` requires `destroy` to
        // be safe to call exactly once, after which `context` must not be
        // used again — which holds here, since this is `RawPlugin`'s
        // `Drop` impl and nothing can observe `self.0` afterward.
        unsafe { (self.0.vtable.destroy)(self.0.context) }
    }
}

/// A loaded Tier B native plugin.
///
/// Owns the [`Library`] so it isn't unloaded while the plugin is still in
/// use. Field order matters here: `raw` must drop (calling the plugin's
/// `destroy`) *before* `_library` drops (unmapping the code `destroy`
/// itself lives in) — Rust drops struct fields in declaration order, so
/// `raw` being declared first is load-bearing, not incidental.
pub struct LoadedNativePlugin {
    raw: RawPlugin,
    _library: Library,
}

impl Plugin for LoadedNativePlugin {
    fn name(&self) -> String {
        self.raw.name()
    }

    fn on_load(&mut self) {
        self.raw.on_load()
    }

    fn on_unload(&mut self) {
        self.raw.on_unload()
    }
}

/// Loads Tier B native plugins: dynamic libraries exporting
/// `canary_plugin_entry` per the ABI contract in [`crate::abi`]. See
/// `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`.
///
/// **This tier is trusted by design: no sandboxing is performed here.**
/// Only load native plugins you trust as much as the host process itself
/// — see the module-level warning in `docs/architecture/plugin-system.md`.
#[derive(Debug, Default)]
pub struct NativePluginLoader;

impl NativePluginLoader {
    /// Creates a loader. Stateless today; a constructor exists so callers
    /// don't depend on that remaining true (e.g. once capability
    /// enforcement or plugin-directory bookkeeping is added).
    pub fn new() -> Self {
        Self
    }

    /// Loads the native plugin at `path`, calling its `canary_plugin_entry`
    /// export.
    ///
    /// # Safety (informal — see the `SAFETY` comments in this function's
    /// body for the formal per-block reasoning)
    ///
    /// Loading an arbitrary dynamic library and running its exported code
    /// is inherently unsafe at the language level: `path` must genuinely
    /// implement the ABI contract in [`crate::abi`], and the plugin's code
    /// is trusted with everything the host process itself can do. This is
    /// a project-level trust decision (Tier B, by design), not something
    /// this function can verify.
    pub fn load(&self, path: impl AsRef<Path>) -> Result<LoadedNativePlugin, PluginError> {
        let path = path.as_ref();

        // SAFETY: `Library::new`'s only inherent unsafety is that loading
        // a library runs arbitrary initializer code — which is exactly
        // the trust boundary Tier B is defined around (see the doc
        // comment above and `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`).
        let library = unsafe { Library::new(path) }
            .map_err(|source| PluginError::Load {
                path: path.to_path_buf(),
                source,
            })?;

        // SAFETY: `ENTRY_SYMBOL` is the documented, required export name
        // for a Tier B plugin (see `crate::abi`); if this symbol is
        // present at all, its signature is part of the same ABI contract
        // this loader and a conforming plugin both implement. `library`
        // outlives `entry` here (it isn't dropped until the end of this
        // function, well after `entry` is last used).
        let entry: Symbol<PluginEntryFn> =
            unsafe { library.get(ENTRY_SYMBOL) }.map_err(|source| PluginError::MissingEntryPoint {
                path: path.to_path_buf(),
                source,
            })?;

        let mut handle = MaybeUninit::<PluginHandle>::uninit();
        // SAFETY: `entry` was resolved from the required export above and
        // is being called with a valid, correctly-sized, writable
        // out-pointer for it to populate — which is exactly the contract
        // `PluginEntryFn` documents.
        unsafe { entry(handle.as_mut_ptr()) };
        // SAFETY: a conforming `canary_plugin_entry` (per the ABI
        // contract) fully initializes every field of `*out` before
        // returning; we have no way to verify that mechanically, which is
        // precisely why this whole tier is trusted-only by design.
        let handle = unsafe { handle.assume_init() };

        Ok(LoadedNativePlugin {
            raw: RawPlugin(handle),
            _library: library,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::PluginVTable;
    use std::os::raw::{c_char, c_void};
    use std::sync::atomic::{AtomicBool, Ordering};

    static DESTROYED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn fake_name(_context: *mut c_void) -> *const c_char {
        b"fake-plugin\0".as_ptr() as *const c_char
    }
    unsafe extern "C" fn fake_noop(_context: *mut c_void) {}
    unsafe extern "C" fn fake_destroy(_context: *mut c_void) {
        DESTROYED.store(true, Ordering::SeqCst);
    }

    /// Exercises `RawPlugin`'s vtable-calling and drop/destroy logic
    /// entirely in-process, with no real dynamic library involved — a
    /// cross-platform complement to the `cc`-compiled, real-`dlopen`
    /// integration test in `tests/native_loader.rs` (Linux-only, since it
    /// shells out to a C compiler).
    #[test]
    fn drop_calls_destroy_exactly_once_and_name_reads_through_the_vtable() {
        DESTROYED.store(false, Ordering::SeqCst);
        let handle = PluginHandle {
            context: std::ptr::null_mut(),
            vtable: PluginVTable {
                name: fake_name,
                on_load: fake_noop,
                on_unload: fake_noop,
                destroy: fake_destroy,
            },
        };

        {
            let mut plugin = RawPlugin(handle);
            assert_eq!(Plugin::name(&plugin), "fake-plugin");
            plugin.on_load();
            plugin.on_unload();
            assert!(
                !DESTROYED.load(Ordering::SeqCst),
                "destroy must not run before drop"
            );
        }

        assert!(
            DESTROYED.load(Ordering::SeqCst),
            "destroy must run exactly once, on drop"
        );
    }

    #[test]
    fn loading_a_missing_path_returns_a_load_error_not_a_panic() {
        let loader = NativePluginLoader::new();
        let result = loader.load("/definitely/does/not/exist/canary-fixture.so");
        assert!(matches!(result, Err(PluginError::Load { .. })));
    }
}
