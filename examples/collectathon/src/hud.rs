//! The HUD build pass: score/goal readout plus the reset button.
//!
//! The build pass renders the game's immutable view into text and widgets
//! before the frame and hands the UI no live `World` borrow — the driver
//! calls this inside the backend's frame with last frame's extracted
//! state, so the HUD trails the scene by exactly one outer frame. That lag
//! is structural, not a bug: input routes UI-before-gameplay, so the build
//! pass runs before the simulation pass whose results it would need to be
//! current. Button presses return as intents; the simulation applies them
//! at its boundary ([`crate::game::register_apply_ui_intents`]).

use canary_ui_core::{UiBuilder, UiId};

/// The HUD reset button's id: the build pass below and the intents stage
/// agree on this value, never on a live `World` borrow.
pub const RESET_BUTTON: UiId = UiId::new("reset");

/// The immutable view the HUD renders: extracted from the world after the
/// previous frame's simulation pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct HudState {
    /// Points banked so far.
    pub score: u32,
    /// Pickups collected so far, goal included.
    pub collected: u32,
    /// Total pickups in the room, goal included.
    pub total: u32,
    /// True once every room pickup (goal included) is collected.
    pub goal_reached: bool,
    /// Player x in logical pixels.
    pub player_x: f32,
    /// Player y in logical pixels.
    pub player_y: f32,
}

/// Builds one HUD frame: a score/goal readout line plus the reset button.
pub fn build_hud(builder: &mut dyn UiBuilder, state: &HudState) {
    let goal = if state.goal_reached { "done" } else { "open" };
    builder.label(&format!(
        "Score: {}  Collected: {}/{}  Goal: {goal}  Player: ({:.0}, {:.0})",
        state.score, state.collected, state.total, state.player_x, state.player_y
    ));
    builder.button(RESET_BUTTON, "Reset");
}
