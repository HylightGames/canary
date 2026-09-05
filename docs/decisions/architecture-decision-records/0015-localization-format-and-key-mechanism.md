# 0015. Localization format and key mechanism: Fluent via `fluent-rs`, a `LocKey` newtype, not a raw string

**Status:** Proposed

## Context

`v0.0.5` is scoped (see
[`v0.0.5-roadmap.md`](../../roadmap/v0.0.5-roadmap.md)) as the first
localization release, redirected from what an earlier draft of the
release sequence had pencilled in as rendering — see that roadmap for
why. [`docs/architecture/localization.md`](../../architecture/localization.md)
already recorded the founding constraint ("no user-facing text is
hardcoded, anywhere, ever") and a first-pass design sketch. This ADR is
where that sketch becomes a recorded decision, since it introduces new
dependencies into the trusted core and a type-level API constraint on
every future `CanaryUI` widget — exactly the two things
[ADR 0001](0001-record-format.md) names as warranting a record.

Three real questions had to be settled, not assumed:

1. **What source format do translators actually edit?** A flat
   key-value table (gettext `.po`, JSON/TOML) versus a format with
   native support for pluralization, grammatical gender, and
   interpolation word order.
2. **How is a call site prevented from slipping a raw string literal
   past the "no hardcoded text" constraint?** A coding-standard
   reminder, or something the type system actually enforces.
3. **Does the wider `fluent-rs`-adjacent ecosystem — `fluent-templates`,
   `fluent-fallback`, `i18n-embed` — actually work as advertised under
   this project's real constraints (rustc 1.75, no `CanaryUI`, no
   `canary-assets`)?** This one turned out not to have an assumed
   answer; it had to be built and run to find out (see "Verified, not
   assumed" below).

## Decision

**Format: [Project Fluent](https://projectfluent.org/) (`.ftl`), via the
`fluent`/`fluent-bundle` crates from the `fluent-rs` family**, chosen
over gettext `.po` or a flat JSON/TOML key-value table specifically
because Fluent's syntax handles pluralization rules, grammatical gender,
and interpolation word order natively — the parts of translation that
differ by language and that a flat key-value format pushes onto every
translator by hand instead of the tooling. `.ftl` files are plain
structured text, so they're diffable/mergeable project data for free,
consistent with [`state-and-versioning.md`](../../architecture/state-and-versioning.md)'s
existing preference.

**Mechanism: a `LocKey` newtype, not `impl Into<String>`.** Every
`CanaryUI` widget that takes user-facing text takes a `LocKey`, resolved
against the active locale's loaded bundle at render time — enforced at
the type level, the same category of guarantee
[`coding-standards.md`](../../development/coding-standards.md)'s
no-leaked-third-party-types review already applies to a different
footgun. A `LocKey` is constructed via a macro
(`loc::key!("main-menu-start-game")`) that can, at minimum, validate the
key's syntax at compile time; validating that the key actually exists in
at least the default locale's `.ftl` files is a real ergonomic win but
is explicitly not promised by this ADR — see `v0.0.5-roadmap.md` for
what's actually in scope for the first cut versus deferred.

**Scope of the dependency footprint, narrower than the pre-existing
sketch in `localization.md`:** `fluent` and `fluent-bundle` are the
load-bearing dependency. `fluent-langneg` (already a transitive
dependency of `fluent`) is the right tool for locale-fallback-chain
resolution — a focused, single-purpose crate for BCP-47 negotiation.
**`fluent-templates`, `fluent-fallback`, and `i18n-embed` are not
adopted in `v0.0.5`**, despite `localization.md` naming them as the
supporting ecosystem worth knowing about — see "Alternatives
considered" for what empirical testing found wrong with reaching for
them *now*, versus later once `canary-assets` exists to pair with them
properly.

## Alternatives considered

- **gettext (`.po`) or a flat JSON/TOML key-value table.** Rejected:
  neither handles per-language pluralization or grammatical gender
  natively; both would push that complexity onto every translator by
  hand. `.po` also brings a heavier, more specialized toolchain
  (`msgfmt` etc.) that Fluent's plain-text `.ftl` avoids.
- **A coding-standard reminder instead of a `LocKey` type.** Rejected
  for the same reason `coding-standards.md` gives for the
  no-leaked-third-party-types rule: a review discipline that isn't
  tooling-enforced degrades under time pressure and new-contributor
  turnover, on a multi-year project that explicitly wants
  tooling-enforced architecture over documentation-only standards.
- **Adopting `i18n-embed`'s `fluent-system` feature now, as
  `localization.md` sketched.** Tested directly, not assumed: with
  default features off and only `fluent-system` enabled,
  `i18n-embed 0.15.4`'s `assets.rs` module unconditionally
  `use rust_embed::RustEmbed`s regardless of which asset-loading
  feature is active — a real limitation in that release, not a
  configuration mistake here, confirmed by reading the crate's own
  source after the compile error pointed at it. Pulling in `rust-embed`
  to work around this means compile-time-embedded assets, which is the
  wrong shape for `v0.0.5` anyway: the actual need is a thin, obviously
  temporary runtime file-load (`std::fs::read_to_string` against a
  fixed `locales/` directory), clearly marked as a placeholder for
  `canary-assets`, not a second, incompatible asset-embedding mechanism
  introduced alongside it. `i18n-embed` is worth revisiting once
  `canary-assets` exists and its own loading story is what call sites
  should route through regardless.
- **Adopting `fluent-fallback`'s `Localization` abstraction now.**
  Rejected for `v0.0.5`, on the same evidence-over-assumption basis:
  read directly, the crate's own top-of-file documentation states
  plainly that "the functionality of this level is complete, but the
  API itself is in the early stages and the goal of being ergonomic is
  yet to be achieved," and its own example requires the separate
  `fluent-resmgr` crate (untested here) for locale-templated resource
  paths. Adopting an API that its own maintainers describe as
  pre-ergonomic, for a release whose entire point is proving the core
  runtime correct in isolation, would be taking on unnecessary surface
  for no present benefit. `fluent-langneg` directly — already in the
  dependency tree, single-purpose, and stable — covers what `v0.0.5`
  actually needs from locale fallback.
- **Validating that a `LocKey`'s string exists in the default locale's
  `.ftl` files at compile time (not just syntax-checking the key
  itself).** A real ergonomic win, genuinely desirable, and explicitly
  not ruled out for later — but it depends on a build-time step reading
  project-specific `.ftl` files, which is exactly the kind of thing
  that should live behind whatever `canary-assets`/`xtask` tooling
  exists once there is one, rather than being bolted onto `canary-loc`
  standalone. Deferred, not rejected.

## Consequences

- `canary-loc` becomes a new workspace crate with `fluent`/`fluent-bundle`
  (and their pinned transitive dependencies — see
  [`build-system.md`](../../development/build-system.md#the-rustc-175-sandbox-validation-floor))
  as its load-bearing external dependency. Anything that later wants a
  different format or mechanism is now a **format migration**, not a
  greenfield choice — Fluent's own maturity and dual MIT/Apache-2.0
  licensing were weighed with that in mind.
- Every future `CanaryUI` widget's text-accepting API is constrained to
  `LocKey` from the day `CanaryUI` starts being implemented, not
  retrofitted afterward — the entire point of treating this as a
  founding constraint rather than a later feature.
- `v0.0.5`'s asset loading is deliberately, visibly a placeholder
  (direct `std::fs` reads), not a second real asset-loading mechanism
  living alongside a future `canary-assets`. Whoever builds
  `canary-assets` should expect to replace this specific code path, not
  discover an accidental second implementation to reconcile.
- Re-adopting `fluent-templates`/`fluent-fallback`/`i18n-embed` later is
  explicitly left open, not foreclosed — this ADR records why *now*
  wasn't the right time, not that they're wrong tools in general.
