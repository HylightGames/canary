//! Bounded loader/boundary fuzz sweep for the Fluent (`.ftl`) locale pack
//! loader (`loader.rs`).
//!
//! FINDINGS-ONLY sweep: this file feeds hostile pack shapes to the public
//! loader entries ([`load_locale_resources`]/[`load_locale_resources_with_budget`]
//! and [`discover_available_locales`]/[`try_discover_available_locales`])
//! and asserts the contract — `Ok(resources)` or `Err(typed)` — never a
//! panic, hang (>10 s per case), unbounded allocation, or an `Ok` that
//! bypassed a documented budget.
//!
//! Method: deterministic seeded corpus (no RNG at test time). A fixed set
//! of `.ftl` content mutations (empty, garbage, unclosed syntax, duplicate
//! keys, multi-kilobyte files truncated at every 1 KiB boundary), budget
//! shapes (per-file over/exact/zero, per-locale file-count over), and
//! directory shapes (missing locale, stray files, non-UTF-8 bytes) is
//! written into fresh temp packs. Files stay at kilobyte scale, so no case
//! can spend more than milliseconds; the whole file runs in seconds.

use std::path::Path;

use canary_loc::{
    discover_available_locales, load_locale_resources, load_locale_resources_with_budget,
    LoaderError, DEFAULT_MAX_LOCALE_FILES, DEFAULT_MAX_LOCALE_FILE_BYTES,
};

/// Creates `<dir>/en-US/` seeded with `files` (`name` → UTF-8 content) and
/// returns the pack base dir.
fn pack_with_files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp pack must be creatable");
    let locale_dir = dir.path().join("en-US");
    std::fs::create_dir(&locale_dir).expect("locale dir must be creatable");
    for (name, content) in files {
        std::fs::write(locale_dir.join(name), content).expect("pack file must be writable");
    }
    dir
}

/// Loads `en-US` from `base` under default budgets, checking the contract:
///
/// - `Err` must be a typed [`LoaderError`] naming a path;
/// - `Ok` is accepted as-is (an empty or comment-only file parses to an
///   empty resource — legitimate, not a bypass).
fn check_contract(base: &Path) -> Result<Vec<fluent::FluentResource>, LoaderError> {
    let locale: unic_langid::LanguageIdentifier = "en-US".parse().expect("locale must parse");
    let result = load_locale_resources(base, &locale);
    if let Err(err) = &result {
        assert!(
            err.path().is_some(),
            "every loader error must name its path, got: {err:?}"
        );
    }
    result
}

#[test]
fn valid_pack_loads_and_non_ftl_files_are_ignored() {
    let dir = pack_with_files(&[
        ("main.ftl", "greeting = Hello!\n"),
        ("items.ftl", "sword-name = Sword\n"),
        ("notes.txt", "not a resource"),
    ]);
    let locale: unic_langid::LanguageIdentifier = "en-US".parse().expect("locale must parse");
    let resources = load_locale_resources(dir.path(), &locale).expect("valid pack must load");
    assert_eq!(resources.len(), 2, "the .txt file must be skipped");

    let found = discover_available_locales(dir.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].to_string(), "en-US");
}

#[test]
fn ftl_content_mutations_never_panic_and_fail_typed() {
    // Each content is loaded as the locale's only `.ftl` file: it must
    // parse to resources or fail as InvalidSyntax/ReadFile — never panic.
    // (`Ok` with any resource count is legitimate: Fluent accepts empty
    // and comment-only files, and duplicate keys resolve by documented
    // override rather than loader invention.)
    let contents = [
        "",
        "\n",
        "# only a comment\n",
        "k = V\n",
        "k = V\nk = W\n",
        "this is = not = valid fluent =",
        "= no-key\n",
        "key = {unclosed\n",
        "key = { $missing }\n",
        "-term = Term\nuse-term = { -term }\n",
        "key = Value\r\ncrlf = Also\r\n",
        "\u{FEFF}bom-key = BOM\n",
        "emoji = 🎉🚀\n",
        "nul = a\0b\n",
    ];
    for (index, content) in contents.iter().enumerate() {
        let dir = pack_with_files(&[("main.ftl", content)]);
        let outcome = check_contract(dir.path());
        match outcome {
            Ok(_) => {}
            Err(err) => assert!(
                matches!(
                    err,
                    LoaderError::InvalidSyntax { .. } | LoaderError::ReadFile { .. }
                ),
                "content case {index} must be InvalidSyntax or ReadFile, got: {err:?}"
            ),
        }
    }
}

