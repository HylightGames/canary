// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

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
//!
//! # ABI versioning (see ADR 0009)
//!
//! [`PluginHandle::abi_version`] is a permanent, load-bearing invariant of
//! this ABI: **it is always the first field of `PluginHandle`, always a
//! `u32`, for every version of this ABI, forever.** That invariant is what
//! lets [`crate::NativePluginLoader::load`] safely read a plugin's
//! declared ABI version and reject a mismatch *before* trusting anything
//! else the plugin wrote — including before assuming the rest of the
//! struct was even validly initialized. Do not reorder this field, change
//! its type, or make its meaning conditional on anything else in this
//! struct; doing so would defeat the entire mechanism this version exists
//! to provide.
//!
//! Only genuinely breaking changes to the four core lifecycle functions
//! should ever bump [`ABI_VERSION`] (and, if the break is severe enough,
//! change [`ENTRY_SYMBOL`] itself). Purely additive capabilities — state
//! serialization for hot reload, capability introspection, anything not
//! yet designed — should go through [`PluginVTable::get_extension`]
//! instead, which lets this ABI grow without ever changing `PluginHandle`
//! or `PluginVTable`'s layout again.

use std::os::raw::{c_char, c_void};

/// The Tier B ABI version this crate implements. A plugin declares which
/// version it was built against via [`PluginHandle::abi_version`]; the
/// loader rejects a mismatch rather than risk misreading memory.
pub const ABI_VERSION: u32 = 1;

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
    /// Extension-query hook (ADR 0009): lets the host ask for optional,
    /// named capabilities defined after this ABI shipped, without ever
    /// changing this struct's layout again. `name` is a null-terminated
    /// UTF-8 string; implementations must return null for any name they
    /// don't recognize. No extensions are defined as of `ABI_VERSION = 1`
    /// — every current plugin's `get_extension` is expected to always
    /// return null — but the slot exists now so adding the first real
    /// extension later doesn't require every existing plugin to
    /// recompile against a new struct layout.
    pub get_extension:
        unsafe extern "C" fn(context: *mut c_void, name: *const c_char) -> *const c_void,
}

/// What a Tier B plugin's entry point produces: the ABI version it was
/// built against, an opaque context pointer (meaningful only to the
/// plugin's own functions; the host never dereferences it directly), and
/// the vtable of functions that operate on it.
///
/// See the module-level docs for why `abi_version`'s position and type
/// are a permanent invariant, not just today's layout.
#[repr(C)]
pub struct PluginHandle {
    /// The ABI version this plugin was built against. See [`ABI_VERSION`]
    /// and the module-level docs — this field's position is permanently
    /// fixed across every version of this ABI.
    pub abi_version: u32,
    /// Opaque, plugin-owned state. May be null if the plugin needs no
    /// per-instance state.
    pub context: *mut c_void,
    /// The function table operating on `context`. Not safe to read or
    /// call through unless `abi_version` has already been confirmed to
    /// match [`ABI_VERSION`] — see [`crate::NativePluginLoader::load`].
    pub vtable: PluginVTable,
}

/// The symbol name every Tier B plugin must export.
pub const ENTRY_SYMBOL: &[u8] = b"canary_plugin_entry";

/// The signature every Tier B plugin exports under [`ENTRY_SYMBOL`]:
/// fills `*out` with the plugin's declared ABI version, context pointer,
/// and vtable. See the module docs for why this uses an out-parameter
/// instead of returning [`PluginHandle`] by value.
pub type PluginEntryFn = unsafe extern "C" fn(out: *mut PluginHandle);
