// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::collections::BTreeSet;
use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::path::{Component, Path, PathBuf};

use libloading::{Library, Symbol};

use crate::abi;
use crate::abi::{PluginEntryFn, PluginHandle, ENTRY_SYMBOL};
use crate::error::{EntryRejection, PluginError};
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

/// The one directory native plugins may be loaded from.
///
/// Constructing a `PluginRoot` canonicalizes the directory exactly once,
/// so later entry resolution is checked against a stable absolute path
/// rather than a relative path whose meaning could change with the working
/// directory. [`NativePluginLoader`] requires one: there is deliberately
/// no constructor that accepts an arbitrary absolute library path.
///
/// Canonicalizing the root also resolves any symlinks *in the root itself*;
/// a caller that points this at a temporary symlink is trusting whoever
/// controls that symlink, the same as trusting the directory's owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRoot {
    canonical: PathBuf,
}

impl PluginRoot {
    /// Creates a plugin root from `root`, canonicalizing it once.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::PluginRootUnavailable`] if `root` cannot be
    /// canonicalized (missing, permission denied, ...), and
    /// [`PluginError::PluginRootNotDirectory`] if it canonicalizes to
    /// something other than a directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, PluginError> {
        let root = root.as_ref();
        let canonical =
            std::fs::canonicalize(root).map_err(|source| PluginError::PluginRootUnavailable {
                path: root.to_path_buf(),
                source: Box::new(source),
            })?;

        let metadata =
            std::fs::metadata(&canonical).map_err(|source| PluginError::PluginRootUnavailable {
                path: canonical.clone(),
                source: Box::new(source),
            })?;
        if !metadata.is_dir() {
            return Err(PluginError::PluginRootNotDirectory { path: canonical });
        }

        Ok(Self { canonical })
    }

    /// The canonical directory every entry resolves under.
    pub fn path(&self) -> &Path {
        &self.canonical
    }

    /// Resolves a relative plugin `entry` against this root, refusing
    /// anything that is absolute, contains `..`, escapes the root after
    /// symlink resolution, or is not a regular file.
    ///
    /// The entry is validated lexically first, then canonicalized; the
    /// canonical result must still be a descendant of this root. This
    /// catches symlinks inside the root that point outside it, not just
    /// textual traversal.
    ///
    /// There remains an unavoidable TOCTOU window between this check and
    /// `dlopen`: `libloading` opens the path by name, so a symlink swapped
    /// after canonicalization could redirect the open. Plugins must
    /// therefore live in a directory whose write access is at least as
    /// trusted as the plugins themselves.
    ///
    /// # Errors
    ///
    /// [`PluginError::EntryRejected`] for lexical escapes, symlink
    /// escapes, and non-file entries; [`PluginError::EntryResolve`] if a
    /// lexically safe entry cannot be canonicalized.
    pub fn resolve(&self, entry: impl AsRef<Path>) -> Result<PathBuf, PluginError> {
        let entry = entry.as_ref();
        let normalized = normalize_entry(entry).map_err(|reason| PluginError::EntryRejected {
            entry: entry.to_path_buf(),
            reason,
        })?;

        let candidate = self.canonical.join(&normalized);
        let resolved =
            std::fs::canonicalize(&candidate).map_err(|source| PluginError::EntryResolve {
                entry: entry.to_path_buf(),
                root: self.canonical.clone(),
                source: Box::new(source),
            })?;

        if !resolved.starts_with(&self.canonical) {
            return Err(PluginError::EntryRejected {
                entry: entry.to_path_buf(),
                reason: EntryRejection::EscapesRoot,
            });
        }

        let metadata =
            std::fs::metadata(&resolved).map_err(|source| PluginError::EntryResolve {
                entry: entry.to_path_buf(),
                root: self.canonical.clone(),
                source: Box::new(source),
            })?;
        if !metadata.is_file() {
            return Err(PluginError::EntryRejected {
                entry: entry.to_path_buf(),
                reason: EntryRejection::NotAFile,
            });
        }

        Ok(resolved)
    }
}

/// Lexically validates a relative entry name and drops `.` components.
///
/// Filesystem-independent by design: `..` is refused before a path is
/// joined or canonicalized, so traversal never depends on how a particular
/// platform's canonicalizer treats it.
fn normalize_entry(entry: &Path) -> Result<PathBuf, EntryRejection> {
    let mut normalized = PathBuf::new();
    for component in entry.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err(EntryRejection::ParentTraversal),
            Component::RootDir | Component::Prefix(_) => return Err(EntryRejection::Absolute),
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(EntryRejection::NotAFile);
    }

    Ok(normalized)
}