#[test]
fn non_utf8_bytes_are_a_read_error_not_a_panic() {
    let dir = tempfile::tempdir().expect("temp pack must be creatable");
    let locale_dir = dir.path().join("en-US");
    std::fs::create_dir(&locale_dir).expect("locale dir must be creatable");
    // Invalid UTF-8: the capped `read_to_string` fails, which must surface
    // as ReadFile (unreadable text), never OverBudget (unmeasured bytes)
    // and never a panic.
    std::fs::write(locale_dir.join("main.ftl"), [0x66, 0x3D, 0xFF, 0xFE, 0x0A])
        .expect("raw bytes must be writable");
    let locale: unic_langid::LanguageIdentifier = "en-US".parse().expect("locale must parse");
    let err = load_locale_resources(dir.path(), &locale).expect_err("non-UTF-8 must fail");
    assert!(
        matches!(err, LoaderError::ReadFile { .. }),
        "non-UTF-8 bytes must be ReadFile, got: {err:?}"
    );
}

#[test]
fn truncation_at_1kb_boundaries_never_panics_and_fails_typed() {
    // A ~3.5 KiB file of 200 messages, truncated at every 1 KiB boundary
    // plus fractional cuts and the off-by-one tail: each prefix must parse
    // (a cut between messages is still valid Fluent) or fail as
    // InvalidSyntax — a cut mid-message is corruption, not a budget event.
    let mut full = String::new();
    for index in 0..200 {
        full.push_str(&format!("message-{index} = Value number {index}\n"));
    }
    assert!(
        full.len() > 3 * 1024,
        "the sweep file must span several KiB, got {}",
        full.len()
    );
    let mut cuts = vec![0usize, 1];
    let mut boundary = 1024;
    while boundary < full.len() {
        cuts.push(boundary);
        boundary += 1024;
    }
    for cut in [
        full.len() / 4,
        full.len() / 2,
        full.len() * 3 / 4,
        full.len() - 1,
    ] {
        cuts.push(cut);
    }
    cuts.sort_unstable();
    cuts.dedup();
    for cut in cuts {
        // Truncation may split a multibyte char; `String` slicing would
        // panic in the *test* — operate on bytes and only keep
        // UTF-8-prefix-safe cuts via `str::get`, skipping the rest.
        let Some(prefix) = full.get(..cut) else {
            continue;
        };
        let dir = pack_with_files(&[("main.ftl", prefix)]);
        match check_contract(dir.path()) {
            Ok(_) => {}
            Err(err) => assert!(
                matches!(err, LoaderError::InvalidSyntax { .. }),
                "truncation at {cut} must be InvalidSyntax, got: {err:?}"
            ),
        }
    }
}

