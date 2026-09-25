// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! **A deliberate, visibly temporary placeholder.** `canary-assets`
//! doesn't exist yet (see `docs/roadmap/v0.0.5-roadmap.md`'s "The two
//! blockers this roadmap works around"), so this crate reads `.ftl`
//! files directly from disk rather than through a real asset pipeline.
//! Whoever builds `canary-assets` should expect to **delete and replace**
//! [`discover_available_locales`] and [`load_locale_resources`], not
//! discover a second, competing loading mechanism to reconcile with —
//! nothing in [`crate::LocaleBundle`] depends on *how* resources are
//! loaded, only that something can produce a `Vec<FluentResource>` for a
//! given locale, so replacing these two functions with a real asset-
//! pipeline-backed loader shouldn't require touching `LocaleBundle` at
//! all.
//!
//! # Expected directory layout
//!
//! ```text
//! <base_dir>/
//!   en-US/
//!     main.ftl
//!     items.ftl
//!   de-DE/
//!     main.ftl
//!     items.ftl
//! ```
//!
//! Each subdirectory's name must itself be a valid BCP-47-ish locale
//! identifier ([`unic_langid::LanguageIdentifier`] parses it); any
//! `.ftl` file directly inside that subdirectory is loaded, in
//! directory-listing order (not sorted -- a real asset pipeline would
//! want a stable, explicit manifest instead, another reason this is a
//! placeholder rather than a design worth hardening further). The
//! *locale list* itself ([`discover_available_locales`]) is sorted,
//! so fallback-chain negotiation never depends on filesystem order.

use std::path::Path;

use fluent::FluentResource;
use unic_langid::LanguageIdentifier;

/// An error loading `.ftl` resources for a locale from disk.
#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    /// The locale's directory (`<base_dir>/<locale>/`) doesn't exist or
    /// couldn't be read.
    #[error("could not read locale directory {path}: {source}")]
    ReadDir {
        /// The directory that couldn't be read.
        path: std::path::PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A `.ftl` file existed but couldn't be read from disk.
    #[error("could not read {path}: {source}")]
    ReadFile {
        /// The file that couldn't be read.
        path: std::path::PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A `.ftl` file's content wasn't valid Fluent syntax.
    #[error("{path} is not valid Fluent syntax: {errors:?}")]
    InvalidSyntax {
        /// The file whose content failed to parse.
        path: std::path::PathBuf,
        /// The parse errors `fluent-syntax` reported.
        errors: Vec<fluent_syntax::parser::ParserError>,
    },
    /// A `.ftl` file (or locale directory) exceeded the decode budget.
    /// Untrusted locale packs must not be able to OOM the loader with
    /// a lying file size — the budget is checked from filesystem
    /// metadata *before* reading, so the bytes are never allocated,
    /// and enforced again *during* the read itself (a capped reader),
    /// so a path whose metadata understates its contents (a symlink to
    /// a device or a file grown between the check and the read) still
    /// cannot blow past the budget.
    #[error("{path} exceeds the {limit_desc} budget ({actual_bytes} bytes)")]
    OverBudget {
        /// The file or directory that exceeded the budget.
        path: std::path::PathBuf,
        /// Which budget fired, in human terms (e.g. "1 MiB per file").
        limit_desc: &'static str,
        /// The size that tripped it.
        actual_bytes: u64,
    },
}

impl LoaderError {
    /// The filesystem path this error is about, if it names one. Every
    /// variant does today; the accessor exists so callers (and future
    /// variants) don't match-spam to recover it.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            LoaderError::ReadDir { path, .. }
            | LoaderError::ReadFile { path, .. }
            | LoaderError::InvalidSyntax { path, .. }
            | LoaderError::OverBudget { path, .. } => Some(path),
        }
    }
}

/// Maximum bytes read from any single `.ftl` file: 1 MiB is orders of
/// magnitude past any realistic locale file (the whole CLDR-derived
/// message set for a language fits in kilobytes) while bounding a
/// hostile pack's allocation before a byte is read.
pub const DEFAULT_MAX_LOCALE_FILE_BYTES: u64 = 1024 * 1024;

/// Maximum `.ftl` files read from one locale directory. Caps a hostile
/// pack's file-count amplification (thousands of tiny files each under
/// the per-file budget) independent of total bytes.
pub const DEFAULT_MAX_LOCALE_FILES: usize = 256;

