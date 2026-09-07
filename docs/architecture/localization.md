# Localization (`CanaryLoc`)

Recorded as a founding constraint, not a later feature: **no user-facing
text is hardcoded in source code, anywhere in Canary or in a game built
with it.** Retrofitting localization onto an engine or a game after
strings are scattered through source as literals is a well-known,
expensive migration — every engine that's shipped in multiple languages
has a story about it. Canary avoids the retrofit by making "user-facing
text is a lookup, not a literal" a constraint from the first UI widget
onward, the same "cheap now, expensive later" logic already applied to
[project state and versioning](state-and-versioning.md).

## What counts as "user-facing"

A real, checkable distinction, not a vague aspiration: text a **player or
end user** sees (UI labels, in-game dialogue, item/ability names,
tooltips, error messages surfaced in a shipped game) goes through
localization. Text a **developer** sees (Rust panics, `tracing` log
output, internal error `Display` messages, code comments) does not — it
stays plain English, since translating developer-facing diagnostics has
no real audience and would only add friction to debugging. Any string
crossing from "engine-internal" to "shown in `CanaryUI`" is the line.

## Mechanism: keys, not literals

User-facing text is referenced by a stable key, resolved at runtime
against the active locale's loaded string table — never embedded as a
literal in the call site:

```rust
// Not this:
canary_ui::label("Start Game");

// This:
canary_ui::label(loc::key!("main-menu-start-game"));
```

The key is the stable identifier (safe to reference from code, from
`CanaryUI` layouts, from save data); the *text* lives entirely in
per-locale resource files, editable by translators who never need to
touch source code or recompile anything.

## Format: Fluent (FTL)

**[Project Fluent](https://projectfluent.org/)** (`.ftl` files) is the
target format, via the `fluent`/`fluent-bundle` crates from the
`fluent-rs` family — a mature, actively maintained, dual
MIT/Apache-2.0-licensed Rust implementation (directly compatible with
Canary's own licensing). Chosen over a plain key-value format (gettext
`.po`, flat JSON/TOML tables) specifically because Fluent's syntax
natively handles the parts of translation that a flat key-value table
handles badly: pluralization rules that differ by language, grammatical
gender, and value interpolation with correct word order — getting these
right per-language is the actual hard part of localization, and a naive
key-value format pushes that difficulty onto every translator by hand
instead of the tooling. See
[ADR 0015](../decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md)
for the full decision record, including why the wider ecosystem
(`fluent-templates`, `fluent-fallback`, `i18n-embed`) is deliberately
*not* adopted yet: `fluent-langneg` (a focused, already-transitive
dependency of `fluent`) covers locale-fallback-chain resolution without
needing them, and each of the three was found, by direct testing, to add
either a real limitation (`i18n-embed 0.15.4`'s asset-loading forcing
compile-time asset embedding regardless of feature flags) or acknowledged
API immaturity (`fluent-fallback`'s own documentation) not worth taking
on for `v0.0.5`.

```ftl
# locales/en-US/main.ftl
main-menu-start-game = Start Game
items-remaining = { $count ->
    [one] { $count } item remaining
   *[other] { $count } items remaining
}
```

## Fallback and missing translations

A locale that's missing a given key falls back to a configured default
locale (typically the locale development happens in) rather than showing
a blank string or a raw key. Locale-fallback-chain resolution is handled
via `fluent-langneg` directly rather than `fluent-fallback`'s higher-level
`Localization` abstraction — see
[ADR 0015](../decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md)
for why. Missing-translation fallback firing is logged (developer-facing,
not player-facing, via `tracing`) so gaps are discoverable during
development rather than only reported by a confused player — implemented
in `LocaleBundle::resolve`, with the logging itself verified to actually
fire (via a real capturing `tracing` layer in `canary-loc`'s own tests),
not just present in the source.

## Relationship to `CanaryUI` and to project state

- **`CanaryUI`** ([`ui-toolkit.md`](ui-toolkit.md)) is the primary
  consumer: every built-in widget that takes text takes a localization
  key, not a `&str` literal, at the API level — this is enforceable at
  the type level (a `LocKey` newtype rather than accepting any `impl
  Into<String>`), which is a stronger guarantee than a coding-standard
  reminder alone. See
  [`docs/development/coding-standards.md`](../development/coding-standards.md)
  for how this pairs with the existing "no leaked third-party types"
  review discipline — this is the same kind of type-level enforcement,
  aimed at a different footgun.
- **Locale resource files are versionable, diffable project data** —
  `.ftl` files are plain structured text, which means they already
  satisfy [`state-and-versioning.md`](state-and-versioning.md)'s
  "diffable and mergeable" preference with zero extra design work.
  A future translation contribution is, mechanically, a normal pull
  request touching a text file, not a special workflow.
- **A future translation pack is naturally a marketplace package** in the
  sense already scoped in [ADR 0012](../decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md) —
  another concrete instance of that ADR's thesis that Git-friendliness,
  modding, and marketplace packages should fall out of one architecture
  rather than needing to be built as separate features.

## Status in this foundation

**Implemented, not just scoped** — see
[`v0.0.5-roadmap.md`](../roadmap/v0.0.5-roadmap.md) and
[ADR 0015](../decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md).
`engine/canary-loc` is a real workspace crate:

- **`LocKey` and `canary_loc::key!`** (the real macro path — this
  document's earlier `loc::key!(...)` example assumes a `use
  canary_loc as loc;` alias, which is the caller's choice, not something
  this crate does for them), validating Fluent's identifier grammar at
  **compile time**. Confirmed against `fluent-syntax`'s own parser
  source, not assumed from the Fluent spec alone — a syntactically
  invalid key is a real, proven build failure (a `compile_fail` doctest
  checks this), not a documented intention.
- **`LocaleBundle`**, resolving keys against a locale-fallback chain
  computed via `fluent-langneg` — proven against real `.ftl` content in
  direct tests, not `CanaryUI`: a plain message, a pluralized message
  correctly branching between CLDR categories, string interpolation, an
  unavailable requested locale correctly falling back to the configured
  default, and a missing key returning the raw key string (not a panic,
  not a blank string) while emitting a real, verified `tracing::warn!`
  event — verified with an actual capturing `tracing` layer, not
  inferred from the returned value, which caught two real bugs in the
  test harness itself before it could pass honestly.
- **The `std::fs`-based placeholder loader**
  (`discover_available_locales`/`load_locale_resources`), doc-commented
  as temporary and naming `canary-assets` as its intended replacement,
  deliberately decoupled from `LocaleBundle` itself (which takes any
  loader closure) so that replacement doesn't require touching
  `LocaleBundle`'s own logic.

**Still deliberately deferred**, per the roadmap and ADR 0015: `CanaryUI`
integration (no widget API exists yet), compile-time validation that a
key resolves against real `.ftl` content (only its syntax is checked),
`fluent-templates`/`fluent-fallback`/`i18n-embed`, and any translator
tooling (Weblate or otherwise) — real, intended, just not this crate's
job yet.
