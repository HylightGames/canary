//! Pure translation of platform input identities to `egui` identities.
//!
//! Positions and sizes stay in logical pixels on both sides (the platform
//! slice promises logical pixels; `egui` points map 1:1 at
//! `pixels_per_point = 1.0`). Events the first-game slice cannot use
//! (unmapped keys, buttons past the fifth) translate to `None` and are
//! dropped by the caller — never synthesized, never guessed.

use canary_platform::{Key, PointerButton};

/// Maps a platform key to the `egui` key, or `None` when the slice has no
/// mapping (platform-private keys, future keys).
///
/// The platform key model is physical position; it feeds `egui`'s logical
/// `key` field (not `physical_key`), because UI widgets consume layout-
/// dependent meaning (typing, shortcuts), the case the `egui` docs reserve
/// `key` for.
pub(crate) fn map_key(key: &Key) -> Option<egui::Key> {
    let mapped = match key {
        Key::A => egui::Key::A,
        Key::B => egui::Key::B,
        Key::C => egui::Key::C,
        Key::D => egui::Key::D,
        Key::E => egui::Key::E,
        Key::F => egui::Key::F,
        Key::G => egui::Key::G,
        Key::H => egui::Key::H,
        Key::I => egui::Key::I,
        Key::J => egui::Key::J,
        Key::K => egui::Key::K,
        Key::L => egui::Key::L,
        Key::M => egui::Key::M,
        Key::N => egui::Key::N,
        Key::O => egui::Key::O,
        Key::P => egui::Key::P,
        Key::Q => egui::Key::Q,
        Key::R => egui::Key::R,
        Key::S => egui::Key::S,
        Key::T => egui::Key::T,
        Key::U => egui::Key::U,
        Key::V => egui::Key::V,
        Key::W => egui::Key::W,
        Key::X => egui::Key::X,
        Key::Y => egui::Key::Y,
        Key::Z => egui::Key::Z,
        Key::Digit0 => egui::Key::Num0,
        Key::Digit1 => egui::Key::Num1,
        Key::Digit2 => egui::Key::Num2,
        Key::Digit3 => egui::Key::Num3,
        Key::Digit4 => egui::Key::Num4,
        Key::Digit5 => egui::Key::Num5,
        Key::Digit6 => egui::Key::Num6,
        Key::Digit7 => egui::Key::Num7,
        Key::Digit8 => egui::Key::Num8,
        Key::Digit9 => egui::Key::Num9,
        Key::F1 => egui::Key::F1,
        Key::F2 => egui::Key::F2,
        Key::F3 => egui::Key::F3,
        Key::F4 => egui::Key::F4,
        Key::F5 => egui::Key::F5,
        Key::F6 => egui::Key::F6,
        Key::F7 => egui::Key::F7,
        Key::F8 => egui::Key::F8,
        Key::F9 => egui::Key::F9,
        Key::F10 => egui::Key::F10,
        Key::F11 => egui::Key::F11,
        Key::F12 => egui::Key::F12,
        Key::ShiftLeft => egui::Key::ShiftLeft,
        Key::ShiftRight => egui::Key::ShiftRight,
        Key::ControlLeft => egui::Key::ControlLeft,
        Key::ControlRight => egui::Key::ControlRight,
        Key::AltLeft => egui::Key::AltLeft,
        Key::AltRight => egui::Key::AltRight,
        Key::SuperLeft => egui::Key::SuperLeft,
        Key::SuperRight => egui::Key::SuperRight,
        Key::ArrowUp => egui::Key::ArrowUp,
        Key::ArrowDown => egui::Key::ArrowDown,
        Key::ArrowLeft => egui::Key::ArrowLeft,
        Key::ArrowRight => egui::Key::ArrowRight,
        Key::Home => egui::Key::Home,
        Key::End => egui::Key::End,
        Key::PageUp => egui::Key::PageUp,
        Key::PageDown => egui::Key::PageDown,
        Key::Insert => egui::Key::Insert,
        Key::Delete => egui::Key::Delete,
        Key::Escape => egui::Key::Escape,
        Key::Space => egui::Key::Space,
        Key::Enter => egui::Key::Enter,
        Key::Tab => egui::Key::Tab,
        Key::Backspace => egui::Key::Backspace,
        Key::Minus => egui::Key::Minus,
        Key::Equal => egui::Key::Equals,
        Key::BracketLeft => egui::Key::OpenBracket,
        Key::BracketRight => egui::Key::CloseBracket,
        Key::Backslash => egui::Key::Backslash,
        Key::Semicolon => egui::Key::Semicolon,
        Key::Quote => egui::Key::Quote,
        Key::Comma => egui::Key::Comma,
        Key::Period => egui::Key::Period,
        Key::Slash => egui::Key::Slash,
        Key::Backquote => egui::Key::Backtick,
        // No first-game meaning: held-state toggles, platform-private
        // keys, and codes the platform has not assigned yet. The wildcard
        // is required: `Key` is `#[non_exhaustive]`, so future keys also
        // land here (unmapped) instead of breaking the build.
        Key::CapsLock | Key::Other(_) | _ => return None,
    };
    Some(mapped)
}