/// An optional operator-controlled allowlist of native plugin entries.
///
/// The manifest is a UTF-8 text file with one entry per line, each relative
/// to the [`PluginRoot`]; blank lines and lines whose first non-whitespace
/// character is `#` are ignored. Entries are compared literally (after
/// dropping `.` components), so the manifest names exactly the strings a
/// caller passes to [`NativePluginLoader::load`]. An empty manifest permits
/// nothing — the allowlist fails closed.
///
/// **Deliberately not TOML.** The full plugin manifest (metadata, version
/// ranges, declared capabilities) is still open design (risk register
/// R-08), and adding a TOML parser solely for a one-key allowlist would
/// put a new security-sensitive parser in the trust path for no benefit.
/// A line-oriented format is small enough to audit, and the `.allowlist`
/// suffix avoids implying TOML syntax the parser does not implement.
#[derive(Debug, Clone)]
pub struct PluginAllowlist {
    manifest: PathBuf,
    entries: BTreeSet<PathBuf>,
}

impl PluginAllowlist {
    /// Reads an allowlist manifest from `manifest`.
    ///
    /// # Errors
    ///
    /// [`PluginError::ManifestIo`] if the file cannot be read as UTF-8,
    /// and [`PluginError::ManifestEntry`] if any entry is absolute,
    /// contains `..`, or is empty.
    pub fn from_file(manifest: impl AsRef<Path>) -> Result<Self, PluginError> {
        let manifest = manifest.as_ref();
        let contents =
            std::fs::read_to_string(manifest).map_err(|source| PluginError::ManifestIo {
                path: manifest.to_path_buf(),
                source: Box::new(source),
            })?;

        let mut entries = BTreeSet::new();
        for (index, raw) in contents.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let entry = Path::new(line);
            let normalized =
                normalize_entry(entry).map_err(|reason| PluginError::ManifestEntry {
                    manifest: manifest.to_path_buf(),
                    line: index + 1,
                    entry: entry.to_path_buf(),
                    reason,
                })?;
            entries.insert(normalized);
        }

        Ok(Self {
            manifest: manifest.to_path_buf(),
            entries,
        })
    }

    /// The manifest path this allowlist was read from, for diagnostics.
    pub fn manifest_path(&self) -> &Path {
        &self.manifest
    }

    /// The normalized entries this allowlist permits, in sorted order.
    pub fn entries(&self) -> impl Iterator<Item = &Path> {
        self.entries.iter().map(PathBuf::as_path)
    }
}

/// Loads Tier B native plugins: dynamic libraries exporting
/// `canary_plugin_entry` per the ABI contract in [`crate::abi`]. See
/// `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`.
///
/// **This tier is trusted by design: path confinement is not sandboxing.**
/// A loader is bound to a [`PluginRoot`] and only opens relative entries
/// that resolve to regular files inside that root, optionally restricted
/// further by a [`PluginAllowlist`] manifest. Once opened, plugin code runs
/// with everything the host process itself can do; only load native plugins
/// you trust as much as the host process. See the module-level warning in
/// `docs/architecture/plugin-system.md`.
#[derive(Debug)]
pub struct NativePluginLoader {
    root: PluginRoot,
    allowlist: Option<PluginAllowlist>,
}

impl NativePluginLoader {
    /// Creates a loader confined to `root`, permitting any regular file
    /// named by a safe relative entry under it.
    pub fn new(root: PluginRoot) -> Self {
        Self {
            root,
            allowlist: None,
        }
    }

    /// Creates a loader confined to `root` and restricted to the entries
    /// named by `allowlist`.
    pub fn with_allowlist(root: PluginRoot, allowlist: PluginAllowlist) -> Self {
        Self {
            root,
            allowlist: Some(allowlist),
        }
    }

    /// Creates a loader confined to `root`, reading its allowlist from
    /// `manifest` (see [`PluginAllowlist::from_file`]).
    ///
    /// # Errors
    ///
    /// Propagates [`PluginAllowlist::from_file`]'s errors.
    pub fn with_manifest(
        root: PluginRoot,
        manifest: impl AsRef<Path>,
    ) -> Result<Self, PluginError> {
        Ok(Self::with_allowlist(
            root,
            PluginAllowlist::from_file(manifest)?,
        ))
    }

    /// The root every entry is resolved under.
    pub fn root(&self) -> &PluginRoot {
        &self.root
    }

