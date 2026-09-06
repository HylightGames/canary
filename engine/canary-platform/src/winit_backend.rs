// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! A real, `winit`-backed implementation of [`Window`]/[`InputSource`],
//! alongside (not replacing) [`crate::HeadlessWindow`]/
//! [`crate::HeadlessInput`]. Behind the `winit-backend` Cargo feature —
//! see `Cargo.toml` for why this isn't an unconditional dependency.
//!
//! ## The pull-model/push-model bridge
//!
//! [`Window::poll_events`] was written expecting a traditional "pump
//! events, return control" game-loop shape. Modern `winit` (0.30)
//! deliberately moved away from exactly that model in favor of
//! [`winit::application::ApplicationHandler`] callbacks driven by
//! `EventLoop::run_app`, which *takes over the calling thread* rather than
//! returning control per frame. The bridge here is
//! [`winit::platform::pump_events::EventLoopExtPumpEvents::pump_app_events`],
//! called with a zero timeout from inside [`WinitWindow::poll_events`].
//!
//! Worth being honest about, not discovered as a surprise later: `winit`'s
//! own documentation describes `pump_app_events` as supported for exactly
//! this compatibility case but discourages it beyond that. This is a real,
//! acknowledged tension between `winit`'s recommended
//! ownership-of-the-main-loop model and this engine's existing (and, for a
//! traditional game loop, more natural) per-frame-polling trait shape —
//! not a frictionless fit, but a deliberate, documented tradeoff.
//!
//! ## Why `WinitWindow` and `WinitInput` share state
//!
//! Unlike [`crate::HeadlessWindow`]/[`crate::HeadlessInput`], which are
//! fully independent of each other, a single `winit` event loop delivers
//! *both* window and input events together — there's no separate "input
//! event source" to pump independently. [`WinitWindow::poll_events`] is
//! the only place that actually pumps the event loop; it also records
//! observed input into state shared (via `Rc<RefCell<_>>`) with any
//! [`WinitInput`] created from it via [`WinitWindow::input_source`].
//! Concretely: call `poll_events()` on the window once per frame, then
//! `poll()` on its input source to drain what that pump observed — not
//! the other way around, and not from two unrelated objects each
//! expecting to pump independently.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::{Window as WinitOsWindow, WindowId};

use crate::input::{InputEvent, InputSource, Key};
use crate::window::{Window, WindowDescriptor};

/// Everything a live `winit` window needs, shared between [`WinitWindow`]
/// (which pumps the event loop) and any [`WinitInput`] created from it
/// (which only drains what the pump observed).
struct SharedState {
    os_window: Option<WinitOsWindow>,
    close_requested: bool,
    pending_input: Vec<InputEvent>,
}

/// The actual [`winit::application::ApplicationHandler`] implementation.
/// Constructed fresh (borrowing the shared state and descriptor) for each
/// [`WinitWindow::poll_events`] call, rather than stored long-term, so it
/// never needs to outlive a single pump.
struct AppHandler<'a> {
    shared: &'a Rc<RefCell<SharedState>>,
    descriptor: &'a WindowDescriptor,
}

impl ApplicationHandler for AppHandler<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let mut shared = self.shared.borrow_mut();
        if shared.os_window.is_some() {
            // Already created; `resumed` can legitimately fire more than
            // once on some platforms (see `ApplicationHandler::resumed`'s
            // own docs) -- nothing to do.
            return;
        }
        let attrs = WinitOsWindow::default_attributes()
            .with_title(self.descriptor.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.descriptor.width as f64,
                self.descriptor.height as f64,
            ));
        match event_loop.create_window(attrs) {
            Ok(window) => shared.os_window = Some(window),
            Err(err) => {
                // `Window::poll_events` has no error return -- matching
                // the existing trait shape, which `HeadlessWindow` also
                // never fails out of. A window that fails to create ends
                // up permanently `should_close()`-true instead of ever
                // reporting a window, which is a safer default than
                // panicking a caller that didn't ask for a `Result`.
                tracing::error!("failed to create winit window: {err}");
                shared.close_requested = true;
            }
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let mut shared = self.shared.borrow_mut();
        match event {
            WindowEvent::CloseRequested => shared.close_requested = true,
            WindowEvent::KeyboardInput { event, .. } => {
                // Key-repeat produces additional "pressed" events while a
                // key is held; `InputEvent::KeyPressed` models a
                // transition (up -> down), not "still down", so repeats
                // are deliberately dropped here -- matching winit's own
                // documented recommendation for exactly this case.
                if event.repeat {
                    return;
                }
                let key = physical_key_to_key(event.physical_key);
                let input_event = match event.state {
                    ElementState::Pressed => InputEvent::KeyPressed(key),
                    ElementState::Released => InputEvent::KeyReleased(key),
                };
                shared.pending_input.push(input_event);
            }
            _ => {}
        }
    }
}

