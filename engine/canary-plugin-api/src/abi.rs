//! The C-ABI-stable plugin interface a Tier B native plugin exports.
//!
//! This uses a plain `#[repr(C)]` struct of function pointers (a manual
//! vtable) rather than a Rust `dyn Trait` object on purpose: `dyn Trait`
//! fat-pointer layout is not a stable ABI guaranteed to match across
//! different rustc versions, let alone across languages — which would
//! quietly break Tier B's "any language, any compatible compiler" promise.
//! See `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`.
//!
//! The entry point ([`PluginEntryFn`]) fills an out-parameter rather than
//! returning [`PluginHandle`] by value, specifically to avoid relying on
//! by-value struct-return calling-convention details across compilers and
//! platforms — writing through a pointer is unambiguous everywhere `extern
//! "C"` is meaningful at all.

use std::os::raw::{c_char, c_void};

/// The function-pointer table a Tier B plugin provides. Every function
/// takes the plugin's own opaque `context` pointer (see [`PluginHandle`])
/// as its argument, in the usual C "manual `this`" style.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginVTable {
    /// Returns a null-terminated, UTF-8, static-duration C string naming
    /// the plugin. Must not return null except as a last resort (callers
    /// treat null as "unnamed plugin" rather than erroring).
    pub name: unsafe extern "C" fn(context: *mut c_void) -> *const c_char,
    /// Called once, after the plugin is loaded.
    pub on_load: unsafe extern "C" fn(context: *mut c_void),
    /// Called once, before the plugin is unloaded.
    pub on_unload: unsafe extern "C" fn(context: *mut c_void),
    /// Called exactly once, when the plugin is being unloaded, after
    /// which `context` must not be used again. Responsible for freeing
    /// whatever `context` points to, if anything.
    pub destroy: unsafe extern "C" fn(context: *mut c_void),
}

/// What a Tier B plugin's entry point produces: an opaque context pointer
/// (meaningful only to the plugin's own functions; the host never
/// dereferences it directly) plus the vtable of functions that operate on
/// it.
#[repr(C)]
pub struct PluginHandle {
    /// Opaque, plugin-owned state. May be null if the plugin needs no
    /// per-instance state.
    pub context: *mut c_void,
    /// The function table operating on `context`.
    pub vtable: PluginVTable,
}

/// The symbol name every Tier B plugin must export.
pub const ENTRY_SYMBOL: &[u8] = b"canary_plugin_entry";

/// The signature every Tier B plugin exports under [`ENTRY_SYMBOL`]:
/// fills `*out` with the plugin's context pointer and vtable. See the
/// module docs for why this uses an out-parameter instead of returning
/// [`PluginHandle`] by value.
pub type PluginEntryFn = unsafe extern "C" fn(out: *mut PluginHandle);
