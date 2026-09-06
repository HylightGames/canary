// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

/// A normalized input event, independent of any specific OS input API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// A key transitioned from up to down.
    KeyPressed(Key),
    /// A key transitioned from down to up.
    KeyReleased(Key),
}

/// A platform-independent, physical (layout-independent) key identifier —
/// "the key in this location on the keyboard," not "the character this key
/// produces." Modeled on the W3C `KeyboardEvent.code` value table (the same
/// model `winit::keyboard::KeyCode` uses), since physical position is what
/// gameplay code almost always actually wants (WASD movement should stay
/// under the left hand on an AZERTY keyboard, not silently become ZQSD).
///
/// Covers a realistic keyboard's main alphanumeric, modifier, navigation,
/// and punctuation keys — not exhaustive (numpad, media keys, and IME/
/// language-specific keys aren't named yet; see [`Key::Other`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    // -- Letters --
    /// The `A` key.
    A,
    /// The `B` key.
    B,
    /// The `C` key.
    C,
    /// The `D` key.
    D,
    /// The `E` key.
    E,
    /// The `F` key.
    F,
    /// The `G` key.
    G,
    /// The `H` key.
    H,
    /// The `I` key.
    I,
    /// The `J` key.
    J,
    /// The `K` key.
    K,
    /// The `L` key.
    L,
    /// The `M` key.
    M,
    /// The `N` key.
    N,
    /// The `O` key.
    O,
    /// The `P` key.
    P,
    /// The `Q` key.
    Q,
    /// The `R` key.
    R,
    /// The `S` key.
    S,
    /// The `T` key.
    T,
    /// The `U` key.
    U,
    /// The `V` key.
    V,
    /// The `W` key.
    W,
    /// The `X` key.
    X,
    /// The `Y` key.
    Y,
    /// The `Z` key.
    Z,

    // -- Digits (top row, not numpad) --
    /// The `0` key (top row, not numpad).
    Digit0,
    /// The `1` key (top row, not numpad).
    Digit1,
    /// The `2` key (top row, not numpad).
    Digit2,
    /// The `3` key (top row, not numpad).
    Digit3,
    /// The `4` key (top row, not numpad).
    Digit4,
    /// The `5` key (top row, not numpad).
    Digit5,
    /// The `6` key (top row, not numpad).
    Digit6,
    /// The `7` key (top row, not numpad).
    Digit7,
    /// The `8` key (top row, not numpad).
    Digit8,
    /// The `9` key (top row, not numpad).
    Digit9,

    // -- Function keys --
    /// The `F1` key.
    F1,
    /// The `F2` key.
    F2,
    /// The `F3` key.
    F3,
    /// The `F4` key.
    F4,
    /// The `F5` key.
    F5,
    /// The `F6` key.
    F6,
    /// The `F7` key.
    F7,
    /// The `F8` key.
    F8,
    /// The `F9` key.
    F9,
    /// The `F10` key.
    F10,
    /// The `F11` key.
    F11,
    /// The `F12` key.
    F12,

    // -- Modifiers --
    /// The left Shift key.
    ShiftLeft,
    /// The right Shift key.
    ShiftRight,
    /// The left Control key.
    ControlLeft,
    /// The right Control key.
    ControlRight,
    /// The left Alt (or Option, on macOS) key.
    AltLeft,
    /// The right Alt (or Option, on macOS) key.
    AltRight,
    /// The left Windows/Command/Super key.
    SuperLeft,
    /// The right Windows/Command/Super key.
    SuperRight,

    // -- Navigation --
    /// The Up arrow key.
    ArrowUp,
    /// The Down arrow key.
    ArrowDown,
    /// The Left arrow key.
    ArrowLeft,
    /// The Right arrow key.
    ArrowRight,
    /// The Home key.
    Home,
    /// The End key.
    End,
    /// The Page Up key.
    PageUp,
    /// The Page Down key.
    PageDown,
    /// The Insert key.
    Insert,
    /// The (forward) Delete key.
    Delete,

    // -- Editing / whitespace --
    /// The Escape key.
    Escape,
    /// The Space bar.
    Space,
    /// The Enter/Return key.
    Enter,
    /// The Tab key.
    Tab,
    /// The Backspace key.
    Backspace,
    /// The Caps Lock key.
    CapsLock,

    // -- Punctuation (US layout positions; see the doc comment above for
    // why this is physical position, not the character produced) --
    /// `-` on a US keyboard.
    Minus,
    /// `=` on a US keyboard.
    Equal,
    /// `[` on a US keyboard.
    BracketLeft,
    /// `]` on a US keyboard.
    BracketRight,
    /// `\` on a US keyboard.
    Backslash,
    /// `;` on a US keyboard.
    Semicolon,
    /// `'` on a US keyboard.
    Quote,
    /// `,` on a US keyboard.
    Comma,
    /// `.` on a US keyboard.
    Period,
    /// `/` on a US keyboard.
    Slash,
    /// `` ` `` on a US keyboard.
    Backquote,

    /// Any key not yet given its own variant. Carries a raw code that is
    /// stable within a single build of this crate (not a cross-platform OS
    /// scancode) — enough to distinguish keys from each other (e.g. for a
    /// rebindable-input system storing raw codes), but not enough to be
    /// meaningful without going through the backend that produced it.
    /// Exists so a real backend can be added later, and so a backend can
    /// gain new physical keys (numpad, media keys, ...) later, without
    /// every intermediate key needing its own named variant first.
    Other(u32),
}

/// A source of normalized input events.
///
/// v0.0.1-pre1 ships only [`crate::HeadlessInput`], which never produces
/// real OS input on its own but can have events injected for tests and
/// headless harnesses via [`crate::HeadlessInput::inject`]. A real backend
/// (`winit`-backed, behind the `winit-backend` feature) shipped in `v0.0.4`
/// — see `winit_backend`, only present when that feature is enabled.
pub trait InputSource {
    /// Returns (and clears) all events observed since the last call.
    fn poll(&mut self) -> Vec<InputEvent>;
}
