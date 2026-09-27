// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The UI-routed event stream: normalized raw events annotated with
//! order-preserving per-event capture flags (ADR 0025, decision 3).
//!
//! The UI adapter gets first refusal on the ordered raw event stream and
//! reports which keyboard/pointer events its current focus and interaction
//! state captured. The mapper receives the *full* ordered stream with those
//! flags attached — never a pre-filtered stream — because capture can only
//! be judged against bindings at mapping time: a captured event is dropped
//! unless a declared pass-through binding covers it. Capture is per-event
//! behavior, not a global flag, and never an `egui`-specific type.

use crate::RawInputEvent;

/// One normalized raw event plus the UI adapter's capture verdict for it.
///
/// `captured == true` means the UI consumed the event for the current UI
/// state. The mapper drops such events — unless a game-declared
/// pass-through binding covers the event's physical control (see
/// [`crate::Binding::pass_through`]), evaluated per binding inside the
/// mapper. Order of the stream is always preserved; flags never reorder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutedEvent {
    /// The normalized raw event.
    pub event: RawInputEvent,
    /// Whether the UI captured this event.
    pub captured: bool,
}

impl RoutedEvent {
    /// An event the UI did not capture: fully visible to gameplay mapping.
    pub fn open(event: RawInputEvent) -> Self {
        Self {
            event,
            captured: false,
        }
    }

    /// An event the UI captured: dropped by the mapper unless a
    /// pass-through binding covers it.
    pub fn captured(event: RawInputEvent) -> Self {
        Self {
            event,
            captured: true,
        }
    }
}

/// Annotates every event as UI-open, preserving order.
pub fn all_open(events: Vec<RawInputEvent>) -> Vec<RoutedEvent> {
    events.into_iter().map(RoutedEvent::open).collect()
}

/// Annotates every event as UI-captured, preserving order.
pub fn all_captured(events: Vec<RawInputEvent>) -> Vec<RoutedEvent> {
    events.into_iter().map(RoutedEvent::captured).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyCode, PointerButton};

    #[test]
    fn annotation_preserves_event_order() {
        let raw = vec![
            RawInputEvent::KeyPressed {
                key: KeyCode::from_code(1),
            },
            RawInputEvent::PointerPressed {
                button: PointerButton::Primary,
            },
            RawInputEvent::FocusLost,
        ];
        let routed = all_open(raw.clone());
        assert_eq!(routed.len(), 3);
        for (routed, raw) in routed.iter().zip(raw.iter()) {
            assert_eq!(&routed.event, raw);
            assert!(!routed.captured);
        }
        let captured = all_captured(raw.clone());
        for (routed, raw) in captured.iter().zip(raw.iter()) {
            assert_eq!(&routed.event, raw);
            assert!(routed.captured);
        }
    }
}
