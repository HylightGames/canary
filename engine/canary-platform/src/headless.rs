// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use crate::input::{InputEvent, InputSource};
use crate::window::{Window, WindowDescriptor};

/// A [`Window`] implementation that never opens a real OS window.
///
/// Used for headless servers, tests, and — in this foundation — the
/// entire `canary-runtime` boot harness, since no real windowing backend
/// exists yet. See
/// `docs/architecture/platform-abstraction.md#status-in-this-foundation`.
pub struct HeadlessWindow {
    descriptor: WindowDescriptor,
    close_requested: bool,
}

impl HeadlessWindow {
    /// Creates a headless window with the given descriptor. No real window
    /// is created; `descriptor` is only stored and reported back.
    pub fn new(descriptor: WindowDescriptor) -> Self {
        Self {
            descriptor,
            close_requested: false,
        }
    }

    /// Test/harness hook: simulate a close request, as a real backend
    /// would report when the user clicks the window's close button.
    pub fn request_close(&mut self) {
        self.close_requested = true;
    }
}

impl Window for HeadlessWindow {
    fn descriptor(&self) -> &WindowDescriptor {
        &self.descriptor
    }

    fn should_close(&self) -> bool {
        self.close_requested
    }

    fn poll_events(&mut self) {
        // A real backend pumps the OS event loop here; there is none to
        // pump in headless mode.
    }
}

/// An [`InputSource`] implementation that never produces real OS input on
/// its own; events are injected via [`HeadlessInput::inject`], which is
/// how tests and headless harnesses simulate input.
#[derive(Default)]
pub struct HeadlessInput {
    queued: Vec<InputEvent>,
}

impl HeadlessInput {
    /// Creates an input source with no queued events.
    pub fn new() -> Self {
        Self::default()
    }

    /// Test/harness hook: queue an event as if it came from the OS. The
    /// next [`InputSource::poll`] call will return it.
    pub fn inject(&mut self, event: InputEvent) {
        self.queued.push(event);
    }
}

impl InputSource for HeadlessInput {
    fn poll(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.queued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Key;

    #[test]
    fn headless_window_reports_its_descriptor_and_close_state() {
        let descriptor = WindowDescriptor {
            title: "Test".into(),
            width: 640,
            height: 480,
        };
        let mut window = HeadlessWindow::new(descriptor.clone());
        assert_eq!(window.descriptor(), &descriptor);
        assert!(!window.should_close());

        window.request_close();
        assert!(window.should_close());

        // Should not panic even though there's no real event loop.
        window.poll_events();
    }

    #[test]
    fn headless_input_returns_injected_events_once() {
        let mut input = HeadlessInput::new();
        assert_eq!(input.poll(), Vec::new());

        input.inject(InputEvent::KeyPressed(Key::Space));
        input.inject(InputEvent::KeyReleased(Key::Escape));

        assert_eq!(
            input.poll(),
            vec![
                InputEvent::KeyPressed(Key::Space),
                InputEvent::KeyReleased(Key::Escape),
            ]
        );
        // Events are consumed by `poll`, not re-delivered.
        assert_eq!(input.poll(), Vec::new());
    }
}
