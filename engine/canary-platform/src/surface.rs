// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The window side of the `Window → surface → swapchain → present` seam:
//! Canary-owned window handles plus the sealed capability trait a
//! presentation backend consumes.
//!
//! The interop version is `raw-window-handle` 0.6, matching what
//! `winit` 0.30 enables by default (`rwh_06` is in its default
//! features; `rwh_04`/`rwh_05` are never enabled anywhere in this
//! workspace). The handle enums below mirror that version's desktop
//! variants field-for-field, using only `std` types (`NonNull`,
//! `c_ulong`, ...) — no third-party type appears in any public
//! signature here, so a future handle-version migration touches the
//! two private conversion sites (here and the render backend's surface
//! module), never this boundary.
//!
//! A [`WindowHandles`] value is only meaningful while its window is
//! alive: the pointers inside alias the OS window/display objects, and
//! using them after the window dies is undefined. Presenters uphold
//! this by borrowing the provider (see
//! [`SurfaceHandlesProvider`]) for the surface's whole lifetime.

use std::ffi::c_void;
use std::os::raw::{c_int, c_ulong};
use std::ptr::NonNull;

use crate::window::Window;

/// Which native display connection a [`DisplayHandle`] carries.
///
/// Mirrors the `raw-window-handle` 0.6 desktop display variants this
/// seam covers. Anything else the OS reports (Android, Haiku, ...) is
/// not representable here on purpose: surface creation for those
/// platforms is unimplemented scope, reported as "no handles" rather
/// than smuggled through a lossy encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayHandle {
    /// An Xlib `Display` connection plus its screen number.
    Xlib {
        /// The Xlib `Display` pointer (`None` requests the default
        /// display, mirroring the interop type).
        display: Option<NonNull<c_void>>,
        /// The X11 screen number.
        screen: c_int,
    },
    /// An XCB server connection plus its screen number.
    Xcb {
        /// The `xcb_connection_t` pointer (`None` requests the default
        /// display, mirroring the interop type).
        connection: Option<NonNull<c_void>>,
        /// The X11 screen number.
        screen: c_int,
    },
    /// A Wayland display connection.
    Wayland {
        /// The `wl_display` pointer.
        display: NonNull<c_void>,
    },
    /// A Win32 display (unit: the handle carries no data, mirroring
    /// the interop type).
    Windows,
    /// An AppKit display (unit, mirroring the interop type).
    AppKit,
}

/// Which native window object a [`WindowHandle`] carries.
///
/// Mirrors the `raw-window-handle` 0.6 desktop window variants this
/// seam covers — same "unrepresentable means unsupported" rule as
/// [`DisplayHandle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowHandle {
    /// An Xlib window plus its visual ID (0 when unknown, mirroring
    /// the interop type).
    Xlib {
        /// The Xlib `Window`.
        window: c_ulong,
        /// The Xlib visual ID, or 0 when unknown.
        visual_id: c_ulong,
    },
    /// An XCB window plus its visual ID when known.
    Xcb {
        /// The `xcb_window_t`.
        window: std::num::NonZeroU32,
        /// The `xcb_visualid_t`, when known.
        visual_id: Option<std::num::NonZeroU32>,
    },
    /// A Wayland surface.
    Wayland {
        /// The `wl_surface` pointer.
        surface: NonNull<c_void>,
    },
    /// A Win32 window.
    Win32 {
        /// The `HWND`.
        hwnd: std::num::NonZeroIsize,
        /// The `HINSTANCE`, when known.
        hinstance: Option<std::num::NonZeroIsize>,
    },
    /// An AppKit view.
    AppKit {
        /// The `NSView` pointer.
        view: NonNull<c_void>,
    },
}

/// The native display/window pair a surface is built from, in
/// Canary-owned types.
///
/// Constructed only by platform backends in this crate (today the
/// `winit` backend) and by tests: the [`WindowHandles::new`]
/// constructor is public so cross-crate (backend) tests can build
/// synthetic carriers, but every constructor call site outside this
/// crate must be a test — production handles always originate from a
/// live OS window, because only real pointers build working surfaces
/// (fabricated ones fail loudly at surface creation, never silently).
///
/// Display and window kinds must agree (Xlib with Xlib, Wayland with
/// Wayland, ...): a mismatched pair is rejected at surface-creation
/// time rather than trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowHandles {
    /// The native display connection.
    pub display: DisplayHandle,
    /// The native window object on that connection.
    pub window: WindowHandle,
}

impl WindowHandles {
    /// Pairs a display connection with its window object.
    pub fn new(display: DisplayHandle, window: WindowHandle) -> Self {
        Self { display, window }
    }

    /// The native display connection.
    pub const fn display(&self) -> DisplayHandle {
        self.display
    }

    /// The native window object on that connection.
    pub const fn window(&self) -> WindowHandle {
        self.window
    }
}

/// What a window offers a presentation backend beyond extent and
/// resize signals: native handles for surface creation.
///
/// Only platform backends in this crate implement it meaningfully
/// (today just the `winit` backend's window) — a fabricated
/// implementation has no way to produce a working surface (see
/// [`WindowHandles`]) — and notably [`crate::HeadlessWindow`] does
/// *not* implement this trait, so passing a headless window where a
/// surface-capable one is required fails to compile rather than
/// failing at runtime.
///
/// Extends [`Window`] so presenters reach the
/// extent and resize generation through the same borrow: one `&dyn
/// SurfaceHandlesProvider` carries handles, extent, and signal
/// together.
///
/// (A private-supertrait seal would state the implementor rule more
/// loudly, but `private_bounds` — deny-by-default under this
/// workspace's warnings gate — rejects that pattern without an
/// `#[allow]`, which this project bans; the trait docs above state the
/// rule instead.)
///
/// The handle method returns `Option` because a live window can
/// briefly have nothing to offer: before the OS window exists, or on a
/// platform this seam does not cover.
pub trait SurfaceHandlesProvider: Window {
    /// The native handles to build a surface from, or `None` when
    /// this window currently has none to offer.
    fn window_handles(&self) -> Option<WindowHandles>;
}

#[cfg(test)]
mod tests {
    use crate::{HeadlessWindow, Window, WindowDescriptor};

    #[test]
    fn headless_windows_offer_no_presentation_surface() {
        let mut window = HeadlessWindow::new(WindowDescriptor::default());
        assert!(!window.supports_presentation());
        assert_eq!(window.surface_extent(), None);
        assert_eq!(window.resize_generation(), 0);
        window.poll_events();
        assert_eq!(window.resize_generation(), 0);
    }
}