/// Maps `winit`'s physical key representation onto this crate's
/// layout-independent [`Key`]. See [`Key`]'s own docs for why physical
/// position (not the character produced) is what's modeled.
fn physical_key_to_key(physical_key: PhysicalKey) -> Key {
    let code = match physical_key {
        PhysicalKey::Code(code) => code,
        // A raw platform-specific code winit couldn't map to a known
        // `KeyCode` at all. There's no single cross-platform numeric type
        // to extract here (X11/Windows/macOS each use a different native
        // representation) -- `u32::MAX` is a documented sentinel, not a
        // meaningful raw code. See `Key::Other`'s docs.
        PhysicalKey::Unidentified(_) => return Key::Other(u32::MAX),
    };
    match code {
        KeyCode::KeyA => Key::A,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyG => Key::G,
        KeyCode::KeyH => Key::H,
        KeyCode::KeyI => Key::I,
        KeyCode::KeyJ => Key::J,
        KeyCode::KeyK => Key::K,
        KeyCode::KeyL => Key::L,
        KeyCode::KeyM => Key::M,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyO => Key::O,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyT => Key::T,
        KeyCode::KeyU => Key::U,
        KeyCode::KeyV => Key::V,
        KeyCode::KeyW => Key::W,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyY => Key::Y,
        KeyCode::KeyZ => Key::Z,

        KeyCode::Digit0 => Key::Digit0,
        KeyCode::Digit1 => Key::Digit1,
        KeyCode::Digit2 => Key::Digit2,
        KeyCode::Digit3 => Key::Digit3,
        KeyCode::Digit4 => Key::Digit4,
        KeyCode::Digit5 => Key::Digit5,
        KeyCode::Digit6 => Key::Digit6,
        KeyCode::Digit7 => Key::Digit7,
        KeyCode::Digit8 => Key::Digit8,
        KeyCode::Digit9 => Key::Digit9,

        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,

        KeyCode::ShiftLeft => Key::ShiftLeft,
        KeyCode::ShiftRight => Key::ShiftRight,
        KeyCode::ControlLeft => Key::ControlLeft,
        KeyCode::ControlRight => Key::ControlRight,
        KeyCode::AltLeft => Key::AltLeft,
        KeyCode::AltRight => Key::AltRight,
        KeyCode::SuperLeft => Key::SuperLeft,
        KeyCode::SuperRight => Key::SuperRight,

        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Insert => Key::Insert,
        KeyCode::Delete => Key::Delete,

        KeyCode::Escape => Key::Escape,
        KeyCode::Space => Key::Space,
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::CapsLock => Key::CapsLock,

        KeyCode::Minus => Key::Minus,
        KeyCode::Equal => Key::Equal,
        KeyCode::BracketLeft => Key::BracketLeft,
        KeyCode::BracketRight => Key::BracketRight,
        KeyCode::Backslash => Key::Backslash,
        KeyCode::Semicolon => Key::Semicolon,
        KeyCode::Quote => Key::Quote,
        KeyCode::Comma => Key::Comma,
        KeyCode::Period => Key::Period,
        KeyCode::Slash => Key::Slash,
        KeyCode::Backquote => Key::Backquote,

        // Every other `KeyCode` (numpad, media keys, IME/language-specific
        // keys, F13+, ...) doesn't have a named `Key` variant yet -- see
        // `Key::Other`'s docs. `KeyCode` is `#[non_exhaustive]`, so this
        // arm also covers anything a future `winit` release adds.
        other => Key::Other(other as u32),
    }
}

/// A real `winit`-backed window. See the module docs for the pull/push
/// bridge this implements and why this shares state with [`WinitInput`].
pub struct WinitWindow {
    descriptor: WindowDescriptor,
    event_loop: EventLoop<()>,
    shared: Rc<RefCell<SharedState>>,
}

/// An error creating a real OS window or its underlying event loop.
#[derive(Debug, thiserror::Error)]
pub enum WinitWindowError {
    /// The platform event loop itself failed to initialize.
    #[error("failed to create winit event loop: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),
}

impl WinitWindow {
    /// Creates a real OS window matching `descriptor`.
    ///
    /// Window creation itself happens inside `winit`'s `resumed` callback,
    /// per its own platform-portability requirements (some platforms,
    /// notably Android, don't allow a render surface before that point) --
    /// so this pumps the event loop once immediately to reach that point,
    /// rather than deferring window creation to the first
    /// [`Window::poll_events`] call. By the time this returns `Ok`, a real
    /// window exists.
    ///
    /// Must be called from the main thread. This is a real, deliberate
    /// constraint, not just an X11 inconvenience: `winit` requires it
    /// unconditionally on some platforms (notably macOS, where AppKit
    /// itself requires it), so this constructor doesn't offer a way
    /// around it -- see [`WinitWindow::new_for_testing`] for the
    /// Linux-only, test-only exception, and why it has to be a separate,
    /// clearly-labeled entry point rather than a flag on this one.
    pub fn new(descriptor: WindowDescriptor) -> Result<Self, WinitWindowError> {
        Self::build(descriptor, EventLoop::new)
    }