#[test]
fn per_file_budget_paths_hold_at_over_exact_and_zero() {
    // Over: a sparse file reports past the per-file budget from metadata
    // while costing nothing on disk — refused without reading.
    let dir = tempfile::tempdir().expect("temp pack must be creatable");
    let locale_dir = dir.path().join("en-US");
    std::fs::create_dir(&locale_dir).expect("locale dir must be creatable");
    let big = std::fs::File::create(locale_dir.join("big.ftl")).expect("sparse must be creatable");
    big.set_len(DEFAULT_MAX_LOCALE_FILE_BYTES + 1)
        .expect("sparse resize must succeed");
    drop(big);
    let locale: unic_langid::LanguageIdentifier = "en-US".parse().expect("locale must parse");
    let err = load_locale_resources(dir.path(), &locale).expect_err("over-limit file must fail");
    match &err {
        LoaderError::OverBudget { limit_desc, .. } => {
            assert_eq!(*limit_desc, "max bytes per file");
        }
        other => panic!("over-limit file must be OverBudget, got: {other:?}"),
    }
    assert_eq!(
        err.path(),
        Some(locale_dir.join("big.ftl").as_path()),
        "the error must name the offending file"
    );

    // Exact and zero with explicit budgets on a 6-byte file: exact reads
    // through untouched, one byte under refuses from the capped reader.
    let dir = tempfile::tempdir().expect("temp pack must be creatable");
    let locale_dir = dir.path().join("en-US");
    std::fs::create_dir(&locale_dir).expect("locale dir must be creatable");
    std::fs::write(locale_dir.join("small.ftl"), "k = V\n").expect("seed must be writable");
    let base = dir.path();
    assert_eq!(
        load_locale_resources_with_budget(base, &locale, 6, DEFAULT_MAX_LOCALE_FILES)
            .expect("exact-budget read must succeed")
            .len(),
        1
    );
    let err = load_locale_resources_with_budget(base, &locale, 5, DEFAULT_MAX_LOCALE_FILES)
        .expect_err("one byte under budget must fail");
    assert!(
        matches!(err, LoaderError::OverBudget { .. }),
        "capped read must be OverBudget, got: {err:?}"
    );
    let err = load_locale_resources_with_budget(base, &locale, 0, DEFAULT_MAX_LOCALE_FILES)
        .expect_err("zero budget must refuse everything");
    assert!(
        matches!(err, LoaderError::OverBudget { .. }),
        "zero budget must be OverBudget, got: {err:?}"
    );
}

#[test]
fn file_count_budget_cannot_be_bypassed_by_tiny_files() {
    // Each file is far under the per-file budget, but there are more files
    // than allowed: the count gate must fire. Uses explicit small budgets
    // so the test writes 3 files, not 257.
    let dir = tempfile::tempdir().expect("temp pack must be creatable");
    let locale_dir = dir.path().join("en-US");
    std::fs::create_dir(&locale_dir).expect("locale dir must be creatable");
    for index in 0..3 {
        std::fs::write(locale_dir.join(format!("f{index}.ftl")), "k = V\n")
            .expect("tiny file must be writable");
    }
    let locale: unic_langid::LanguageIdentifier = "en-US".parse().expect("locale must parse");
    let err = load_locale_resources_with_budget(dir.path(), &locale, 1024, 2)
        .expect_err("third file past a 2-file budget must fail");
    match &err {
        LoaderError::OverBudget { limit_desc, .. } => {
            assert_eq!(*limit_desc, "max files per locale");
        }
        other => panic!("over-count must be OverBudget, got: {other:?}"),
    }
    // And a zero file-count budget refuses even a single tiny file.
    let err = load_locale_resources_with_budget(dir.path(), &locale, 1024, 0)
        .expect_err("zero file-count budget must refuse everything");
    assert!(
        matches!(err, LoaderError::OverBudget { .. }),
        "zero file-count budget must be OverBudget, got: {err:?}"
    );
    // Default budgets still accept the default-count shape from the
    // sibling per-file test (256 files is exercised in-module).
    const {
        assert!(
            DEFAULT_MAX_LOCALE_FILES >= 3,
            "defaults must cover this pack shape"
        );
    }
}

#[test]
fn directory_shapes_fail_typed_or_read_empty() {
    let locale: unic_langid::LanguageIdentifier = "en-US".parse().expect("locale must parse");
    // Missing locale dir is a real error, not an empty vec.
    let dir = tempfile::tempdir().expect("temp pack must be creatable");
    let err = load_locale_resources(dir.path(), &locale).expect_err("missing locale must fail");
    assert!(
        matches!(err, LoaderError::ReadDir { .. }),
        "missing locale dir must be ReadDir, got: {err:?}"
    );
    // Missing base reads as zero locales.
    assert!(discover_available_locales(dir.path().join("never-created").as_path()).is_empty());
    // Stray entries never become locales: a file named like a locale and
    // a typo'd directory are skipped, not errors.
    std::fs::write(dir.path().join("en-US"), "a file, not a dir").expect("stray must be writable");
    std::fs::create_dir(dir.path().join("not_a_locale!!")).expect("typo dir must be creatable");
    assert!(
        discover_available_locales(dir.path()).is_empty(),
        "non-directory and non-locale names must be skipped"
    );
}
