// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::io;
use std::path::{Path, PathBuf};

/// Errors returned by `canary-assets` operations.
///
/// Styled after [`canary_ecs::EcsError`](https://github.com/HylightGames/canary/blob/dev/engine/canary-ecs/src/error.rs)
/// — one small `thiserror` enum with a variant per failure mode, each
/// carrying the context needed to act on it — but deliberately its own
/// type rather than a reuse: asset failures (unreadable file, malformed
/// bytes, over-budget decode) are a different failure domain from ECS
/// failures (stale entity, duplicate schema id), and merging them would
/// couple the two crates' error evolution for no benefit. See
/// [ADR 0018](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0018-asset-handles-and-synchronous-loading.md).
///
/// Every file-originated variant carries the offending `path`, so a log
/// line or a user-facing message can name the file without the caller
/// threading it through separately. `path` is cloned into the error at
/// construction time (setup-path cost, never hot-path), which keeps the
/// error self-contained after the caller's `Path` borrow ends.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AssetError {
    /// The file could not be read from disk (missing, permission denied,
    /// I/O failure mid-read). The underlying [`io::Error`] is preserved
    /// as the source so callers can match on its kind; the message names
    /// the path because "No such file or directory" alone never says
    /// *which* file.
    #[error("cannot read asset file '{}': {source}", path.display())]
    Io {
        /// The path that was being read when the I/O failure occurred.
        path: PathBuf,
        /// The underlying operating-system I/O failure.
        #[source]
        source: io::Error,
    },

    /// The file's bytes were readable but do not parse as the expected
    /// format (truncated GLB header, bad PNG signature, corrupt chunk,
    /// garbage where hex was expected). The `reason` names what was
    /// wrong in one short clause — enough to distinguish "not this
    /// format at all" from "this format, damaged" without dumping the
    /// offending bytes into the message.
    #[error("invalid asset file '{}': {reason}", path.display())]
    InvalidFormat {
        /// The path whose bytes failed to parse.
        path: PathBuf,
        /// What was wrong with the bytes, in one short clause.
        reason: String,
    },

    /// The file is well-formed but uses a feature this release's minimal
    /// loaders deliberately do not implement (interlaced PNG, a GLB
    /// extension, >8-bit sample depth beyond normalization). Separate
    /// from [`AssetError::InvalidFormat`] on purpose: invalid means the
    /// *file* is broken, unsupported means the *loader* is narrow, and
    /// conflating them would send content authors to "fix" a file that
    /// is fine.
    #[error("unsupported feature in asset file '{}': {feature}", path.display())]
    UnsupportedFeature {
        /// The path whose bytes require the unimplemented feature.
        path: PathBuf,
        /// Which feature the loader declined to implement.
        feature: String,
    },

    /// Decoding the file would exceed the caller's stated budget
    /// (dimensions, pixel count, total bytes). This is untrusted-file
    /// discipline from day one per ADR 0018: a 1x1 PNG claiming a
    /// gigapixel IDAT must fail here, with the numbers attached, rather
    /// than allocating first and apologizing after.
    #[error("asset file '{}' exceeds budget: {actual} > limit {limit}", path.display())]
    OverBudget {
        /// The path whose decode was refused.
        path: PathBuf,
        /// The budget that was enforced, in the unit the loader documents.
        limit: u64,
        /// The claimed size that exceeded it, in the same unit.
        actual: u64,
    },

    /// An [`crate::AssetHandle`] referred to a slot that is empty,
    /// holds a different generation, or was never allocated in this
    /// [`crate::AssetStore`]. Carries raw `index`/`generation` rather
    /// than the handle itself so this variant stays constructible
    /// without importing the generic handle type, and so the message
    /// reads the same whether the caller held a typed handle or raw
    /// parts from a boundary. Note the store's `get` returns `None`
    /// for this condition instead of this error — this variant exists
    /// for paths that must distinguish "stale handle" from other
    /// failures (Phase 2 loaders resolving references between files).
    #[error("unknown asset handle: index {index}, generation {generation}")]
    UnknownHandle {
        /// The slot index the stale handle pointed at.
        index: u32,
        /// The generation the stale handle expected.
        generation: u64,
    },
}

