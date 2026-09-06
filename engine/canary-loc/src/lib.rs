// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine localization: [`LocKey`] + Fluent (`.ftl`) resolution.
//!
//! See `docs/architecture/localization.md` and
//! [ADR 0015](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0015-localization-format-and-key-mechanism.md)
//! for the full design. In short: user-facing text is referenced by a
//! [`LocKey`] (constructed via [`key!`]), never a literal string, and
//! resolved at runtime against the active locale's loaded `.ftl` string
//! table via [`LocaleBundle`].

mod key;
mod loader;

pub use key::LocKey;
pub use loader::{discover_available_locales, load_locale_resources, LoaderError};

// Re-exported so `key!`'s macro-generated code can refer to these
// without every consumer needing its own `fluent` dependency just to
// call the macro.
#[doc(hidden)]
pub use key::__validate_key_syntax_or_panic;
