// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The HUD build pass: readouts plus the fire button.
//!
//! The build pass renders the game's immutable view into text and widgets
//! before the frame and hands the UI no live `World` borrow — the driver
//! calls this inside the backend's frame with last frame's extracted
//! state, so the HUD trails the scene by exactly one outer frame. Button
//! presses return as intents; the simulation applies them at its boundary
//! ([`crate::game::register_apply_ui_intents`]).

use canary_ui_core::UiBuilder;

use crate::game::FIRE_BUTTON;

/// The immutable view the HUD renders: extracted from the world after the
/// previous frame's simulation pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct HudState {
    /// Shots fired so far.
    pub shots: u32,
    /// Player x in logical pixels.
    pub player_x: f32,
    /// Player y in logical pixels.
    pub player_y: f32,
}

/// Builds one HUD frame: a readout line plus the fire button.
pub fn build_hud(builder: &mut dyn UiBuilder, state: &HudState) {
    builder.label(&format!(
        "Shots: {}  Player: ({:.0}, {:.0})",
        state.shots, state.player_x, state.player_y
    ));
    builder.button(FIRE_BUTTON, "Fire");
}
