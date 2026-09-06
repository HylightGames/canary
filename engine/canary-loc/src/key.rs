// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! [`LocKey`] and the [`key!`] macro — the only way to construct one.
//!
//! See `docs/architecture/localization.md`'s "Mechanism: keys, not
//! literals" and
//! [ADR 0015](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md)
//! for why this is a distinct type (enforced at the type level) rather
//! than `impl Into<String>` at every call site that takes user-facing
//! text.

/// A stable, validated reference to a piece of user-facing text, resolved
/// at runtime against the active locale's loaded string table — never a
/// literal string shown directly. Constructed only via [`key!`], which
/// validates the key's *syntax* at compile time (see that macro's docs
/// for exactly what "valid" means, and what's deliberately not checked
/// yet).
///
/// Two `LocKey`s are equal exactly when their underlying key strings are
/// equal — this is a stable identifier comparison, independent of which
/// locale (if any) is currently active or what text it currently
/// resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocKey(&'static str);

impl LocKey {
    /// Constructs a `LocKey` without validating `key`'s syntax. Not
    /// exposed as `pub` — [`key!`] is the only real construction path,
    /// since skipping the syntax check here would defeat the entire
    /// point of this type. Exists as a separate function (rather than
    /// inlining the construction into the macro) purely so the macro
    /// expansion stays small and its intent — "validate, then wrap" —
    /// stays readable at the call site.
    #[doc(hidden)]
    pub const fn __new_unchecked(key: &'static str) -> Self {
        Self(key)
    }

    /// The underlying key string. Not meant for display to a player —
    /// this is the stable identifier, not resolved text (see
    /// `docs/architecture/localization.md`'s "What counts as
    /// 'user-facing'" for that distinction) — but useful for logging,
    /// debugging, and this crate's own fallback reporting.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for LocKey {
    /// Displays the raw key string — again, not resolved text. Exists so
    /// a `LocKey` can appear directly in a `tracing` field or an
    /// unresolved-key fallback message without every call site needing
    /// `.as_str()`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Whether `s` matches Fluent's identifier grammar
/// (`[a-zA-Z][a-zA-Z0-9_-]*`) — confirmed against `fluent-syntax`'s own
/// parser source (`is_identifier_start`/`get_identifier_unchecked` in
/// its `parser` module), not assumed from the Fluent spec alone, since
/// what actually matters here is matching what `fluent`'s own parser
/// will accept as a message identifier, not a close paraphrase of it.
///
/// `const fn` so [`key!`] can evaluate this at compile time.
const fn is_valid_fluent_identifier(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() {
        let b = bytes[i];
        if !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return false;
        }
        i += 1;
    }
    true
}

/// Panics (at compile time, when called from a `const` context — see
/// [`key!`]) if `key` doesn't match Fluent's identifier grammar. A
/// separate function from [`is_valid_fluent_identifier`] purely so the
/// panic message lives in one place rather than being duplicated at
/// every call site.
#[doc(hidden)]
pub const fn __validate_key_syntax_or_panic(key: &str) {
    if !is_valid_fluent_identifier(key) {
        panic!(
            "invalid localization key: must match Fluent's identifier grammar, \
             [a-zA-Z][a-zA-Z0-9_-]* -- starts with a letter, then any mix of \
             letters, digits, `-`, and `_`"
        );
    }
}

/// Constructs a [`LocKey`] from a string literal, validating its syntax
/// against Fluent's identifier grammar **at compile time** — an invalid
/// key is a build failure, not a runtime surprise. See this module's
/// docs for the exact grammar, confirmed against `fluent-syntax`'s own
/// parser.
///
/// ```
/// # use canary_loc::key;
/// let k = key!("main-menu-start-game");
/// assert_eq!(k.as_str(), "main-menu-start-game");
/// ```
///
/// A syntactically invalid key fails to compile:
///
/// ```compile_fail
/// # use canary_loc::key;
/// let _ = key!("1-starts-with-a-digit"); // fails to compile
/// ```
///
/// **Not validated by this macro**: that `key` actually resolves against
/// any particular locale's loaded `.ftl` files — only its syntax. See
/// [ADR 0015](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md)
/// for why that's deliberately deferred rather than attempted here.
#[macro_export]
macro_rules! key {
    ($key:literal) => {{
        const _: () = $crate::__validate_key_syntax_or_panic($key);
        $crate::LocKey::__new_unchecked($key)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_realistic_key() {
        let k = key!("main-menu-start-game");
        assert_eq!(k.as_str(), "main-menu-start-game");
    }

    #[test]
    fn accepts_digits_hyphens_and_underscores_after_the_first_character() {
        let k = key!("item-42_special-case");
        assert_eq!(k.as_str(), "item-42_special-case");
    }

    #[test]
    fn two_keys_with_the_same_string_are_equal() {
        assert_eq!(key!("same-key"), key!("same-key"));
    }

    #[test]
    fn two_keys_with_different_strings_are_not_equal() {
        assert_ne!(key!("key-one"), key!("key-two"));
    }

    #[test]
    fn display_shows_the_raw_key_not_resolved_text() {
        let k = key!("main-menu-start-game");
        assert_eq!(k.to_string(), "main-menu-start-game");
    }

    #[test]
    fn rejects_empty_string_at_compile_time_check_function() {
        // Exercises the underlying const fn directly, since a real empty
        // string literal can't be passed to `key!` in a test without
        // failing this test file's own compilation -- see
        // `tests/key_compile_fail.rs` for the actual compile-fail
        // coverage of `key!` itself.
        assert!(!is_valid_fluent_identifier(""));
    }

    #[test]
    fn rejects_a_leading_digit() {
        assert!(!is_valid_fluent_identifier("1invalid"));
    }

    #[test]
    fn rejects_a_space() {
        assert!(!is_valid_fluent_identifier("has space"));
    }

    #[test]
    fn rejects_a_leading_hyphen() {
        assert!(!is_valid_fluent_identifier("-leading-hyphen"));
    }
}
