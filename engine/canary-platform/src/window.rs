// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

/// Describes the window a [`Window`] implementation should create.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowDescriptor {
    /// The window's title bar text (ignored by headless implementations).
    pub title: String,
    /// Requested width in logical pixels.
    pub width: u32,
    /// Requested height in logical pixels.
    pub height: u32,
}

impl Default for WindowDescriptor {
    fn default() -> Self {
        Self {
            title: String::from("Canary Engine"),
            width: 1280,
            height: 720,
        }
    }
}

/// A platform window, real or virtual.
///
/// The first implementation was [`crate::HeadlessWindow`], which satisfies
/// this trait without ever creating a real OS window; since `v0.0.4` a
/// real `winit`-backed implementation also exists (see `winit_backend`,
/// behind the `winit-backend` feature). The headless form still matters
/// for headless servers and for tests, not just as a stopgap — see
/// `docs/architecture/platform-abstraction.md#why-this-is-a-real-trait-boundary-and-not-just-we-use-winit`.
pub trait Window {
    /// The descriptor this window was created with.
    fn descriptor(&self) -> &WindowDescriptor;

    /// Whether the window (real or virtual) has been asked to close.
    fn should_close(&self) -> bool;

    /// Poll for platform events. A real backend pumps the OS event loop
    /// here; [`crate::HeadlessWindow`]'s implementation is a no-op.
    fn poll_events(&mut self);

    /// Whether this window can back a presentation swapchain.
    ///
    /// Additive (default `false`) so existing implementors — including
    /// [`crate::HeadlessWindow`] — compile unchanged: only a real,
    /// surface-capable backend overrides this. Presenters treat `false`
    /// as "never ask for handles", not as an error.
    fn supports_presentation(&self) -> bool {
        false
    }

    /// The current surface extent in physical pixels, or `None` when
    /// this window has none to report. A `Some` with a zero axis means
    /// minimized: presenters suspend rather than error.
    ///
    /// Additive (default `None`): headless windows report nothing.
    fn surface_extent(&self) -> Option<(u32, u32)> {
        None
    }

    /// Counts resizes this window has observed since creation.
    /// Presenters compare it frame-to-frame as a recreate signal,
    /// alongside the extent itself: a change means "recreate the
    /// swapchain", even when the extent already matches (e.g. a resize
    /// back to the same size still invalidates the surface on some
    /// drivers).
    ///
    /// Additive (default `0`): windows that never resize stay silent.
    fn resize_generation(&self) -> u64 {
        0
    }
}