/// Scans `base_dir` for subdirectories whose names parse as a
/// [`LanguageIdentifier`], returning those as the set of "available"
/// locales — the `available` argument
/// [`crate::LocaleBundle::new`] negotiates a fallback chain against.
/// Silently skips any entry that isn't a directory or doesn't parse as
/// a locale identifier (a stray file, a typo'd directory name, `.git`,
/// ...) rather than treating it as an error -- being liberal here is
/// safe, since a locale that was never actually requested or listed as
/// preferred by the game simply never gets negotiated into the fallback
/// chain regardless of appearing in this list.
///
/// Returns an empty `Vec` (not an error) if `base_dir` itself doesn't
/// exist — a game shipping zero locale files, while a real gap, isn't
/// this function's job to flag; a real content-validation step belongs
/// to whatever build tooling eventually enforces "the default locale
/// covers every key," per the roadmap's deferred compile-time-validation
/// item.
pub fn discover_available_locales(base_dir: &Path) -> Vec<LanguageIdentifier> {
    let Ok(entries) = std::fs::read_dir(base_dir) else {
        return Vec::new();
    };
    let mut locales: Vec<LanguageIdentifier> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter_map(|name| name.parse::<LanguageIdentifier>().ok())
        .collect();
    // Filesystem iteration order is unspecified; negotiation downstream
    // must not depend on it.
    locales.sort();
    locales
}

/// Loads every `.ftl` file directly inside `base_dir/<locale>/` as a
/// [`FluentResource`]. See this module's docs for the expected layout.
///
/// Enforces [`DEFAULT_MAX_LOCALE_FILE_BYTES`] per file (checked from
/// metadata before reading) and [`DEFAULT_MAX_LOCALE_FILES`] per
/// directory; use [`load_locale_resources_with_budget`] for explicit
/// limits.
pub fn load_locale_resources(
    base_dir: &Path,
    locale: &LanguageIdentifier,
) -> Result<Vec<FluentResource>, LoaderError> {
    load_locale_resources_with_budget(
        base_dir,
        locale,
        DEFAULT_MAX_LOCALE_FILE_BYTES,
        DEFAULT_MAX_LOCALE_FILES,
    )
}

