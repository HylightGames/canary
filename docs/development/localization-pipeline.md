# Localization pipeline (phase 1)

How a user-facing string travels from an author's `.ftl` edit to a
player's screen — and every gate it passes on the way. The runtime end
of this pipeline (`LocKey`, `LocaleBundle`, the `.ftl` loader) is
designed in [`docs/architecture/localization.md`](../architecture/localization.md)
and [ADR 0015](../decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md);
this document is the *process* around it: file layout, automated gates,
pseudo-locale testing, and the human Weblate workflow. Phase 1 means:
the gates exist and are proven working, but there is no locale content
yet (`locales/` does not exist — no UI strings exist to translate, so
inventing content files would be fiction), and Weblate itself is a
documented human procedure, not a configured integration.

## The stages

```text
author → test → lint/compare → pseudo → Weblate → pull → resolve
```

1. **Author.** A developer (or translator, via Weblate — stage 5) edits
   `locales/<lang>/main.ftl` (layout below). The default locale is
   `en-US`: it is the language development happens in and the key set
   every other locale is measured against.
2. **Test.** `engine/canary-loc`'s own suite proves resolution behavior
   against real `.ftl` content: plain messages, plural branching,
   interpolation, fallback-chain resolution, and the missing-key path
   (raw key returned plus a verified `tracing::warn!`). Run it with
   `cargo test -p canary-loc` (debug) and, for the release half of the
   debug-vs-release contract below,
   `cargo test --release -p canary-loc`.
3. **Lint/compare (CI).** The `localization` job in
   `.github/workflows/ci.yml` installs Mozilla's
   [`moz.l10n`](https://github.com/mozilla/moz-l10n) CLI at the pinned
   version (`moz.l10n==0.14.2`, checked 2026-09-20 as latest on PyPI)
   with system `python3` — no third-party GitHub Actions involved — and
   runs two gates over `locales/` once `.ftl` files exist:
   - `moz-l10n lint 'locales/**/*.ftl'` fails the job if any file does
     not parse. Note the quoted glob, expanded by `moz-l10n` itself:
     passing a bare `locales/` directory only visits files mirroring
     the reference locale's layout, so a broken extra file elsewhere is
     silently skipped (verified directly against 0.14.2). The glob is
     what makes this a real parse gate.
   - `moz-l10n compare --json <each non-default locale> --source
     locales/en-US`, with a small stdlib-`python3` check failing the
     job on any key missing relative to `en-US` (or any per-file
     error), surfaced as file-annotated `::error ::` lines. The wrapper
     exists because `compare` itself only *reports* — it exits 0 even
     with keys missing (verified) — so without enforcement it would be
     a dashboard, not a gate.
   - Until the first `.ftl` lands, the job installs the pinned tool,
     proves it executes (`moz-l10n --version`), and echoes an explicit
     `vacuous-active` note. Environment, pin, and invocation are
     exercised on every run, so the first content commit inherits a
     working gate with zero CI changes.
4. **Pseudo.** Before (or without) human translations, generated
   stand-in text proves the UI survives translation-shaped content —
   see "Pseudo-locales" below.
5. **Weblate.** Human translation happens in Weblate against this same
   GitHub repo — see "Weblate setup (human runbook)" below. Weblate
   commits (or pull requests) land as normal `.ftl` diffs, which rejoin
   the pipeline at stage 3 on the next CI run.
6. **Pull.** Translated `.ftl` updates arrive as ordinary git commits
   touching text files — no special workflow, no asset rebuild —
   consistent with the "diffable project data" property in the
   architecture doc.
7. **Resolve.** At runtime `LocaleBundle::resolve` looks the key up
   through the negotiated locale chain (`fluent-langneg`), formats the
   pattern with the caller's `FluentArgs`, and degrades gracefully per
   the contract below.

## File layout convention

```text
locales/
  en-US/
    main.ftl
  de-DE/
    main.ftl
```

- One directory per locale, named with a BCP-47 identifier that
  `unic_langid::LanguageIdentifier` parses (the loader,
  `discover_available_locales`, silently skips anything else and sorts
  what remains, so negotiation never depends on filesystem order).
- `main.ftl` is the starting file per locale. The loader
  (`load_locale_resources`) reads *every* `*.ftl` directly inside the
  locale directory, so splitting into more files later (per-feature,
  per-screen) needs no code or CI changes — but start with one file
  until there is enough content to justify the split.
- `locales/` intentionally does not exist yet: with no `CanaryUI`
  widgets there are no user-facing strings to translate, and an empty
  `en-US/main.ftl` (or invented example strings) would either trip the
  compare gate's source-empty check or, worse, look like real content.
  The first UI string authored is the commit that creates
  `locales/en-US/main.ftl` — and activates the CI gates above.
- This layout is what the Weblate component's file mask
  (`locales/*/main.ftl`) and the CI compare step (`--source
  locales/en-US`) both assume. Changing it means changing all three
  together.

## Debug-vs-release contract

`LocaleBundle::resolve` treats two failure classes differently, on
purpose:

| Failure | Meaning | Debug (`cargo test`, dev builds) | Release (shipped game) |
|---|---|---|---|
| Missing formatting argument: the pattern needs `$name` and the caller passed no/incomplete `args` | Bug at the **call site** (Rust code) | Loud `debug_assert!` failure naming the key, locale, and missing variable(s) — CI catches it | Unchanged resilient path: `tracing::warn!` + best-effort string (`{$name}` placeholder intact) |
| Any other formatting error (unknown message/term/function reference, cycle, missing select default) | Bad **content** (`.ftl` authoring/translator mistake) | Same as release: warn + best effort, never an assert | Warn + best effort |

Why the split exists: a missing argument can only be fixed by changing
the Rust call site, so it must surface where the programmer runs the
code — a subtly wrong string reaching a player is the failure mode
being prevented. Bad content, conversely, is exactly what stages 3–5
catch (lint, compare, pseudo tests, Weblate checks); asserting on it
inside the resolver would turn every translator typo into a crashed
dev build instead of a pipeline finding.

Why the scope is narrow (missing-args *only*): the assert keys on one
precise signal — `fluent` reporting an unknown *variable* reference for
a name absent from the caller's `FluentArgs` (see
`LocaleBundle::missing_arg_names`) — and nothing else. Widening it to
"any formatting error" would conflate caller bugs with content bugs and
reintroduce the crash-on-typo problem above.

