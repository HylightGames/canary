// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Budget-capped file reads shared by every asset loader.
//!
//! The loaders in [`crate::mesh`], [`crate::texture`], and [`crate::AssetId`]
//! all start from the same place — raw file bytes — so they share one
//! budget discipline rather than each re-implementing it: a metadata
//! pre-check (refuse without reading a byte when the filesystem already
//! reports past the budget) backed by a capped reader (at most one byte
//! past the budget is ever pulled from disk, so a path whose metadata
//! understates its contents still cannot blow past it). This mirrors the
//! proven pattern in `canary-loc`'s `loader.rs`: the metadata check is the
//! fast path, the capped reader is the backstop, and both report
//! [`crate::AssetError::OverBudget`] with the same numbers.

use std::path::{Path, PathBuf};

use crate::AssetError;

/// Maximum bytes read from any single asset file: 64 MiB comfortably
/// covers this release's kilobyte fixtures and any realistic near-term
/// content (a 4K RGBA8 texture decodes to ~64 MiB but compresses to far
/// less on disk; a self-contained GLB past tens of megabytes is already
/// outside this release's minimal-loader scope) while bounding a hostile
/// file's allocation before a byte is read.
///
/// The value is **provisional** pending measured calibration against real
/// content, exactly like the per-primitive mesh budgets and the texture
/// decode budget. Callers with measured needs read through
/// [`read_file_with_budget`] with their own ceiling instead.
pub const MAX_ASSET_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Resolves an untrusted root-relative `candidate` path against a
/// trusted asset `root`, refusing anything that escapes the root.
///
/// This is the confinement primitive the future asset manager builds
/// on: it will join manifest- or pack-listed relative paths onto a
/// content root, and a hostile listing must not be able to name
/// `/etc/passwd` or `../../secrets.bin`. Three escape shapes are
/// refused with [`crate::AssetError::OutsideRoot`]:
/// - an absolute `candidate` (joining it would discard `root` entirely);
/// - a `..` traversal that lands outside `root` after normalization;
/// - a symlink inside `root` whose target lies outside it.
///
/// Both sides are canonicalized (symlinks resolved, `.`/`..`
/// normalized) before the `starts_with` check, so the comparison sees
/// the real locations, not the spelling. A `..` that stays inside
/// (`sub/../file.glb`) is accepted: confinement is about *where* the
/// bytes live, not how the path is spelled.
///
/// Failure taxonomy:
/// - Unresolvable `root` or `candidate` (missing file, permission
///   denied) → [`crate::AssetError::Io`], keeping the existing
///   read-before-anything order: a missing file is never reported as
///   a budget or confinement failure.
/// - Resolved outside `root` → [`crate::AssetError::OutsideRoot`]
///   with both the root and the resolved path attached.
///
/// Known limit, stated honestly: like the budget pre-check below,
/// this is check-then-use — a path swapped between resolution and
/// read (TOCTOU) is not covered. Fully atomic confinement needs
/// `openat`-style directory-relative opens, which `std` does not
/// offer; this helper shrinks the confused-deputy surface for the
/// manager without claiming to be a sandbox boundary.
///
/// Deliberately duplicated rather than shared with `canary-loc`:
/// the two crates share no leaf dependency (`canary-loc` depends on
/// no workspace crate at all), and their checks differ anyway — the
/// locale loader needs a symlink no-follow on one fixed directory,
/// while this resolves arbitrary candidates under a root — so one
/// shared helper would couple the crates for negative benefit.
pub fn resolve_in_root(root: &Path, candidate: &Path) -> Result<PathBuf, AssetError> {
    if candidate.is_absolute() {
        return Err(AssetError::outside_root(root, candidate));
    }
    let canonical_root =
        std::fs::canonicalize(root).map_err(|source| AssetError::io(root, source))?;
    let joined = root.join(candidate);
    let resolved =
        std::fs::canonicalize(&joined).map_err(|source| AssetError::io(&joined, source))?;
    if resolved.starts_with(&canonical_root) {
        Ok(resolved)
    } else {
        Err(AssetError::outside_root(root, &resolved))
    }
}

