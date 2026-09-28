//! UI input-capture results: which input classes the UI consumed this frame.

/// Which input classes the UI backend captured on the last frame.
///
/// The runtime hands this to gameplay input mapping *before* mapping:
/// events in a captured class are dropped unless a game-declared
/// per-binding pass-through covers that event (ADR 0025, decisions 3-4).
/// Gameplay code never reads backend-specific capture flags — this struct
/// is the whole contract, so swapping the UI backend cannot change what
/// gameplay observes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CaptureResult {
    /// `true` when keyboard input went to a UI widget this frame.
    pub keyboard: bool,
    /// `true` when pointer input went to a UI widget this frame
    /// (hover, press, drag, or a focused control holding the pointer).
    pub pointer: bool,
}

impl CaptureResult {
    /// Nothing captured: every event stays available to gameplay mapping.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            keyboard: false,
            pointer: false,
        }
    }

    /// Everything captured: the mapper drops all gameplay events that lack
    /// an explicit per-binding pass-through.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            keyboard: true,
            pointer: true,
        }
    }

    /// `true` when at least one input class was captured.
    #[must_use]
    pub const fn any(self) -> bool {
        self.keyboard || self.pointer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_captures_nothing() {
        let capture = CaptureResult::none();
        assert!(!capture.keyboard);
        assert!(!capture.pointer);
        assert!(!capture.any());
    }

    #[test]
    fn all_captures_everything() {
        let capture = CaptureResult::all();
        assert!(capture.keyboard);
        assert!(capture.pointer);
        assert!(capture.any());
    }

    #[test]
    fn any_is_true_when_either_class_is_captured() {
        let keyboard_only = CaptureResult {
            keyboard: true,
            ..CaptureResult::none()
        };
        let pointer_only = CaptureResult {
            pointer: true,
            ..CaptureResult::none()
        };
        assert!(keyboard_only.any());
        assert!(pointer_only.any());
        assert!(!CaptureResult::default().any());
    }
}
