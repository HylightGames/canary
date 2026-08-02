/// A normalized input event, independent of any specific OS input API.
///
/// v0.0.1-pre1 note: this is an intentionally small, illustrative set —
/// expanded when a real input backend lands. See
/// `docs/architecture/platform-abstraction.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// A key transitioned from up to down.
    KeyPressed(Key),
    /// A key transitioned from down to up.
    KeyReleased(Key),
}

/// A platform-independent key identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// The Escape key.
    Escape,
    /// The Space bar.
    Space,
    /// Any key not yet given its own variant, identified by a
    /// backend-specific raw code. Exists so a real backend can be added
    /// later without every intermediate key needing its own variant first.
    Other(u32),
}

/// A source of normalized input events.
///
/// v0.0.1-pre1 ships only [`crate::HeadlessInput`], which never produces
/// real OS input on its own but can have events injected for tests and
/// headless harnesses via [`crate::HeadlessInput::inject`].
pub trait InputSource {
    /// Returns (and clears) all events observed since the last call.
    fn poll(&mut self) -> Vec<InputEvent>;
}