/// Reads the file at `path` into memory, refusing files past
/// `max_file_bytes` with [`crate::AssetError::OverBudget`] instead of
/// allocating without bound.
///
/// Enforcement lives in two places on purpose: the metadata pre-check
/// refuses an over-limit file without its bytes ever being read (a sparse
/// file reporting gigabytes fails here, cheaply), and the capped reader
/// takes at most one byte past the budget, so a path whose metadata
/// understates its contents (a symlink to a device, a file grown between
/// the check and the read) still errors instead of allocating. A missing
/// or unreadable file yields [`crate::AssetError::Io`] — the read runs
/// before every budget guard, so a missing file can never report a
/// budget failure.
pub(crate) fn read_file_with_budget(
    path: &Path,
    max_file_bytes: u64,
) -> Result<Vec<u8>, AssetError> {
    if let Ok(metadata) = std::fs::metadata(path) {
        let size = metadata.len();
        if size > max_file_bytes {
            return Err(AssetError::over_budget(path, max_file_bytes, size));
        }
    }
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|source| AssetError::io(path, source))?;
    let mut capped = file.take(max_file_bytes.saturating_add(1));
    let mut bytes = Vec::new();
    capped
        .read_to_end(&mut bytes)
        .map_err(|source| AssetError::io(path, source))?;
    let actual = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if actual > max_file_bytes {
        return Err(AssetError::over_budget(path, max_file_bytes, actual));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_file_is_refused_before_reading_a_byte() {
        // A sparse file reports past the budget from metadata while
        // costing (almost) nothing on disk: the loader must refuse it
        // without allocating its contents.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("big.glb");
        let file = std::fs::File::create(&path).expect("sparse fixture must be creatable");
        file.set_len(MAX_ASSET_FILE_BYTES + 1)
            .expect("sparse resize must succeed");
        drop(file);

        let err = read_file_with_budget(&path, MAX_ASSET_FILE_BYTES)
            .expect_err("over-limit file must fail");
        match &err {
            AssetError::OverBudget { limit, actual, .. } => {
                assert_eq!(*limit, MAX_ASSET_FILE_BYTES);
                assert_eq!(*actual, MAX_ASSET_FILE_BYTES + 1);
            }
            other => panic!("over-limit file must be OverBudget, got: {other:?}"),
        }
        assert_eq!(
            err.path(),
            Some(path.as_path()),
            "the error must name the offending file"
        );
    }

    #[test]
    fn the_read_itself_is_capped_even_when_metadata_passes() {
        // A plain file's metadata is exact, so no plain file can pass
        // the metadata pre-check yet exceed the budget at read time —
        // that backstop exists for paths whose metadata understates
        // their contents (a symlink to a device, a file grown between
        // the check and the read). This test pins the observable half
        // of that contract: over-budget content fails capped with both
        // numbers attached, and an at-or-under-budget read passes
        // through byte-identical.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("small.bin");
        std::fs::write(&path, b"12345").expect("scratch fixture must be writable");

        let err =
            read_file_with_budget(&path, 4).expect_err("5 bytes past a 4-byte budget must fail");
        match &err {
            AssetError::OverBudget { limit, actual, .. } => {
                assert_eq!(*limit, 4);
                assert_eq!(*actual, 5);
            }
            other => panic!("a read past the byte budget must fail capped, got: {other:?}"),
        }
        assert_eq!(err.path(), Some(path.as_path()));

        let bytes = read_file_with_budget(&path, 5).expect("exact-budget read must succeed");
        assert_eq!(bytes, b"12345");
    }

    #[test]
    fn missing_file_is_io_not_over_budget() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("definitely-not-here.bin");
        let err = read_file_with_budget(&path, 0).expect_err("missing file must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "read-before-budget: missing file must be Io even with budget 0, got: {err:?}"
        );
    }

    #[test]
    fn dot_dot_escape_is_outside_root() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let root = dir.path().join("assets");
        std::fs::create_dir(&root).expect("root must be creatable");
        let outside = dir.path().join("secret.bin");
        std::fs::write(&outside, b"top secret").expect("outside file must be writable");

        let err = resolve_in_root(&root, Path::new("../secret.bin"))
            .expect_err("traversal out of the root must fail");
        assert!(
            matches!(err, AssetError::OutsideRoot { .. }),
            "dot-dot escape must be OutsideRoot, got: {err:?}"
        );
    }

    #[test]
    fn absolute_candidate_is_outside_root_even_when_it_exists() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let root = dir.path().join("assets");
        std::fs::create_dir(&root).expect("root must be creatable");
        let outside = dir.path().join("secret.bin");
        std::fs::write(&outside, b"top secret").expect("outside file must be writable");

        // Joining an absolute candidate would discard the root entirely,
        // so it is refused without even resolving — existence is
        // irrelevant to the verdict.
        let err = resolve_in_root(&root, &outside).expect_err("absolute candidate must fail");
        assert!(
            matches!(err, AssetError::OutsideRoot { .. }),
            "absolute path must be OutsideRoot, got: {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_root_pointing_out_is_outside_root() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let root = dir.path().join("assets");
        std::fs::create_dir(&root).expect("root must be creatable");
        let outside = dir.path().join("secret.bin");
        std::fs::write(&outside, b"top secret").expect("outside file must be writable");
        std::os::unix::fs::symlink(&outside, root.join("link.bin"))
            .expect("symlink must be creatable");

        let err = resolve_in_root(&root, Path::new("link.bin")).expect_err("symlink-out must fail");
        match &err {
            AssetError::OutsideRoot { path, .. } => {
                // The implementation names the canonical target (symlinks
                // resolved): compare against the canonicalized expectation
                // so the test holds where the temp dir itself sits behind
                // a symlink (macOS /tmp -> /private/tmp).
                let expected =
                    std::fs::canonicalize(&outside).expect("outside fixture must be resolvable");
                assert_eq!(
                    path.as_path(),
                    expected.as_path(),
                    "the error must name the real target, not the link"
                );
            }
            other => panic!("symlink-out must be OutsideRoot, got: {other:?}"),
        }
    }

    #[test]
    fn valid_relative_paths_resolve_and_read_byte_identical() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let root = dir.path().join("assets");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).expect("nested root must be creatable");
        std::fs::write(sub.join("file.bin"), b"12345").expect("fixture must be writable");

        // Plain relative, nested relative, and a `..` that stays inside
        // all resolve; the bytes read through the resolution equal the
        // bytes read directly — confinement changes *which* paths are
        // accepted, never the content of accepted ones. The root is
        // canonicalized for comparison because resolution returns
        // canonical paths while temp dirs may sit behind symlinks
        // (macOS /tmp) or verbatim prefixes (Windows \\?\).
        let canonical_root = std::fs::canonicalize(&root).expect("root must be resolvable");
        for candidate in [Path::new("sub/file.bin"), Path::new("sub/../sub/file.bin")] {
            let resolved =
                resolve_in_root(&root, candidate).expect("in-root candidate must resolve");
            assert!(
                resolved.starts_with(canonical_root.as_path()),
                "resolution must stay under the root, got: {}",
                resolved.display()
            );
            let via_root = read_file_with_budget(&resolved, MAX_ASSET_FILE_BYTES)
                .expect("resolved file must read");
            let direct = read_file_with_budget(&sub.join("file.bin"), MAX_ASSET_FILE_BYTES)
                .expect("direct read must succeed");
            assert_eq!(via_root, direct);
            assert_eq!(via_root, b"12345");
        }
    }

    #[test]
    fn missing_candidate_is_io_not_outside_root() {
        // Read-before-confinement order: canonicalizing a nonexistent
        // candidate fails first, so a missing file stays Io — the same
        // taxonomy the single-path loaders already promise.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let root = dir.path().join("assets");
        std::fs::create_dir(&root).expect("root must be creatable");

        let err = resolve_in_root(&root, Path::new("definitely-not-here.bin"))
            .expect_err("missing candidate must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "missing candidate must be Io, got: {err:?}"
        );
    }
}
