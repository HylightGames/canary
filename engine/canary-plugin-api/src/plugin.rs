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
/// regardless of which tier loaded it: native (Tier B, implemented) today,
/// sandboxed WASM (Tier A) once implemented. See
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