This is Firefox's precedent, adopted at the same layer Firefox chose.
Mozilla Bug 1685180 ("Debug assert Fluent strings where replaced
variables are not provided", fixed in Firefox 109) added a debug assert
for exactly this caller bug in Firefox's own `localization-ffi` layer
— deliberately *not* inside `fluent-rs` upstream — after finding
missing-variable call sites the hard way; Bug 1453765's design ("throw
in automation, salvage as much as possible on release") is the same
two-profile contract. Canary follows both: assert in debug where the
programmer is watching, degrade gracefully where the player is.

## Pseudo-locales

A pseudo-locale is generated text standing in for real translations:
each source string run through `fluent-pseudo`'s
`transform(source, flipped = false, elongate = true)`, which replaces
Latin letters with accented lookalikes and doubles `a`/`e`/`o`/`u` to
emulate the ~30% growth real translations typically add (e.g. `Hello
World` → `Ħeeŀŀoo Ẇoořŀḓ` — verified against `fluent-pseudo 0.3.3`).
That exercises what actually breaks in translated UIs — overflow,
clipping, broken assumptions about string length, untranslated strings
visually obvious by their un-accented text — before any human
translator is involved.

- **Today (test support):** `fluent-pseudo` is a dev-dependency of
  `canary-loc` only — never linked into shipping builds — and the
  `pseudo_locale_generation_flows_end_to_end_through_resolve` test
  proves the full loop: inline en-US resource → pseudo-generate →
  load as a `qps-ploc` locale beside en-US → resolve through
  `LocaleBundle` → output shows expansion and accent markers. `qps-ploc`
  is Mozilla's conventional pseudo-locale tag (private-use `qps` +
  `ploc`); `unic-langid` accepts it as well-formed.
- **Production use (documented, not built):** once real UI exists,
  generate a `qps-ploc`-style locale at *build time* — transform every
  value in the default locale's resources, write the result to
  `locales/qps-ploc/main.ftl` as a build artifact (never committed),
  and run layout/overflow UI tests against it. `transform_dom`'s
  `with_markers` option (wrapping in `[...]`) is the convention for
  spotting strings that bypassed localization entirely. Building that
  generator is future work, most likely behind whatever `xtask`/asset
  tooling exists then; the test above is its specification-in-miniature.

## Weblate setup (human runbook)

Translation hosting is a human-driven setup, done once, when the first
real UI strings exist. No SaaS configuration lives in this repo; what
follows is the checklist for the person doing it:

1. **Account.** Create an account on
   [hosted.weblate.org](https://hosted.weblate.org/) (Weblate's gratis
   hosting for libre software — step 2 is the request for it).
2. **Libre hosting request.** Apply for free hosting as an open-source
   project, pointing at this repo's GitHub URL and license (MIT).
   Until approved, translation happens via direct `.ftl` pull requests
   — the pipeline works without Weblate, just with more manual review.
3. **Project + component.** Create a project for Canary, then a
   component per translatable file set with:
   - source-code repository: this repo's GitHub URL;
   - file mask: `locales/*/main.ftl` (matches the layout above; add
     masks if the single file ever splits);
   - file format: Fluent;
   - source language: `en-US` (the key set other locales are measured
     against, same as the CI compare step).
4. **Sync.** Connect Weblate to GitHub (OAuth or a deploy key with push
   access on a translation branch, per Weblate's GitHub-integration
   docs) so approved translations push back as commits and source
   changes pull into Weblate automatically. Every such commit rejoins
   this pipeline at the lint/compare stage.
5. **Glossary + checks.** Seed a glossary with project terms that must
   stay consistent (engine-specific vocabulary, proper nouns), and turn
   on Weblate's Fluent checks (mismatched placeholders, missing plurals)
   so translator-side mistakes are caught before they reach CI — defense
   in depth with stages 3–4, not a replacement for them.

## Translator conventions

The rules every `.ftl` edit — human or Weblate-mediated — must follow.
Violations are caught by stages 3–5, but knowing them saves a round trip:

- **Never change the key.** Keys are stable identifiers referenced from
  code; renaming one orphans every call site. Only the value after
  `=` is translatable.
- **Preserve every placeholder exactly.** `{ $name }`, `{ $count }`,
  and select-expression branches are code, not prose: same spelling,
  same `$` prefix. A dropped placeholder is a missing-argument
  fallback at best.
- **No concatenation.** If a sentence needs reordering for your
  language, reorder the Fluent pattern itself — never split one
  message into pieces assembled by the caller. (Fluent's "social
  contract", in Mozilla's tutorial wording: concatenation is
  discouraged precisely because translators cannot fix word order
  across fragments.)
- **Plurals need your language's categories, not English's.** A select
  on `$count` must cover the CLDR plural categories *your* locale
  requires (many languages need more than `one`/`other`), and every
  select keeps its `*[other]` default branch — the resolver falls back
  to it, and removing it is a content error.
- **Translate meaning, flag problems.** If a source string is ambiguous
  or untranslatable as written, fix (or file an issue against) the
  `en-US` source rather than working around it in translation — a
  workaround in one locale becomes ten divergent workarounds in ten.