    /// The same as [`WinitWindow::new`], except the underlying event loop
    /// is built with `winit`'s `any_thread` escape hatch enabled, which
    /// this project's own tests need since `cargo test` runs each test
    /// function on a worker thread, not the process's actual main thread
    /// -- confirmed necessary the direct way: [`WinitWindow::new`] panics
    /// immediately under `cargo test` without it (`winit` calls this "a
    /// significant cross-platform compatibility hazard" in its own panic
    /// message, which is accurate -- this is why it isn't the default).
    ///
    /// Linux-only and test-only. Not a general-purpose alternative
    /// constructor: `any_thread` is far less supported on macOS (AppKit
    /// itself requires the main thread) and is not something real,
    /// shipped game code should reach for. Exists so
    /// `tests/winit_backend_window.rs` (an external integration-test
    /// binary, which can't reach a `#[cfg(test)]`-gated item in this
    /// crate) has a real, public, but unmistakably test-only way to
    /// construct a window.
    #[cfg(target_os = "linux")]
    #[doc(hidden)]
    pub fn new_for_testing(descriptor: WindowDescriptor) -> Result<Self, WinitWindowError> {
        use winit::platform::x11::EventLoopBuilderExtX11;
        Self::build(descriptor, || {
            let mut builder = EventLoop::builder();
            builder.with_any_thread(true);
            builder.build()
        })
    }

    fn build(
        descriptor: WindowDescriptor,
        make_event_loop: impl FnOnce() -> Result<EventLoop<()>, winit::error::EventLoopError>,
    ) -> Result<Self, WinitWindowError> {
        let mut event_loop = make_event_loop()?;
        let shared = Rc::new(RefCell::new(SharedState {
            os_window: None,
            close_requested: false,
            pending_input: Vec::new(),
        }));

        let mut handler = AppHandler {
            shared: &shared,
            descriptor: &descriptor,
        };
        event_loop.pump_app_events(Some(Duration::ZERO), &mut handler);

        Ok(Self {
            descriptor,
            event_loop,
            shared,
        })
    }

    /// Creates an [`InputSource`] sharing this window's event pump. See
    /// the module docs for why this has to share state rather than being
    /// fully independent the way [`crate::HeadlessInput`] is.
    pub fn input_source(&self) -> WinitInput {
        WinitInput {
            shared: Rc::clone(&self.shared),
        }
    }
}

impl Window for WinitWindow {
    fn descriptor(&self) -> &WindowDescriptor {
        &self.descriptor
    }

    fn should_close(&self) -> bool {
        self.shared.borrow().close_requested
    }

    fn poll_events(&mut self) {
        let mut handler = AppHandler {
            shared: &self.shared,
            descriptor: &self.descriptor,
        };
        // Zero timeout: return immediately once the currently queued OS
        // events are drained, rather than blocking for new ones -- this
        // is a per-frame poll, not an event-driven main loop.
        self.event_loop
            .pump_app_events(Some(Duration::ZERO), &mut handler);
    }
}

/// A real `winit`-backed input source, sharing state with the
/// [`WinitWindow`] it was created from. See the module docs.
pub struct WinitInput {
    shared: Rc<RefCell<SharedState>>,
}

impl InputSource for WinitInput {
    fn poll(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.shared.borrow_mut().pending_input)
    }
}

#[cfg(test)]
mod tests {
    //! This module's own test only exercises `physical_key_to_key`
    //! directly -- everything requiring a live window (creation, redraw,
    //! resize, and close-request handling) lives in
    //! `tests/winit_backend_window.rs`, which needs a real (if virtual)
    //! display and is therefore `#[ignore]`d by default. See that file's
    //! own docs for why, and for the CI job that actually runs it.
    use super::*;

    #[test]
    fn maps_known_physical_keys_to_named_variants() {
        assert_eq!(
            physical_key_to_key(PhysicalKey::Code(KeyCode::KeyW)),
            Key::W
        );
        assert_eq!(
            physical_key_to_key(PhysicalKey::Code(KeyCode::Space)),
            Key::Space
        );
        assert_eq!(
            physical_key_to_key(PhysicalKey::Code(KeyCode::ArrowUp)),
            Key::ArrowUp
        );
    }

    #[test]
    fn maps_unnamed_physical_keys_to_other_not_a_panic() {
        // NumpadStar has no named `Key` variant yet.
        match physical_key_to_key(PhysicalKey::Code(KeyCode::NumpadStar)) {
            Key::Other(_) => {}
            other => panic!("expected Key::Other, got {other:?}"),
        }
    }

    #[test]
    fn maps_unidentified_physical_keys_to_the_documented_sentinel() {
        assert_eq!(
            physical_key_to_key(PhysicalKey::Unidentified(
                winit::keyboard::NativeKeyCode::Unidentified
            )),
            Key::Other(u32::MAX)
        );
    }
}
