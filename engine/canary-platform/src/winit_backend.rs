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
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::{Window as WinitOsWindow, WindowId};

use crate::input::{InputEvent, InputSource, Key, PointerButton};
use crate::surface::{DisplayHandle, SurfaceHandlesProvider, WindowHandle, WindowHandles};
use crate::window::{Window, WindowDescriptor};

/// Everything a live `winit` window needs, shared between [`WinitWindow`]
/// (which pumps the event loop) and any [`WinitInput`] created from it
/// (which only drains what the pump observed).
struct SharedState {
    os_window: Option<WinitOsWindow>,
    close_requested: bool,
    pending_input: Vec<InputEvent>,
    /// How many resizes this window has observed — the presenter's
    /// recreate signal (see [`SurfaceHandlesProvider::resize_generation`]).
    resize_generation: u64,
    /// Whether the window currently holds OS input focus. Starts `true`:
    /// a just-created window opens in the foreground on every desktop
    /// platform this backend targets, and `winit` corrects it on the
    /// first focus event either way — while no events can arrive at an
    /// unfocused window, so a wrong-`true` first frame routes nothing.
    focused: bool,
}

impl SharedState {
    /// The window's current scale factor, or 1.0 if the OS window doesn't
    /// exist yet (no window event can arrive before creation, so this is
    /// unreachable in practice — but physical pixels equal logical pixels
    /// at 1.0, making it the only safe fallback).
    fn scale_factor(&self) -> f64 {
        self.os_window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor())
    }
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
            WindowEvent::Resized(_) => {
                shared.resize_generation = shared.resize_generation.saturating_add(1);
            }
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
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = logical_position(position, shared.scale_factor());
                shared.pending_input.push(InputEvent::PointerMoved { x, y });
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = pointer_button_from_winit(button);
                let input_event = match state {
                    ElementState::Pressed => InputEvent::PointerPressed(button),
                    ElementState::Released => InputEvent::PointerReleased(button),
                };
                shared.pending_input.push(input_event);
            }
            WindowEvent::CursorLeft { .. } => {
                shared.pending_input.push(InputEvent::PointerLeft);
            }
            WindowEvent::Focused(focused) => {
                // Focus gained needs no event: the mapper holds nothing
                // to release for a window that just became active. Focus
                // lost pushes `FocusLost` so the mapper can clear held
                // controls — and either way the flag tracks the window's
                // live focus state for the frame driver's UI input.
                shared.focused = focused;
                if !focused {
                    shared.pending_input.push(InputEvent::FocusLost);
                }
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

/// Maps a `winit` mouse button onto this crate's [`PointerButton`].
/// `winit` names three buttons and numbers the rest from 0; this crate
/// names the same three and continues the numbering after them, so the
/// first unnamed winit button is [`PointerButton::Other`]`(3)`.
fn pointer_button_from_winit(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
        // Numbered from 0 in winit, continuing after the three named
        // buttons here. Saturates instead of wrapping: a backend with
        // more than 259 buttons is a broken backend, and an index that
        // collides is diagnosable while one that wraps silently is not.
        MouseButton::Back => PointerButton::Other(3),
        MouseButton::Forward => PointerButton::Other(4),
        MouseButton::Other(index) => PointerButton::Other(u8::try_from(index).unwrap_or(u8::MAX)),
    }
}

/// Converts a `winit` physical cursor position into logical pixels using
/// the window's scale factor. No numeric cast on our side: `winit`'s own
/// [`winit::dpi::PhysicalPosition::to_logical`] performs the physical ->
/// logical division and yields `f32` components directly.
fn logical_position(position: winit::dpi::PhysicalPosition<f64>, scale_factor: f64) -> (f32, f32) {
    let logical = position.to_logical::<f32>(scale_factor);
    (logical.x, logical.y)
}

