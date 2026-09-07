// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! [`LocaleBundle`]: resolves a [`LocKey`] against a locale's `.ftl`
//! content, falling back through a negotiated locale chain when the
//! primary locale is missing a key (or is entirely unavailable) rather
//! than showing a blank string or panicking.

use fluent::{FluentArgs, FluentBundle, FluentResource};
use fluent_langneg::{negotiate_languages, NegotiationStrategy};
use unic_langid::LanguageIdentifier;

use crate::key::LocKey;

/// Resolves [`LocKey`]s against a locale's loaded `.ftl` content, falling
/// back through a negotiated chain of locales (computed via
/// `fluent-langneg`) when the primary locale can't resolve a given key —
/// see `docs/architecture/localization.md`'s "Fallback and missing
/// translations".
///
/// Deliberately decoupled from *how* `.ftl` content is loaded: construct
/// this with a loader closure, which can be the real (if temporary)
/// `std::fs`-based one in [`crate::load_locale_resources`], or synthetic
/// in-memory resources for a test. See `docs/roadmap/v0.0.5-roadmap.md`
/// for why that separation matters.
pub struct LocaleBundle {
    /// Ordered from highest to lowest priority. The last entry is always
    /// the configured default locale (`fluent-langneg`'s own contract —
    /// see [`LocaleBundle::new`]'s use of
    /// [`fluent_langneg::negotiate_languages`]), even if no `.ftl`
    /// content could actually be loaded for it — an empty bundle simply
    /// never resolves anything, which [`LocaleBundle::resolve`] already
    /// handles by falling through to the next candidate (or, for the
    /// last one, to the raw-key fallback).
    bundles: Vec<(LanguageIdentifier, FluentBundle<FluentResource>)>,
}

impl LocaleBundle {
    /// Negotiates a fallback chain from `requested` (highest to lowest
    /// preference — typically the player's OS/game-configured locale
    /// first) against `available` (the locales real `.ftl` content
    /// actually exists for, e.g. from
    /// [`crate::discover_available_locales`]), always ending at
    /// `default` even if `default` isn't itself present in `available`
    /// (confirmed against `fluent-langneg`'s own source, not assumed:
    /// its `default` parameter doesn't have to alias an element of
    /// `available` to be appended as the final fallback candidate).
    ///
    /// `load` is called once per negotiated locale to get that locale's
    /// `.ftl` resources; a locale the loader can't provide resources for
    /// (e.g. it returns an empty `Vec`) simply never resolves a key,
    /// which [`LocaleBundle::resolve`] treats the same as "this locale's
    /// bundle didn't have that key" — falling through to the next
    /// candidate rather than treating it as a construction error, since
    /// a missing lower-priority locale shouldn't be fatal when a
    /// higher-priority one (or the default) might still cover the key
    /// being looked up.
    pub fn new(
        requested: &[LanguageIdentifier],
        available: &[LanguageIdentifier],
        default: LanguageIdentifier,
        mut load: impl FnMut(&LanguageIdentifier) -> Vec<FluentResource>,
    ) -> Self {
        let negotiated = negotiate_languages(
            requested,
            available,
            Some(&default),
            NegotiationStrategy::Filtering,
        );

        let bundles = negotiated
            .into_iter()
            .map(|locale| {
                let locale = locale.clone();
                let mut bundle = FluentBundle::new(vec![locale.clone()]);
                for resource in load(&locale) {
                    if let Err(errors) = bundle.add_resource(resource) {
                        tracing::warn!(
                            locale = %locale,
                            ?errors,
                            "some .ftl entries could not be added to the bundle for this \
                             locale, most likely duplicate keys across multiple .ftl files"
                        );
                    }
                }
                (locale, bundle)
            })
            .collect();

        Self { bundles }
    }

