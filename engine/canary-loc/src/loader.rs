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
//! placeholder rather than a design worth hardening further).

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
}

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
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter_map(|name| name.parse::<LanguageIdentifier>().ok())
        .collect()
}

/// Loads every `.ftl` file directly inside `base_dir/<locale>/` as a
/// [`FluentResource`]. See this module's docs for the expected layout.
pub fn load_locale_resources(
    base_dir: &Path,
    locale: &LanguageIdentifier,
) -> Result<Vec<FluentResource>, LoaderError> {
    let locale_dir = base_dir.join(locale.to_string());
    let entries = std::fs::read_dir(&locale_dir).map_err(|source| LoaderError::ReadDir {
        path: locale_dir.clone(),
        source,
    })?;

    let mut resources = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| LoaderError::ReadDir {
            path: locale_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ftl") {
            continue;
        }
        let content = std::fs::read_to_string(&path).map_err(|source| LoaderError::ReadFile {
            path: path.clone(),
            source,
        })?;
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
}