/// Maps a platform pointer button, or `None` past the fifth button
/// (`egui` models five: primary/secondary/middle plus two extras).
pub(crate) fn map_button(button: &PointerButton) -> Option<egui::PointerButton> {
    let mapped = match button {
        PointerButton::Primary => egui::PointerButton::Primary,
        PointerButton::Secondary => egui::PointerButton::Secondary,
        PointerButton::Middle => egui::PointerButton::Middle,
        // Platform numbering continues the sequence after the three named
        // buttons, so `Other(3)`/`Other(4)` are the fourth/fifth buttons.
        PointerButton::Other(3) => egui::PointerButton::Extra1,
        PointerButton::Other(4) => egui::PointerButton::Extra2,
        // The wildcard is required: `PointerButton` is `#[non_exhaustive]`.
        PointerButton::Other(_) | _ => return None,
    };
    Some(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_map_to_same_egui_key() {
        assert_eq!(map_key(&Key::A), Some(egui::Key::A));
        assert_eq!(map_key(&Key::Z), Some(egui::Key::Z));
    }

    #[test]
    fn digits_map_to_egui_number_keys() {
        assert_eq!(map_key(&Key::Digit0), Some(egui::Key::Num0));
        assert_eq!(map_key(&Key::Digit9), Some(egui::Key::Num9));
    }

    #[test]
    fn controls_and_navigation_map() {
        assert_eq!(map_key(&Key::Escape), Some(egui::Key::Escape));
        assert_eq!(map_key(&Key::Enter), Some(egui::Key::Enter));
        assert_eq!(map_key(&Key::Space), Some(egui::Key::Space));
        assert_eq!(map_key(&Key::ArrowUp), Some(egui::Key::ArrowUp));
        assert_eq!(map_key(&Key::F1), Some(egui::Key::F1));
        assert_eq!(map_key(&Key::F12), Some(egui::Key::F12));
    }

    #[test]
    fn punctuation_maps_by_meaning_not_name() {
        assert_eq!(map_key(&Key::BracketLeft), Some(egui::Key::OpenBracket));
        assert_eq!(map_key(&Key::Minus), Some(egui::Key::Minus));
        assert_eq!(map_key(&Key::Slash), Some(egui::Key::Slash));
    }

    #[test]
    fn unmapped_keys_translate_to_none() {
        assert_eq!(map_key(&Key::CapsLock), None);
        assert_eq!(map_key(&Key::Other(0xFFFF_FFFF)), None);
    }

    #[test]
    fn five_pointer_buttons_map() {
        assert_eq!(
            map_button(&PointerButton::Primary),
            Some(egui::PointerButton::Primary)
        );
        assert_eq!(
            map_button(&PointerButton::Secondary),
            Some(egui::PointerButton::Secondary)
        );
        assert_eq!(
            map_button(&PointerButton::Middle),
            Some(egui::PointerButton::Middle)
        );
        assert_eq!(
            map_button(&PointerButton::Other(3)),
            Some(egui::PointerButton::Extra1)
        );
        assert_eq!(
            map_button(&PointerButton::Other(4)),
            Some(egui::PointerButton::Extra2)
        );
    }

    #[test]
    fn buttons_past_the_fifth_translate_to_none() {
        assert_eq!(map_button(&PointerButton::Other(5)), None);
        assert_eq!(map_button(&PointerButton::Other(u8::MAX)), None);
    }
}