/// A real `winit`-backed window. See the module docs for the pull/push
/// bridge this implements and why this shares state with [`WinitInput`].
/// The shared state is `Rc<RefCell<..>>`, so this type is `!Send`:
/// `poll_events` (here) and `poll` (on the paired [`WinitInput`]) must
/// run on the same thread, in that order.
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
    ///
    /// Known limitation: if the OS window itself fails to create inside
    /// `resumed`, that failure is recorded as `close_requested` (and
    /// logged) rather than returned — `Window::poll_events` has no error
    /// return, so a caller that never checks `should_close` cannot tell
    /// "no window" from "idle window". Check `should_close` after
    /// construction when creation success matters.
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
            resize_generation: 0,
            focused: true,
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

    /// The window's current scale factor: physical pixels per logical
    /// pixel. The frame driver divides the physical `surface_extent` by
    /// this to size the UI viewport in logical pixels.
    pub fn scale_factor(&self) -> f64 {
        self.shared.borrow().scale_factor()
    }

    /// Whether the window currently holds OS input focus, as of the last
    /// [`Window::poll_events`] pump. The frame driver passes this through
    /// to the UI frame; the mapper learns of focus loss separately via
    /// the [`InputEvent::FocusLost`] event the pump pushes.
    pub fn is_focused(&self) -> bool {
        self.shared.borrow().focused
    }

    /// Minimizes (`true`) or restores (`false`) the window.
    ///
    /// Linux-only and test-only, like [`WinitWindow::new_for_testing`]:
    /// the live presentation proof needs to drive the suspend path
    /// through a real window manager. Asynchronous like the resize
    /// hatch above — poll events and re-read [`Window::surface_extent`]
    /// (a minimized window typically reports a zero extent, which is
    /// what suspends presentation).
    #[cfg(target_os = "linux")]
    #[doc(hidden)]
    pub fn set_minimized_for_testing(&self, minimized: bool) {
        if let Some(os_window) = self.shared.borrow().os_window.as_ref() {
            os_window.set_minimized(minimized);
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

    fn supports_presentation(&self) -> bool {
        self.shared.borrow().os_window.is_some()
    }

    fn surface_extent(&self) -> Option<(u32, u32)> {
        let shared = self.shared.borrow();
        let os_window = shared.os_window.as_ref()?;
        let size = os_window.inner_size();
        Some((size.width, size.height))
    }

    fn resize_generation(&self) -> u64 {
        self.shared.borrow().resize_generation
    }
}

impl SurfaceHandlesProvider for WinitWindow {
    fn window_handles(&self) -> Option<WindowHandles> {
        let shared = self.shared.borrow();
        let os_window = shared.os_window.as_ref()?;
        extract_window_handles(os_window)
    }
}

/// Converts a live `winit` window into Canary-owned handles.
///
/// The only `raw-window-handle` contact in this crate, via `winit`'s
/// own `rwh_06` re-export (no direct dependency, no `rwh_04`/`rwh_05`
/// features anywhere): `winit` 0.30 implements the 0.6 traits, so the
/// version always matches by construction. Returns `None` for
/// platforms this seam does not cover yet (anything outside the
/// desktop Xlib/Xcb/Wayland/Win32/AppKit set).
fn extract_window_handles(os_window: &WinitOsWindow) -> Option<WindowHandles> {
    use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

    let display = os_window.display_handle().ok()?;
    let window = os_window.window_handle().ok()?;
    let display = match display.as_ref() {
        winit::raw_window_handle::RawDisplayHandle::Xlib(handle) => DisplayHandle::Xlib {
            display: handle.display,
            screen: handle.screen,
        },
        winit::raw_window_handle::RawDisplayHandle::Xcb(handle) => DisplayHandle::Xcb {
            connection: handle.connection,
            screen: handle.screen,
        },
        winit::raw_window_handle::RawDisplayHandle::Wayland(handle) => DisplayHandle::Wayland {
            display: handle.display,
        },
        winit::raw_window_handle::RawDisplayHandle::Windows(_) => DisplayHandle::Windows,
        winit::raw_window_handle::RawDisplayHandle::AppKit(_) => DisplayHandle::AppKit,
        _ => return None,
    };
    let window = match window.as_ref() {
        winit::raw_window_handle::RawWindowHandle::Xlib(handle) => WindowHandle::Xlib {
            window: handle.window,
            visual_id: handle.visual_id,
        },
        winit::raw_window_handle::RawWindowHandle::Xcb(handle) => WindowHandle::Xcb {
            window: handle.window,
            visual_id: handle.visual_id,
        },
        winit::raw_window_handle::RawWindowHandle::Wayland(handle) => WindowHandle::Wayland {
            surface: handle.surface,
        },
        winit::raw_window_handle::RawWindowHandle::Win32(handle) => WindowHandle::Win32 {
            hwnd: handle.hwnd,
            hinstance: handle.hinstance,
        },
        winit::raw_window_handle::RawWindowHandle::AppKit(handle) => WindowHandle::AppKit {
            view: handle.ns_view,
        },
        _ => return None,
    };
    Some(WindowHandles::new(display, window))
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

    #[test]
    fn maps_winit_pointer_buttons_to_named_variants() {
        use winit::event::MouseButton;
        assert_eq!(
            pointer_button_from_winit(MouseButton::Left),
            crate::input::PointerButton::Primary
        );
        assert_eq!(
            pointer_button_from_winit(MouseButton::Right),
            crate::input::PointerButton::Secondary
        );
        assert_eq!(
            pointer_button_from_winit(MouseButton::Middle),
            crate::input::PointerButton::Middle
        );
        assert_eq!(
            pointer_button_from_winit(MouseButton::Back),
            crate::input::PointerButton::Other(3)
        );
        assert_eq!(
            pointer_button_from_winit(MouseButton::Forward),
            crate::input::PointerButton::Other(4)
        );
    }

    #[test]
    fn converts_physical_cursor_position_to_logical_pixels() {
        // Given: a physical position on a 2x display.
        let physical = winit::dpi::PhysicalPosition::new(200.0, 100.0);
        // When: normalized with that display's scale factor.
        // Then: logical pixels, matching the `PointerMoved` contract.
        assert_eq!(logical_position(physical, 2.0), (100.0, 50.0));
        assert_eq!(logical_position(physical, 1.0), (200.0, 100.0));
    }
}