impl AssetError {
    /// Constructs the [`AssetError::Io`] variant from a path-like and
    /// the I/O failure that reading it produced.
    ///
    /// Exists because `map_err` closures at call sites would otherwise
    /// repeat the `path.to_path_buf()` bookkeeping at every read; one
    /// constructor keeps every site honest about attaching the path.
    pub fn io(path: &Path, source: io::Error) -> Self {
        AssetError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    /// Constructs the [`AssetError::InvalidFormat`] variant from a
    /// path-like and a short reason clause.
    pub fn invalid_format(path: &Path, reason: impl Into<String>) -> Self {
        AssetError::InvalidFormat {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }

    /// Constructs the [`AssetError::UnsupportedFeature`] variant from a
    /// path-like and the feature name the loader declined.
    pub fn unsupported_feature(path: &Path, feature: impl Into<String>) -> Self {
        AssetError::UnsupportedFeature {
            path: path.to_path_buf(),
            feature: feature.into(),
        }
    }

    /// Constructs the [`AssetError::OverBudget`] variant from a
    /// path-like, the enforced limit, and the claimed size that
    /// exceeded it (both in the unit the loader documents).
    pub fn over_budget(path: &Path, limit: u64, actual: u64) -> Self {
        AssetError::OverBudget {
            path: path.to_path_buf(),
            limit,
            actual,
        }
    }

    /// The path this error is about, or `None` for
    /// [`AssetError::UnknownHandle`], which names a store slot rather
    /// than a file.
    ///
    /// Exists so loggers and user-facing reporters can group or filter
    /// asset failures by file without matching on every variant.
    pub fn path(&self) -> Option<&Path> {
        match self {
            AssetError::Io { path, .. }
            | AssetError::InvalidFormat { path, .. }
            | AssetError::UnsupportedFeature { path, .. }
            | AssetError::OverBudget { path, .. } => Some(path),
            AssetError::UnknownHandle { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_message_names_the_path() {
        let err = AssetError::io(
            Path::new("meshes/quad.glb"),
            io::Error::new(io::ErrorKind::NotFound, "missing here"),
        );
        let rendered = err.to_string();
        assert!(
            rendered.contains("meshes/quad.glb"),
            "message must name the file, got: {rendered}"
        );
        assert_eq!(
            err.path(),
            Some(Path::new("meshes/quad.glb")),
            "path accessor must return the file"
        );
    }

    #[test]
    fn missing_file_surfaces_as_io_not_a_panic() {
        let result: Result<Vec<u8>, AssetError> = std::fs::read("definitely-not-here.glb")
            .map_err(|source| AssetError::io(Path::new("definitely-not-here.glb"), source));
        let err = result.expect_err("reading a missing file must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "missing file must be Io, got: {err:?}"
        );
    }

    #[test]
    fn invalid_format_message_carries_path_and_reason() {
        let err = AssetError::invalid_format(Path::new("tex/noise.png"), "bad PNG signature");
        let rendered = err.to_string();
        assert!(rendered.contains("tex/noise.png"), "got: {rendered}");
        assert!(rendered.contains("bad PNG signature"), "got: {rendered}");
    }

    #[test]
    fn unsupported_feature_is_distinct_from_invalid_format() {
        let err = AssetError::unsupported_feature(Path::new("tex/deep.png"), "16-bit sample depth");
        assert!(
            matches!(err, AssetError::UnsupportedFeature { .. }),
            "narrow loader must not blame the file, got: {err:?}"
        );
        assert!(err.to_string().contains("16-bit sample depth"));
    }

    #[test]
    fn over_budget_reports_both_numbers() {
        let err = AssetError::over_budget(Path::new("tex/huge.png"), 65_536, 1_000_000);
        let rendered = err.to_string();
        assert!(rendered.contains("65536"), "got: {rendered}");
        assert!(rendered.contains("1000000"), "got: {rendered}");
    }

    #[test]
    fn unknown_handle_has_no_path() {
        let err = AssetError::UnknownHandle {
            index: 3,
            generation: 1,
        };
        assert_eq!(err.path(), None, "handle errors name a slot, not a file");
        assert!(err.to_string().contains('3'));
    }
}
