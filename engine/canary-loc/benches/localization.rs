// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Benchmarks for localization: building a negotiated
//! [`LocaleBundle`](canary_loc::LocaleBundle) (startup and locale-switch
//! cost) and resolving keys through it (per-frame UI cost).
//!
//! Resolution is what a UI pays for every visible string, including the
//! fallback walk when a key is missing from the preferred locale, so both
//! the hit and the fallback paths are measured here.
//!
//! Run locally with `cargo bench -p canary-loc`; in CI these are measured
//! by CodSpeed (see `.github/workflows/codspeed.yml`).

use canary_loc::{key, LocKey, LocaleBundle};
use divan::{black_box, Bencher};
use fluent::{FluentArgs, FluentResource};
use unic_langid::LanguageIdentifier;

fn main() {
    divan::main();
}

/// String-table sizes (messages per locale) the benchmarks run at: a
/// small game's UI, and a content-heavy one.
const MESSAGE_COUNTS: &[usize] = &[64, 1_024];

/// Parses a locale identifier that is known-good at the call site.
fn langid(tag: &str) -> LanguageIdentifier {
    tag.parse().expect("benchmark locale tags are valid")
}

/// A synthetic `.ftl` string table with `count` plain messages, plus the
/// three keys the resolution benchmarks look up by name.
fn ftl_source(count: usize, prefix: &str) -> String {
    let mut source = String::new();
    for index in 0..count {
        source.push_str(&format!(
            "{prefix}-message-{index} = Message number {index}\n"
        ));
    }
    source.push_str("main-menu-start-game = Start Game\n");
    source.push_str("welcome-player = Welcome, { $name }!\n");
    source.push_str(
        "items-remaining = { $count ->\n    [one] { $count } item remaining\n   *[other] { $count } items remaining\n}\n",
    );
    source
}

/// Parses `source` into a Fluent resource, panicking on malformed
/// benchmark input (which would be a bug in this file, not a measurement).
fn resource(source: String) -> FluentResource {
    FluentResource::try_new(source).expect("benchmark .ftl content is valid")
}

/// A two-locale bundle: `en-US` requested and available, `de-DE` as the
/// lower-priority fallback that only carries the fallback-only key.
fn bundle_with(count: usize) -> LocaleBundle {
    let en = langid("en-US");
    let de = langid("de-DE");
    LocaleBundle::new(
        &[en.clone(), de.clone()],
        &[en.clone(), de.clone()],
        en.clone(),
        move |locale| {
            if *locale == en {
                vec![resource(ftl_source(count, "en"))]
            } else {
                vec![
                    resource(ftl_source(count, "de")),
                    resource("fallback-only-key = Nur im Deutschen\n".to_string()),
                ]
            }
        },
    )
}

/// Startup cost: negotiate the fallback chain, parse every `.ftl`
/// resource, and build one Fluent bundle per negotiated locale. Also the
/// cost of an in-game language switch.
#[divan::bench(args = MESSAGE_COUNTS)]
fn build_bundle(bencher: Bencher, count: usize) {
    bencher.bench_local(|| black_box(bundle_with(count)));
}

/// Parsing `.ftl` text on its own, separated from bundle construction.
#[divan::bench(args = MESSAGE_COUNTS)]
fn parse_ftl_resource(bencher: Bencher, count: usize) {
    let source = ftl_source(count, "en");
    bencher
        .with_inputs(|| source.clone())
        .bench_local_values(|source| black_box(resource(source)));
}

/// The common case: a key present in the highest-priority locale, with no
/// arguments to interpolate.
#[divan::bench(args = MESSAGE_COUNTS)]
fn resolve_plain_message(bencher: Bencher, count: usize) {
    let bundle = bundle_with(count);
    let key = key!("main-menu-start-game");
    bencher.bench_local(|| black_box(bundle.resolve(black_box(&key), None)));
}

/// Interpolation: one argument substituted into the pattern.
#[divan::bench(args = MESSAGE_COUNTS)]
fn resolve_message_with_argument(bencher: Bencher, count: usize) {
    let bundle = bundle_with(count);
    let key = key!("welcome-player");
    let mut args = FluentArgs::new();
    args.set("name", "Cloudy");
    bencher.bench_local(|| black_box(bundle.resolve(black_box(&key), Some(&args))));
}

/// Plural selection: the branch a Fluent runtime has to evaluate against
/// the locale's plural rules, not just substitute.
#[divan::bench(args = MESSAGE_COUNTS)]
fn resolve_pluralized_message(bencher: Bencher, count: usize) {
    let bundle = bundle_with(count);
    let key = key!("items-remaining");
    let mut args = FluentArgs::new();
    args.set("count", 5);
    bencher.bench_local(|| black_box(bundle.resolve(black_box(&key), Some(&args))));
}

/// A key the preferred locale doesn't carry: resolution walks down the
/// negotiated chain before it finds one that does.
#[divan::bench(args = MESSAGE_COUNTS)]
fn resolve_through_fallback_locale(bencher: Bencher, count: usize) {
    let bundle = bundle_with(count);
    let key = key!("fallback-only-key");
    bencher.bench_local(|| black_box(bundle.resolve(black_box(&key), None)));
}

/// A key no locale carries: the whole chain is walked and the raw key is
/// returned — the worst case, and the one a missing translation hits.
#[divan::bench(args = MESSAGE_COUNTS)]
fn resolve_missing_key(bencher: Bencher, count: usize) {
    let bundle = bundle_with(count);
    let key: LocKey = key!("no-locale-has-this-key");
    bencher.bench_local(|| black_box(bundle.resolve(black_box(&key), None)));
}
