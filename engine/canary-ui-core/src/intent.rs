//! User intents: widget activations returned to the runtime.

/// Identity of one widget, for intent routing.
///
/// A `&'static str` keeps the first-game slice allocation-free; a richer id
/// (numeric handle, widget path) arrives when a second consumer needs it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct UiId(pub &'static str);

impl UiId {
    /// Names a widget. Ids must be unique within one frame's widget tree;
    /// two live widgets sharing an id route both activations to the same
    /// intent, which is a game bug, not a UI bug.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }
}

/// A widget activation the runtime applies at the next simulation boundary.
///
/// Intents travel on a channel separate from `SimulationInput`: they are UI
/// consequences (a button was pressed), not gameplay actions (jump is held).
/// The UI never holds a live `World` borrow, so a button cannot mutate game
/// state directly — it returns an intent, and the runtime applies it where
/// the game declared the boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum UiIntent {
    /// A button widget was activated this frame.
    ButtonPressed(UiId),
}

/// Widget activations published as an ECS resource: the frame driver
/// overwrites this once per outer frame, and the simulation pass reads it
/// at the declared boundary. Separate from gameplay's `SimulationInput` on
/// purpose — intents are UI consequences (a button was pressed), not
/// gameplay actions (jump is held). The dependency runs upward
/// (`canary-runtime` knows both crates); gameplay input never depends on UI.
#[derive(Clone, Debug, Default)]
pub struct UiIntents {
    /// Activations in activation order; empty on frames with no widgets.
    pub intents: Vec<UiIntent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_pressed_intent_carries_its_widget_id() {
        let id = UiId::new("pause");
        let intent = UiIntent::ButtonPressed(id);
        assert_eq!(intent, UiIntent::ButtonPressed(UiId("pause")));
        assert_ne!(intent, UiIntent::ButtonPressed(UiId::new("resume")));
    }

    #[test]
    fn intents_resource_defaults_to_empty_and_preserves_order() {
        assert!(UiIntents::default().intents.is_empty());
        let intents = UiIntents {
            intents: vec![
                UiIntent::ButtonPressed(UiId::new("a")),
                UiIntent::ButtonPressed(UiId::new("b")),
            ],
        };
        assert_eq!(
            intents.intents,
            vec![
                UiIntent::ButtonPressed(UiId::new("a")),
                UiIntent::ButtonPressed(UiId::new("b")),
            ]
        );
    }
}
