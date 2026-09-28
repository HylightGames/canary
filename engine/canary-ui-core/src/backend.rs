//! Backend trait plus per-frame input/output plumbing.

use canary_platform::InputEvent;

use crate::capture::CaptureResult;
use crate::intent::{UiId, UiIntent};
use crate::paint::UiPaint;

/// Everything a backend needs to advance one UI frame.
#[derive(Debug)]
pub struct UiFrameInput<'a> {
    /// Normalized platform events since the last frame, poll order
    /// preserved. The backend translates these into its own input model;
    /// gameplay mapping receives the same slice plus the resulting
    /// [`CaptureResult`].
    pub events: &'a [InputEvent],
    /// Viewport width in logical pixels (UI points map 1:1).
    pub screen_width_px: f32,
    /// Viewport height in logical pixels.
    pub screen_height_px: f32,
    /// `false` on frames whose event stream contained `FocusLost`, `true`
    /// otherwise: an unfocused window receives no input events, so the
    /// presence of fresh events implies focus. The frame driver owns this
    /// flag; the platform emits `FocusLost` only (no regain event).
    pub focused: bool,
    /// Wall-clock delta since the last UI frame, in seconds.
    pub dt_seconds: f64,
}

/// Everything a backend returns from one UI frame.
#[derive(Debug)]
pub struct UiFrameOutput {
    /// Which input classes the UI captured (feeds gameplay mapping).
    pub capture: CaptureResult,
    /// Widget activations for the runtime to apply at the next simulation
    /// boundary, in activation order.
    pub intents: Vec<UiIntent>,
    /// Tessellated widget output plus texture uploads/releases.
    pub paint: UiPaint,
}

/// Immediate-mode widget surface the game builds its HUD with.
///
/// Implemented once per backend adapter; game code programs against this
/// trait, so swapping adapters never touches gameplay. Readouts take plain
/// `&str`: the game renders its immutable view into text before the frame
/// and hands the UI no live `World` borrow.
pub trait UiBuilder {
    /// Shows read-only text (a HUD readout line).
    fn label(&mut self, text: &str);
    /// Shows a button; returns `true` when the button was activated this
    /// frame. The adapter converts activation into
    /// [`UiIntent::ButtonPressed`](crate::intent::UiIntent::ButtonPressed).
    fn button(&mut self, id: UiId, label: &str) -> bool;
}

/// A UI backend: turns platform events plus one widget build pass into
/// capture results, user intents, and renderer-independent paint output.
pub trait UiBackend {
    /// Advances one UI frame: routes `input.events`, builds widgets via
    /// `build`, and returns what the frame captured, intended, and drew.
    fn run_frame(
        &mut self,
        input: &UiFrameInput<'_>,
        build: &mut dyn FnMut(&mut dyn UiBuilder),
    ) -> UiFrameOutput;
}

/// A backend with no widgets: captures nothing, intends nothing, draws
/// nothing, and never invokes the build closure (no widget tree exists to
/// build). For headless harnesses and driver tests — any frame that needs
/// real widgets needs a real backend.
#[derive(Debug, Default)]
pub struct NullBackend;

impl UiBackend for NullBackend {
    fn run_frame(
        &mut self,
        _input: &UiFrameInput<'_>,
        _build: &mut dyn FnMut(&mut dyn UiBuilder),
    ) -> UiFrameOutput {
        UiFrameOutput {
            capture: CaptureResult::none(),
            intents: Vec::new(),
            paint: UiPaint::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_backend_captures_nothing_and_draws_nothing() {
        let mut backend = NullBackend;
        let mut build_ran = false;
        let output = backend.run_frame(
            &UiFrameInput {
                events: &[],
                screen_width_px: 320.0,
                screen_height_px: 240.0,
                focused: true,
                dt_seconds: 1.0 / 60.0,
            },
            &mut |_| build_ran = true,
        );
        assert_eq!(output.capture, CaptureResult::none());
        assert!(output.intents.is_empty());
        assert!(output.paint.is_empty());
        assert!(
            !build_ran,
            "no widget tree exists to build, so the build closure must never run"
        );
    }
}