    /// Resolves `key` against this bundle's negotiated locale chain,
    /// trying the highest-priority locale first and falling back through
    /// the rest in order. A key that resolves via any locale other than
    /// the first is logged (developer-facing, via `tracing::debug!`) as
    /// a fallback — normal, expected behavior, not a bug, but worth
    /// being able to see. A key that doesn't resolve in *any* locale in
    /// the chain is logged (via `tracing::warn!`, a real gap worth
    /// noticing) and the raw key string is returned rather than a blank
    /// string or a panic — a widely-used pattern (React Intl, i18next,
    /// and others do the same) precisely because a visibly-wrong string
    /// like `main-menu-start-game` is immediately recognizable as "this
    /// key is missing," where a blank label looks like nothing is wrong
    /// at all.
    pub fn resolve(&self, key: &LocKey, args: Option<&FluentArgs>) -> String {
        for (index, (locale, bundle)) in self.bundles.iter().enumerate() {
            let Some(message) = bundle.get_message(key.as_str()) else {
                continue;
            };
            let Some(pattern) = message.value() else {
                continue;
            };

            let mut errors = Vec::new();
            let resolved = bundle.format_pattern(pattern, args, &mut errors);
            if !errors.is_empty() {
                tracing::warn!(
                    key = %key,
                    locale = %locale,
                    ?errors,
                    "formatting errors while resolving a localization key"
                );
            }
            if index > 0 {
                tracing::debug!(
                    key = %key,
                    requested_locale = %self.bundles[0].0,
                    resolved_locale = %locale,
                    "localization key resolved via a fallback locale, not the primary one"
                );
            }
            return resolved.into_owned();
        }

        tracing::warn!(
            key = %key,
            "localization key did not resolve in any locale in the fallback chain; \
             showing the raw key"
        );
        key.as_str().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(ftl: &str) -> FluentResource {
        FluentResource::try_new(ftl.to_string()).expect("test .ftl content should be valid")
    }

    fn langid(s: &str) -> LanguageIdentifier {
        s.parse()
            .expect("test locale string should be a valid locale identifier")
    }

    #[test]
    fn resolves_a_plain_message() {
        let en = langid("en-US");
        let bundle = LocaleBundle::new(&[en.clone()], &[en.clone()], en, |_| {
            vec![resource("main-menu-start-game = Start Game")]
        });

        let resolved = bundle.resolve(&crate::key!("main-menu-start-game"), None);
        assert_eq!(resolved, "Start Game");
    }

    #[test]
    fn resolves_a_pluralized_message_across_multiple_counts() {
        let en = langid("en-US");
        let bundle = LocaleBundle::new(&[en.clone()], &[en.clone()], en, |_| {
            vec![resource(
                "items-remaining = { $count ->\n    [one] { $count } item remaining\n   *[other] { $count } items remaining\n}",
            )]
        });

        let mut args_one = FluentArgs::new();
        args_one.set("count", 1);
        let resolved_one = bundle.resolve(&crate::key!("items-remaining"), Some(&args_one));
        assert!(
            resolved_one.contains('1')
                && resolved_one.contains("item remaining")
                && !resolved_one.contains("items"),
            "expected the singular branch for count=1, got: {resolved_one:?}"
        );

        let mut args_many = FluentArgs::new();
        args_many.set("count", 5);
        let resolved_many = bundle.resolve(&crate::key!("items-remaining"), Some(&args_many));
        assert!(
            resolved_many.contains('5') && resolved_many.contains("items remaining"),
            "expected the plural branch for count=5, got: {resolved_many:?}"
        );
    }

    #[test]
    fn resolves_a_message_with_string_interpolation() {
        let en = langid("en-US");
        let bundle = LocaleBundle::new(&[en.clone()], &[en.clone()], en, |_| {
            vec![resource("welcome-player = Welcome, { $name }!")]
        });

        let mut args = FluentArgs::new();
        args.set("name", "Cloudy");
        let resolved = bundle.resolve(&crate::key!("welcome-player"), Some(&args));
        assert!(
            resolved.contains("Welcome,") && resolved.contains("Cloudy"),
            "expected the interpolated name to appear, got: {resolved:?}"
        );
    }

    #[test]
    fn a_missing_key_returns_the_raw_key_not_a_panic_or_blank_string() {
        let en = langid("en-US");
        let bundle = LocaleBundle::new(&[en.clone()], &[en.clone()], en, |_| {
            vec![resource("real-key = Real Text")]
        });

        let resolved = bundle.resolve(&crate::key!("this-key-does-not-exist"), None);
        assert_eq!(resolved, "this-key-does-not-exist");
    }

    #[test]
    fn an_unavailable_requested_locale_falls_back_to_the_default() {
        let requested = langid("fr-FR");
        let default = langid("en-US");
        // Only en-US content actually exists.
        let bundle = LocaleBundle::new(&[requested], &[default.clone()], default, |_| {
            vec![resource("greeting = Hello!")]
        });

        let resolved = bundle.resolve(&crate::key!("greeting"), None);
        assert_eq!(
            resolved, "Hello!",
            "should have fallen back to the default locale's content"
        );
    }

    #[test]
    fn a_default_locale_with_no_actual_content_still_falls_back_to_the_raw_key_gracefully() {
        // A real misconfiguration: the default locale is requested, but
        // the loader has nothing for it (e.g. a missing/renamed
        // directory) -- this must not panic.
        let default = langid("en-US");
        let bundle = LocaleBundle::new(&[default.clone()], &[], default, |_| vec![]);

        let resolved = bundle.resolve(&crate::key!("anything"), None);
        assert_eq!(resolved, "anything");
    }
}
