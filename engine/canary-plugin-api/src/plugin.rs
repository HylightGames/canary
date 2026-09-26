// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

/// The lifecycle every loaded Canary plugin exposes to engine code,
/// regardless of which tier loaded it: native (Tier B) or sandboxed WASM
/// (Tier A, shipped in `v0.0.3`). See
/// `docs/architecture/plugin-system.md`.
pub trait Plugin {
    /// A human-readable plugin name, used in logging and (eventually)
    /// marketplace/capability-review UI.
    fn name(&self) -> String;

    /// Called once after the plugin is loaded.
    fn on_load(&mut self);

    /// Called once before the plugin is unloaded.
    fn on_unload(&mut self);
}

/// Which lifecycle callback a scoped Tier A invocation runs.
///
/// Closed to these two until a later ADR opens it: R-34 ships no
/// per-frame hook and no editor-panel API (see
/// `docs/reviews/2026-09-r34-api-review.md` §8). The guest cannot
/// observe or branch on this value — it is reported in host-side
/// errors only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginPhase {
    /// The `on-load` lifecycle callback.
    OnLoad,
    /// The `on-unload` lifecycle callback.
    OnUnload,
}

/// How the runtime registered a plugin: whether its failure aborts the
/// run or degrades to a logged skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRequirement {
    /// A load- or unload-time failure aborts startup/teardown with a
    /// typed error.
    Required,
    /// A load-time failure is reported via `tracing::warn!` and the run
    /// continues without the plugin.
    Optional,
}
