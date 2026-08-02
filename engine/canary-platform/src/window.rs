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
/// v0.0.1-pre1 ships only [`crate::HeadlessWindow`], which satisfies this
/// trait without ever creating a real OS window. This matters for headless
/// servers and for tests, not just as a stopgap — see
/// `docs/architecture/platform-abstraction.md#why-this-is-a-real-trait-boundary-and-not-just-we-use-winit`.
pub trait Window {
    /// The descriptor this window was created with.
    fn descriptor(&self) -> &WindowDescriptor;

    /// Whether the window (real or virtual) has been asked to close.
    fn should_close(&self) -> bool;

    /// Poll for platform events. A real backend pumps the OS event loop
    /// here; [`crate::HeadlessWindow`]'s implementation is a no-op.
    fn poll_events(&mut self);
}