/// [`load_locale_resources`] with explicit decode budgets, for callers
/// (tests, tooling) that need limits other than the defaults. Budgets
/// are enforced before allocation (from filesystem metadata, so an
/// over-limit file errors without its bytes ever being read) and again
/// during the read itself (a capped reader, so a path whose metadata
/// understates its contents still cannot blow past the budget).
pub fn load_locale_resources_with_budget(
    base_dir: &Path,
    locale: &LanguageIdentifier,
    max_file_bytes: u64,
    max_files: usize,
) -> Result<Vec<FluentResource>, LoaderError> {
    let locale_dir = base_dir.join(locale.to_string());
    let entries = std::fs::read_dir(&locale_dir).map_err(|source| LoaderError::ReadDir {
        path: locale_dir.clone(),
        source,
    })?;

    let mut resources = Vec::new();
    let mut file_count = 0usize;
    for entry in entries {
        let entry = entry.map_err(|source| LoaderError::ReadDir {
            path: locale_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ftl") {
            continue;
        }
        file_count += 1;
        if file_count > max_files {
            return Err(LoaderError::OverBudget {
                path: locale_dir.clone(),
                limit_desc: "max files per locale",
                actual_bytes: file_count as u64,
            });
        }
        let size = entry
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(u64::MAX);
        if size > max_file_bytes {
            return Err(LoaderError::OverBudget {
                path: path.clone(),
                limit_desc: "max bytes per file",
                actual_bytes: size,
            });
        }
        let content = read_file_with_budget(&path, max_file_bytes)?;
        let resource = FluentResource::try_new(content).map_err(|(_partial, errors)| {
            LoaderError::InvalidSyntax {
                path: path.clone(),
                errors,
            }
        })?;
        resources.push(resource);
    }
    Ok(resources)
}

/// Reads `path` to a string with `max_file_bytes` enforced during the
/// read itself, not just from metadata beforehand: the reader takes at
/// most one byte past the budget, so a path whose metadata understates
/// its contents (a symlink to a device, a file grown between the check
/// and the read) errors with [`LoaderError::OverBudget`] instead of
/// allocating without bound. The metadata pre-check in the caller stays
/// as the fast path (exact size, no bytes read); this is the backstop.
fn read_file_with_budget(path: &Path, max_file_bytes: u64) -> Result<String, LoaderError> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|source| LoaderError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    let mut capped = file.take(max_file_bytes.saturating_add(1));
    let mut content = String::new();
    capped
        .read_to_string(&mut content)
        .map_err(|source| LoaderError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
    if content.len() as u64 > max_file_bytes {
        return Err(LoaderError::OverBudget {
            path: path.to_path_buf(),
            limit_desc: "max bytes per file",
            actual_bytes: content.len() as u64,
        });
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_locale_subdirectories_and_skips_non_locale_entries() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        std::fs::create_dir(dir.path().join("en-US")).unwrap();
        std::fs::create_dir(dir.path().join("de-DE")).unwrap();
        // Not a valid locale identifier -- should be skipped, not error.
        std::fs::create_dir(dir.path().join("not_a_locale!!")).unwrap();
        // A file, not a directory -- should also be skipped.
        std::fs::write(dir.path().join("readme.txt"), "hi").unwrap();

        let mut found = discover_available_locales(dir.path());
        found.sort_by_key(|l| l.to_string());

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].to_string(), "de-DE");
        assert_eq!(found[1].to_string(), "en-US");
    }

    #[test]
    fn missing_base_dir_returns_empty_not_an_error() {
        let found = discover_available_locales(Path::new("/does/not/exist/at/all"));
        assert!(found.is_empty());
    }

    #[test]
    fn loads_real_ftl_files_and_ignores_non_ftl_files_in_the_locale_dir() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let locale_dir = dir.path().join("en-US");
        std::fs::create_dir(&locale_dir).unwrap();
        std::fs::write(locale_dir.join("main.ftl"), "greeting = Hello!").unwrap();
        std::fs::write(locale_dir.join("items.ftl"), "sword-name = Sword").unwrap();
        std::fs::write(locale_dir.join("notes.txt"), "not a resource").unwrap();

        let locale: LanguageIdentifier = "en-US".parse().unwrap();
        let resources = load_locale_resources(dir.path(), &locale)
            .expect("loading real .ftl files should succeed");

        assert_eq!(resources.len(), 2, "the .txt file should have been skipped");
    }

    #[test]
    fn reports_a_real_syntax_error_rather_than_silently_dropping_the_file() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let locale_dir = dir.path().join("en-US");
        std::fs::create_dir(&locale_dir).unwrap();
        std::fs::write(
            locale_dir.join("broken.ftl"),
            "this is = not = valid fluent =",
        )
        .unwrap();

        let locale: LanguageIdentifier = "en-US".parse().unwrap();
        let result = load_locale_resources(dir.path(), &locale);

        assert!(
            matches!(result, Err(LoaderError::InvalidSyntax { .. })),
            "expected an InvalidSyntax error, got: {result:?}"
        );
    }

    #[test]
    fn missing_locale_directory_is_a_real_error_not_an_empty_vec() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let locale: LanguageIdentifier = "fr-FR".parse().unwrap();
        let result = load_locale_resources(dir.path(), &locale);
        assert!(
            matches!(result, Err(LoaderError::ReadDir { .. })),
            "expected a ReadDir error for a locale directory that was never created, got: {result:?}"
        );
    }

    #[test]
    fn oversized_and_excess_files_fail_with_overbudget_before_reading() {
        // Over-size: metadata reports past the per-file budget, so the
        // loader must refuse without allocating the content. A sparse
        // file keeps this cheap on disk while reporting a large size.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let locale_dir = dir.path().join("en-US");
        std::fs::create_dir(&locale_dir).unwrap();
        let big = std::fs::File::create(locale_dir.join("big.ftl")).unwrap();
        big.set_len(DEFAULT_MAX_LOCALE_FILE_BYTES + 1).unwrap();
        drop(big);

        let locale: LanguageIdentifier = "en-US".parse().unwrap();
        let result = load_locale_resources(dir.path(), &locale);
        assert!(
            matches!(result, Err(LoaderError::OverBudget { .. })),
            "expected OverBudget for the oversized file, got: {result:?}"
        );
        assert_eq!(
            result.unwrap_err().path(),
            Some(locale_dir.join("big.ftl").as_path()),
            "the error must name the offending file"
        );

        // Over-count: more files than the per-directory budget, each
        // tiny (so the per-file budget never fires first).
        let dir2 = tempfile::tempdir().expect("failed to create temp dir");
        let locale_dir2 = dir2.path().join("en-US");
        std::fs::create_dir(&locale_dir2).unwrap();
        for index in 0..=DEFAULT_MAX_LOCALE_FILES {
            std::fs::write(locale_dir2.join(format!("f{index}.ftl")), "k = V").unwrap();
        }
        let result = load_locale_resources(dir2.path(), &locale);
        assert!(
            matches!(result, Err(LoaderError::OverBudget { .. })),
            "expected OverBudget past the file-count budget, got: {result:?}"
        );
    }

    #[test]
    fn the_read_itself_is_capped_even_when_metadata_passes() {
        // The metadata pre-check sees a small file here (an explicit
        // 8-byte budget against a 5-byte file passes it), so only the
        // capped reader can refuse: proves enforcement lives in the
        // read, not just in the `stat`.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let file = dir.path().join("small.ftl");
        std::fs::write(&file, "k = V\n").unwrap();

        let result = read_file_with_budget(&file, 2);
        assert!(
            matches!(result, Err(LoaderError::OverBudget { .. })),
            "a read past the byte budget must fail capped, got: {result:?}"
        );
        assert_eq!(result.unwrap_err().path(), Some(file.as_path()));

        // At-or-under budget still reads through untouched.
        let content = read_file_with_budget(&file, 6).expect("exact-budget read must succeed");
        assert_eq!(content, "k = V\n");
    }
}
