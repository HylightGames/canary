//! Bounded local-file reads shared by both state products.
//!
//! Authored project files and snapshot files arrive from outside the
//! process (downloads, mods, shared projects), so neither may be read
//! without a size ceiling: `std::fs::read`/`read_to_string` allocate as
//! much as the file claims before any validation runs. The pattern here
//! mirrors `canary-assets`' `read_file_with_budget` and the Tier A
//! loader's size gate: a metadata pre-check refuses an over-limit file
//! without reading a byte, and a capped reader takes at most one byte
//! past the limit so a file that grows between the check and the read
//! still errors instead of allocating without bound.

use std::io::Read as _;
use std::path::Path;

use crate::error::StateError;

/// Reads the file at `path`, refusing more than `max_bytes`.
///
/// The returned bytes are at most `max_bytes` long. A missing or
/// unreadable file reports [`StateError::File`]; an over-budget file
/// reports [`StateError::InputTooLarge`].
pub(crate) fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, StateError> {
    if let Ok(metadata) = std::fs::metadata(path) {
        let size = metadata.len();
        if size > max_bytes {
            return Err(too_large(path, size, max_bytes));
        }
    }
    let file = std::fs::File::open(path).map_err(|error| StateError::File {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| StateError::File {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    let actual = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if actual > max_bytes {
        return Err(too_large(path, actual, max_bytes));
    }
    Ok(bytes)
}

/// Builds the typed over-budget refusal for one path.
pub(crate) fn too_large(path: &Path, size: u64, max: u64) -> StateError {
    StateError::InputTooLarge {
        path: path.to_path_buf(),
        size,
        max,
    }
}