    /// Loads the native plugin named by `entry`, resolved against this
    /// loader's [`PluginRoot`], and calls its `canary_plugin_entry` export.
    ///
    /// `entry` must be relative: absolute paths, `..` components, symlinks
    /// that resolve outside the root, and non-file targets are refused
    /// before any dynamic library is opened. If an allowlist is configured,
    /// an entry not named by it is refused before filesystem resolution
    /// too, so a denial does not reveal whether the file exists.
    ///
    /// # Safety (informal — see the `SAFETY` comments in this function's
    /// body for the formal per-block reasoning)
    ///
    /// Loading a dynamic library and running its exported code is
    /// inherently unsafe at the language level: the library must genuinely
    /// implement the ABI contract in [`crate::abi`], and the plugin's code
    /// is trusted with everything the host process itself can do. Path
    /// confinement narrows *which* file is opened; it cannot verify that
    /// the file is well-behaved. This is a project-level trust decision
    /// (Tier B, by design), not something this function can verify.
    pub fn load(&self, entry: impl AsRef<Path>) -> Result<LoadedNativePlugin, PluginError> {
        let entry = entry.as_ref();

        if let Some(allowlist) = &self.allowlist {
            let normalized =
                normalize_entry(entry).map_err(|reason| PluginError::EntryRejected {
                    entry: entry.to_path_buf(),
                    reason,
                })?;
            if !allowlist.entries.contains(&normalized) {
                return Err(PluginError::NotAllowlisted {
                    entry: normalized,
                    manifest: allowlist.manifest.clone(),
                });
            }
        }

        let path = self.root.resolve(entry)?;

        // SAFETY: `Library::new`'s only inherent unsafety is that loading
        // a library runs arbitrary initializer code — which is exactly
        // the trust boundary Tier B is defined around (see the doc
        // comment above and `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`).
        // `path` is the canonical result of `PluginRoot::resolve`, so it
        // is a regular file inside the operator-configured root.
        let library = unsafe { Library::new(&path) }.map_err(|source| PluginError::Load {
            path: path.clone(),
            source: Box::new(source),
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
                    source: Box::new(source),
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
        c"fake-plugin".as_ptr()
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

    /// Creates a unique temporary directory for one test. Tests run in
    /// parallel, so the label keeps directories from colliding; a stale
    /// directory from an earlier failed run is removed first.
    fn temp_dir(label: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("canary-plugin-api-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create a temporary directory");
        path
    }

    fn loader_for(root: &Path) -> NativePluginLoader {
        NativePluginLoader::new(PluginRoot::new(root).expect("temporary root must canonicalize"))
    }

    #[test]
    fn plugin_root_must_be_an_existing_directory() {
        let missing = temp_dir("missing-root").join("does-not-exist");
        match PluginRoot::new(&missing) {
            Err(PluginError::PluginRootUnavailable { .. }) => {}
            other => panic!("expected PluginRootUnavailable, got {other:?}"),
        }

        let file = temp_dir("file-root").join("not-a-directory");
        std::fs::write(&file, b"not a directory").expect("failed to write the fixture file");
        match PluginRoot::new(&file) {
            Err(PluginError::PluginRootNotDirectory { .. }) => {}
            other => panic!("expected PluginRootNotDirectory, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(file.parent().expect("temp file has a parent"));
    }

    #[test]
    fn rejects_absolute_entry_names() {
        let root = temp_dir("absolute");
        let plugin_root = PluginRoot::new(&root).expect("temporary root must canonicalize");

        match plugin_root.resolve("/definitely/outside/canary-fixture.so") {
            Err(PluginError::EntryRejected {
                reason: EntryRejection::Absolute,
                ..
            }) => {}
            other => panic!("expected an absolute-path rejection, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_parent_traversal_entry_names() {
        let root = temp_dir("parent");
        let plugin_root = PluginRoot::new(&root).expect("temporary root must canonicalize");

        for entry in ["../outside.so", "sub/../../outside.so"] {
            match plugin_root.resolve(entry) {
                Err(PluginError::EntryRejected {
                    reason: EntryRejection::ParentTraversal,
                    ..
                }) => {}
                other => panic!("expected `{entry}` to be rejected, got {other:?}"),
            }
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_resolving_outside_the_root() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink-root");
        let outside = temp_dir("symlink-outside");
        let target = outside.join("outside.so");
        std::fs::write(&target, b"not a real library").expect("failed to write the fixture file");
        symlink(&target, root.join("escape.so")).expect("failed to create the escaping symlink");

        let plugin_root = PluginRoot::new(&root).expect("temporary root must canonicalize");
        match plugin_root.resolve("escape.so") {
            Err(PluginError::EntryRejected {
                reason: EntryRejection::EscapesRoot,
                ..
            }) => {}
            other => panic!("expected a symlink-escape rejection, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn accepts_symlinks_that_stay_inside_the_root() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("inside-symlink");
        let target = root.join("real.so");
        std::fs::write(&target, b"not a real library").expect("failed to write the fixture file");
        symlink(&target, root.join("alias.so")).expect("failed to create the alias symlink");

        let plugin_root = PluginRoot::new(&root).expect("temporary root must canonicalize");
        let resolved = plugin_root
            .resolve("alias.so")
            .expect("a symlink staying inside the root must resolve");
        assert_eq!(
            resolved,
            std::fs::canonicalize(&target).expect("target must canonicalize")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolves_a_plain_relative_entry_under_the_root() {
        let root = temp_dir("relative");
        let target = root.join("plugin.so");
        std::fs::write(&target, b"not a real library").expect("failed to write the fixture file");

        let plugin_root = PluginRoot::new(&root).expect("temporary root must canonicalize");
        let resolved = plugin_root
            .resolve("plugin.so")
            .expect("a plain relative entry must resolve");
        assert_eq!(
            resolved,
            std::fs::canonicalize(&target).expect("target must canonicalize")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_entry_returns_a_resolve_error_not_a_panic() {
        let root = temp_dir("missing-entry");
        let loader = loader_for(&root);

        match loader.load("does-not-exist.so") {
            Err(PluginError::EntryResolve { .. }) => {}
            Err(other) => panic!("expected EntryResolve, got {other}"),
            Ok(_) => panic!("a missing entry must not load"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unlisted_entries_are_refused_before_any_filesystem_lookup() {
        let root = temp_dir("unlisted");
        let manifest = root.join("plugins.allowlist");
        std::fs::write(&manifest, "listed.so\n").expect("failed to write the allowlist manifest");
        let allowlist =
            PluginAllowlist::from_file(&manifest).expect("failed to parse the allowlist manifest");
        let loader = NativePluginLoader::with_allowlist(
            PluginRoot::new(&root).expect("temporary root must canonicalize"),
            allowlist,
        );

        // The entry does not exist on disk at all: if the allowlist were
        // consulted after resolution, this would be an `EntryResolve`
        // error instead, leaking filesystem existence to the caller.
        match loader.load("unlisted.so") {
            Err(PluginError::NotAllowlisted { .. }) => {}
            Err(other) => panic!("expected NotAllowlisted, got {other}"),
            Ok(_) => panic!("an unlisted entry must not load"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn with_manifest_reads_the_allowlist_and_binds_the_loader_to_the_root() {
        let root = temp_dir("with-manifest");
        let manifest = root.join("plugins.allowlist");
        std::fs::write(&manifest, "# operator allowlist\nlisted.so\n")
            .expect("failed to write the allowlist manifest");

        let canonical_root = std::fs::canonicalize(&root).expect("root must canonicalize");
        let loader = NativePluginLoader::with_manifest(
            PluginRoot::new(&root).expect("temporary root must canonicalize"),
            &manifest,
        )
        .expect("failed to read the allowlist manifest");

        assert_eq!(loader.root().path(), canonical_root);

        match loader.load("unlisted.so") {
            Err(PluginError::NotAllowlisted { .. }) => {}
            Err(other) => panic!("expected NotAllowlisted, got {other}"),
            Ok(_) => panic!("an unlisted entry must not load"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn allowlist_manifest_rejects_absolute_entry_names() {
        let root = temp_dir("manifest-absolute");
        let manifest = root.join("plugins.allowlist");
        std::fs::write(&manifest, "/absolute/plugin.so\n")
            .expect("failed to write the allowlist manifest");

        match PluginAllowlist::from_file(&manifest) {
            Err(PluginError::ManifestEntry {
                line: 1,
                reason: EntryRejection::Absolute,
                ..
            }) => {}
            other => panic!("expected a manifest-line rejection, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn allowlist_manifest_rejects_parent_traversal() {
        let root = temp_dir("manifest-parent");
        let manifest = root.join("plugins.allowlist");
        std::fs::write(&manifest, "../outside.so\n")
            .expect("failed to write the allowlist manifest");

        match PluginAllowlist::from_file(&manifest) {
            Err(PluginError::ManifestEntry {
                line: 1,
                reason: EntryRejection::ParentTraversal,
                ..
            }) => {}
            other => panic!("expected a manifest-line rejection, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
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
