use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::path::Path;

use libloading::{Library, Symbol};

use crate::abi;
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
        let library = unsafe { Library::new(path) }.map_err(|source| PluginError::Load {
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
            unsafe { library.get(ENTRY_SYMBOL) }.map_err(|source| {
                PluginError::MissingEntryPoint {
                    path: path.to_path_buf(),
                    source,
                }
            })?;

        let mut handle = MaybeUninit::<PluginHandle>::uninit();
        let handle_ptr = handle.as_mut_ptr();
        // SAFETY: `entry` was resolved from the required export above and
        // is being called with a valid, correctly-sized, writable
        // out-pointer for it to populate — which is exactly the contract
        // `PluginEntryFn` documents.
        unsafe { entry(handle_ptr) };

        // Read only `abi_version` via a raw pointer, before ever calling
        // `assume_init` on the whole struct. This matters: `assume_init`
        // asserts the *entire* value is validly initialized, which we
        // cannot yet claim for a plugin that might have written a
        // different, incompatible layout. Reading a `u32` through a raw
        // pointer computed with `addr_of!` doesn't require that — integers
        // have no invalid bit patterns, and `abi_version`'s position is a
        // permanent invariant of this ABI (see `crate::abi`'s module
        // docs), so this read is sound regardless of what the rest of the
        // struct contains.
        // SAFETY: `handle_ptr` points to `size_of::<PluginHandle>()` bytes
        // of memory this function allocated (via `MaybeUninit`), and
        // `abi_version`'s permanently-fixed position/type (see
        // `crate::abi`) makes this specific field access sound even
        // before the rest of the struct is known to be valid.
        let found_version = unsafe { std::ptr::addr_of!((*handle_ptr).abi_version).read() };
        if found_version != abi::ABI_VERSION {
            // Deliberately not calling `destroy` here: if the ABI version
            // doesn't match, the vtable's function pointers cannot be
            // trusted to be at the offsets this host expects, so no
            // cleanup call can be safely made. This is an inherent limit
            // of manual ABI versioning, not an oversight — plugins are
            // expected to declare a matching version specifically so this
            // path should not normally be reached. See ADR 0009.
            return Err(PluginError::UnsupportedAbiVersion {
                path: path.to_path_buf(),
                found: found_version,
                expected: abi::ABI_VERSION,
            });
        }

        // SAFETY: the version check above confirms this plugin was built
        // against a `PluginHandle` layout this host understands, and a
        // conforming `canary_plugin_entry` (per the ABI contract) fully
        // initializes every field of `*out` before returning — which we
        // have no way to verify beyond the version check, precisely why
        // this whole tier is trusted-only by design.
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
    unsafe extern "C" fn fake_get_extension(
        _context: *mut c_void,
        _name: *const c_char,
    ) -> *const c_void {
        std::ptr::null()
    }

    fn fake_handle() -> PluginHandle {
        PluginHandle {
            abi_version: abi::ABI_VERSION,
            context: std::ptr::null_mut(),
            vtable: PluginVTable {
                name: fake_name,
                on_load: fake_noop,
                on_unload: fake_noop,
                destroy: fake_destroy,
                get_extension: fake_get_extension,
            },
        }
    }

    /// Exercises `RawPlugin`'s vtable-calling and drop/destroy logic
    /// entirely in-process, with no real dynamic library involved — a
    /// cross-platform complement to the `cc`-compiled, real-`dlopen`
    /// integration test in `tests/native_loader.rs` (Linux-only, since it
    /// shells out to a C compiler).
    #[test]
    fn drop_calls_destroy_exactly_once_and_name_reads_through_the_vtable() {
        DESTROYED.store(false, Ordering::SeqCst);

        {
            let mut plugin = RawPlugin(fake_handle());
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

    /// `abi_version`'s position/type is a permanent invariant this whole
    /// versioning scheme depends on (see `crate::abi`'s module docs) --
    /// this test is a cheap, permanent guard against ever accidentally
    /// reordering it, since a byte-offset check is exactly the kind of
    /// thing a future refactor could silently break otherwise.
    #[test]
    fn abi_version_is_the_first_field_of_plugin_handle() {
        let handle = fake_handle();
        let handle_ptr: *const PluginHandle = &handle;
        // SAFETY: reading the address of a field of a live, valid,
        // stack-allocated `PluginHandle` we own -- purely a pointer-
        // arithmetic comparison, never dereferenced as anything but the
        // addresses being compared.
        let version_field_addr = unsafe { std::ptr::addr_of!((*handle_ptr).abi_version) } as usize;
        assert_eq!(
            version_field_addr, handle_ptr as usize,
            "abi_version must remain PluginHandle's first field -- see crate::abi's module docs"
        );
    }
}
