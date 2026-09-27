// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Physical bindings and the [`InputMapper`] that routes ordered,
//! capture-flagged events into aggregate action state (ADR 0025, decisions
//! 2–4).
//!
//! Multiple physical bindings to one action aggregate *before* edge
//! derivation: the action presses only on the aggregate up-to-down
//! transition and releases only on the aggregate down-to-up transition. A
//! repeated press while already held never synthesizes another edge.
//!
//! The only `canary-platform` types this module names are the normalized
//! [`canary_platform::InputEvent`]/[`canary_platform::Key`] inputs to the
//! explicit conversion seam ([`raw_event_from_platform`],
//! [`drain_platform_events`]); no `winit`, `egui`, scancode, or other
//! OS-specific type appears anywhere here. Converted events become the
//! backend-neutral [`RawInputEvent`], which is all the mapper ever sees.

use std::collections::HashSet;

use thiserror::Error;

use crate::snapshot::{ActionSnapshot, ActionState};
use crate::{ActionId, ActionSchema, PlayerSlot, RoutedEvent, SimulationInput};

/// A backend-neutral physical key identifier: "the key in this location on
/// the keyboard", matching the physical-position model of
/// [`canary_platform::Key`].
///
/// The `u32` payload is a stable code assigned by [`KeyCode::from_code`]
/// and [`KeyCode::from_platform_key`]. Named-key codes are fixed by the
/// conversion table below and never change within a schema; games treat
/// them as opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCode(u32);

impl KeyCode {
    /// Codes at or above this base belong to `Key::Other` payloads
    /// (`base + raw code`, wrapping). Named keys always map below it, so
    /// the two ranges never overlap for realistic raw codes.
    pub const OTHER_BASE: u32 = 1_000_000;

    /// Code produced for a platform key this crate does not name yet
    /// (the `non_exhaustive` wildcard arm): distinct from every table and
    /// `Other` code, so future keys degrade to a distinguishable control
    /// rather than aliasing a real one.
    pub const UNMAPPED_FUTURE: u32 = u32::MAX;

    /// Wraps a raw stable code, e.g. for tests or game-side key tables.
    pub const fn from_code(code: u32) -> Self {
        Self(code)
    }

    /// The raw stable code.
    pub const fn code(self) -> u32 {
        self.0
    }

    /// Maps a normalized platform key to its stable code.
    ///
    /// Named keys map to `0..=82` in the platform enum's declaration
    /// order (letters, digits, function keys, modifiers, navigation,
    /// editing, punctuation). `Key::Other(code)` maps to
    /// [`KeyCode::OTHER_BASE`] plus the raw code. Platform variants added
    /// after this table was written map to
    /// [`KeyCode::UNMAPPED_FUTURE`].
    pub fn from_platform_key(key: canary_platform::Key) -> Self {
        use canary_platform::Key as PlatformKey;
        let code = match key {
            PlatformKey::A => 0,
            PlatformKey::B => 1,
            PlatformKey::C => 2,
            PlatformKey::D => 3,
            PlatformKey::E => 4,
            PlatformKey::F => 5,
            PlatformKey::G => 6,
            PlatformKey::H => 7,
            PlatformKey::I => 8,
            PlatformKey::J => 9,
            PlatformKey::K => 10,
            PlatformKey::L => 11,
            PlatformKey::M => 12,
            PlatformKey::N => 13,
            PlatformKey::O => 14,
            PlatformKey::P => 15,
            PlatformKey::Q => 16,
            PlatformKey::R => 17,
            PlatformKey::S => 18,
            PlatformKey::T => 19,
            PlatformKey::U => 20,
            PlatformKey::V => 21,
            PlatformKey::W => 22,
            PlatformKey::X => 23,
            PlatformKey::Y => 24,
            PlatformKey::Z => 25,
            PlatformKey::Digit0 => 26,
            PlatformKey::Digit1 => 27,
            PlatformKey::Digit2 => 28,
            PlatformKey::Digit3 => 29,
            PlatformKey::Digit4 => 30,
            PlatformKey::Digit5 => 31,
            PlatformKey::Digit6 => 32,
            PlatformKey::Digit7 => 33,
            PlatformKey::Digit8 => 34,
            PlatformKey::Digit9 => 35,
            PlatformKey::F1 => 36,
            PlatformKey::F2 => 37,
            PlatformKey::F3 => 38,
            PlatformKey::F4 => 39,
            PlatformKey::F5 => 40,
            PlatformKey::F6 => 41,
            PlatformKey::F7 => 42,
            PlatformKey::F8 => 43,
            PlatformKey::F9 => 44,
            PlatformKey::F10 => 45,
            PlatformKey::F11 => 46,
            PlatformKey::F12 => 47,
            PlatformKey::ShiftLeft => 48,
            PlatformKey::ShiftRight => 49,
            PlatformKey::ControlLeft => 50,
            PlatformKey::ControlRight => 51,
            PlatformKey::AltLeft => 52,
            PlatformKey::AltRight => 53,
            PlatformKey::SuperLeft => 54,
            PlatformKey::SuperRight => 55,
            PlatformKey::ArrowUp => 56,
            PlatformKey::ArrowDown => 57,
            PlatformKey::ArrowLeft => 58,
            PlatformKey::ArrowRight => 59,
            PlatformKey::Home => 60,
            PlatformKey::End => 61,
            PlatformKey::PageUp => 62,
            PlatformKey::PageDown => 63,
            PlatformKey::Insert => 64,
            PlatformKey::Delete => 65,
            PlatformKey::Escape => 66,
            PlatformKey::Space => 67,
            PlatformKey::Enter => 68,
            PlatformKey::Tab => 69,
            PlatformKey::Backspace => 70,
            PlatformKey::CapsLock => 71,
            PlatformKey::Minus => 72,
            PlatformKey::Equal => 73,
            PlatformKey::BracketLeft => 74,
            PlatformKey::BracketRight => 75,
            PlatformKey::Backslash => 76,
            PlatformKey::Semicolon => 77,
            PlatformKey::Quote => 78,
            PlatformKey::Comma => 79,
            PlatformKey::Period => 80,
            PlatformKey::Slash => 81,
            PlatformKey::Backquote => 82,
            PlatformKey::Other(raw) => Self::OTHER_BASE.wrapping_add(raw),
            // `Key` is `non_exhaustive`: later platform variants land here.
            _ => Self::UNMAPPED_FUTURE,
        };
        Self(code)
    }
}

/// A backend-neutral pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerButton {
    /// The primary (usually left) button.
    Primary,
    /// The secondary (usually right) button.
    Secondary,
    /// The middle button.
    Middle,
    /// Any further button, by index.
    Other(u8),
}

/// One physical control a binding can name: a key or a pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicalControl {
    /// A physical keyboard key.
    Key(KeyCode),
    /// A pointer button.
    Pointer(PointerButton),
}

/// One backend-neutral normalized raw input event.
///
/// Keyboard transitions and pointer buttons drive digital actions;
/// [`RawInputEvent::PointerMoved`] carries position for UI routing order
/// but changes no digital hold. Positions are in logical pixels — the
/// physical `surface_extent` seam is never conflated with them. The
/// synthesis signals ([`RawInputEvent::FocusLost`],
/// [`RawInputEvent::PointerLeave`], [`RawInputEvent::CaptureTaken`]) are
/// mapper-directed: they are always processed, never UI-consumable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawInputEvent {
    /// A physical key transitioned from up to down.
    KeyPressed {
        /// Which key moved.
        key: KeyCode,
    },
    /// A physical key transitioned from down to up.
    KeyReleased {
        /// Which key moved.
        key: KeyCode,
    },
    /// The pointer moved. Carried for stream-order fidelity; digital
    /// aggregation ignores it.
    PointerMoved {
        /// Horizontal position in logical pixels.
        x: f32,
        /// Vertical position in logical pixels.
        y: f32,
    },
    /// A pointer button transitioned from up to down.
    PointerPressed {
        /// Which button moved.
        button: PointerButton,
    },
    /// A pointer button transitioned from down to up.
    PointerReleased {
        /// Which button moved.
        button: PointerButton,
    },
    /// The pointer left the window while buttons may be held. Synthesizes
    /// releases for pointer-held controls — unless pointer capture is held
    /// for the drag, in which case a captured pointer cannot "leave"
    /// mid-gesture and the event is a no-op.
    PointerLeave {
        /// Whether pointer capture is held for the in-progress gesture.
        pointer_capture_held: bool,
    },
    /// The window lost focus. Clears every held control and synthesizes
    /// releases for actions that were down, so no held action can stick.
    FocusLost,
    /// The UI began capturing an already-held physical control. Gameplay
    /// receives a release for the affected actions before the mapper
    /// forgets the hold.
    CaptureTaken {
        /// The control the UI took over.
        control: PhysicalControl,
    },
}

impl RawInputEvent {
    /// The physical control this event moves, if it is a control
    /// transition (key/pointer press or release).
    pub fn control(self) -> Option<PhysicalControl> {
        match self {
            Self::KeyPressed { key } | Self::KeyReleased { key } => Some(PhysicalControl::Key(key)),
            Self::PointerPressed { button } | Self::PointerReleased { button } => {
                Some(PhysicalControl::Pointer(button))
            }
            Self::PointerMoved { .. }
            | Self::PointerLeave { .. }
            | Self::FocusLost
            | Self::CaptureTaken { .. } => None,
        }
    }

    /// Whether this event latches a hold (`KeyPressed`/`PointerPressed`).
    fn is_press(self) -> bool {
        matches!(self, Self::KeyPressed { .. } | Self::PointerPressed { .. })
    }
}

/// One game-declared physical-to-action binding.
///
/// Bindings are evaluated inside the mapper against the per-event capture
/// flag: a captured event is dropped *unless* a binding with
/// `pass_through == true` covers its control. Pass-through plus capture is
/// the context switch — there is no separate mapping-contexts layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The physical control that drives the action.
    pub control: PhysicalControl,
    /// The declared action it drives.
    pub action: ActionId,
    /// Whether this binding still fires when the UI captured the event.
    pub pass_through: bool,
}

impl Binding {
    /// A gameplay binding: dropped when the UI captures the event.
    pub fn gameplay(control: PhysicalControl, action: ActionId) -> Self {
        Self {
            control,
            action,
            pass_through: false,
        }
    }

    /// A binding that fires even when the UI captured the event.
    pub fn pass_through(control: PhysicalControl, action: ActionId) -> Self {
        Self {
            control,
            action,
            pass_through: true,
        }
    }
}

/// Failure to register a [`Binding`] on an [`InputMapper`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MappingError {
    /// The binding names an action its mapper's schema never declared.
    #[error("binding targets undeclared action {id:?}")]
    UnknownAction {
        /// The undeclared action identity.
        id: ActionId,
    },
}

/// A binding with its action resolved to a schema declaration position.
#[derive(Debug, Clone, Copy)]
struct ResolvedBinding {
    /// The physical control that drives the action.
    control: PhysicalControl,
    /// Declaration position of the driven action.
    action: usize,
    /// Whether the binding survives UI capture.
    pass_through: bool,
}

/// Routes ordered, capture-flagged raw events into aggregate action state.
///
/// Holds the latch set (which physical controls are down) across passes so
/// edges derive from genuine aggregate transitions: `pressed` only on an
/// up-to-down pass, `released` only on a down-to-up pass, never on steady
/// holds. Duplicate presses and stray releases change nothing. Use one
/// mapper per local player; the mapper itself is frame-agnostic and stamps
/// [`SimulationInput::frame_index`] from the `frame_index` each
/// [`InputMapper::route`] call receives.
#[derive(Debug)]
pub struct InputMapper {
    /// Canonical action order and identity validation.
    schema: ActionSchema,
    /// Bindings with schema positions resolved at registration.
    bindings: Vec<ResolvedBinding>,
    /// Bound controls per action position; rebuilt on registration.
    controls_by_action: Vec<Vec<PhysicalControl>>,
    /// Physical controls currently latched down.
    held: HashSet<PhysicalControl>,
}

impl InputMapper {
    /// Creates a mapper over `schema` with no bindings and nothing held.
    pub fn new(schema: ActionSchema) -> Self {
        let mut controls_by_action = Vec::with_capacity(schema.len());
        for _ in 0..schema.len() {
            controls_by_action.push(Vec::new());
        }
        Self {
            schema,
            bindings: Vec::new(),
            controls_by_action,
            held: HashSet::new(),
        }
    }

    /// The schema this mapper routes against.
    pub fn schema(&self) -> &ActionSchema {
        &self.schema
    }

    /// Registers one binding.
    ///
    /// # Errors
    ///
    /// Returns [`MappingError::UnknownAction`] when `binding.action` was
    /// never declared in this mapper's schema.
    pub fn add_binding(&mut self, binding: Binding) -> Result<(), MappingError> {
        let Some(action) = self.schema.position(binding.action) else {
            return Err(MappingError::UnknownAction { id: binding.action });
        };
        self.bindings.push(ResolvedBinding {
            control: binding.control,
            action,
            pass_through: binding.pass_through,
        });
        self.controls_by_action[action].push(binding.control);
        Ok(())
    }

    /// Whether any currently held control drives `action`.
    fn aggregate_for(&self, held: &HashSet<PhysicalControl>, action: usize) -> bool {
        self.controls_by_action[action]
            .iter()
            .any(|control| held.contains(control))
    }

    /// The aggregate hold of every action, in declaration order.
    fn current_aggregate(&self) -> Vec<bool> {
        let mut aggregate = Vec::with_capacity(self.controls_by_action.len());
        for action in 0..self.controls_by_action.len() {
            aggregate.push(self.aggregate_for(&self.held, action));
        }
        aggregate
    }

    /// Whether a captured event for `control` still reaches gameplay: only
    /// when a pass-through binding covers that control.
    fn passes_capture(&self, control: PhysicalControl) -> bool {
        self.bindings
            .iter()
            .any(|binding| binding.control == control && binding.pass_through)
    }

    /// Declaration positions of actions bound to `control`.
    fn actions_for(&self, control: PhysicalControl) -> Vec<usize> {
        self.bindings
            .iter()
            .filter(|binding| binding.control == control)
            .map(|binding| binding.action)
            .collect()
    }

    /// Re-derives the aggregate of the listed actions from the latch set,
    /// recording aggregate transitions into the edge flags.
    fn resync_actions(
        &self,
        actions: &[usize],
        aggregate: &mut [bool],
        went_down: &mut [bool],
        went_up: &mut [bool],
    ) {
        for action in actions {
            let index = *action;
            let held = self.aggregate_for(&self.held, index);
            if held != aggregate[index] {
                aggregate[index] = held;
                went_down[index] |= held;
                went_up[index] |= !held;
            }
        }
    }

    /// Re-derives every action's aggregate (used after mass latch changes
    /// like focus loss, where any action may have moved).
    fn resync_all(&self, aggregate: &mut [bool], went_down: &mut [bool], went_up: &mut [bool]) {
        for action in 0..self.controls_by_action.len() {
            let held = self.aggregate_for(&self.held, action);
            if held != aggregate[action] {
                aggregate[action] = held;
                went_down[action] |= held;
                went_up[action] |= !held;
            }
        }
    }

    /// Latches a press unless it is a duplicate (already held) or a
    /// UI-consumed event with no pass-through cover.
    fn press_control(
        &mut self,
        control: PhysicalControl,
        captured: bool,
        aggregate: &mut [bool],
        went_down: &mut [bool],
        went_up: &mut [bool],
    ) {
        if captured && !self.passes_capture(control) {
            return;
        }
        if self.held.insert(control) {
            let affected = self.actions_for(control);
            self.resync_actions(&affected, aggregate, went_down, went_up);
        }
        // A duplicate press while already held latches nothing and
        // synthesizes no edge — OS repeat must not create presses.
    }

    /// Unlatches a release unless it is stray (not held) or UI-consumed
    /// with no pass-through cover. A consumed press never latched, so a
    /// later open release of it correctly finds nothing held.
    fn release_control(
        &mut self,
        control: PhysicalControl,
        captured: bool,
        aggregate: &mut [bool],
        went_down: &mut [bool],
        went_up: &mut [bool],
    ) {
        if captured && !self.passes_capture(control) {
            return;
        }
        if self.held.remove(&control) {
            let affected = self.actions_for(control);
            self.resync_actions(&affected, aggregate, went_down, went_up);
        }
    }

    /// Routes one pass of ordered, capture-flagged events into a
    /// [`SimulationInput`] snapshot.
    ///
    /// Events are processed in slice order; the snapshot lists actions in
    /// schema declaration order either way. `frame_index` is stamped here,
    /// at route time; `tick` stays `None` until the runtime stamps it via
    /// [`SimulationInput::stamp_tick`] immediately before the scheduled
    /// pass. This method touches no ECS state at all — delivery happens
    /// separately through [`SimulationInput::publish`].
    pub fn route(&mut self, events: &[RoutedEvent], frame_index: u64) -> SimulationInput {
        let mut aggregate = self.current_aggregate();
        let mut went_down = vec![false; self.controls_by_action.len()];
        let mut went_up = vec![false; self.controls_by_action.len()];

        for routed in events {
            match routed.event {
                RawInputEvent::PointerMoved { .. } => {
                    // Position is UI-routing information; digital holds are
                    // unaffected. The event still occupies its stream slot,
                    // so ordering around it is preserved.
                }
                RawInputEvent::PointerLeave {
                    pointer_capture_held,
                } => {
                    if pointer_capture_held {
                        // A captured pointer cannot leave mid-gesture.
                        continue;
                    }
                    let pointer_held: Vec<PhysicalControl> = self
                        .held
                        .iter()
                        .copied()
                        .filter(|control| matches!(control, PhysicalControl::Pointer(_)))
                        .collect();
                    for control in pointer_held {
                        self.held.remove(&control);
                    }
                    self.resync_all(&mut aggregate, &mut went_down, &mut went_up);
                }
                RawInputEvent::FocusLost => {
                    // Never retain a stuck input across focus changes.
                    self.held.clear();
                    self.resync_all(&mut aggregate, &mut went_down, &mut went_up);
                }
                RawInputEvent::CaptureTaken { control } => {
                    // Mapper-directed synthesis, never UI-consumable: the
                    // UI took an already-held control, so gameplay gets its
                    // release before the latch is forgotten.
                    if self.held.remove(&control) {
                        let affected = self.actions_for(control);
                        self.resync_actions(
                            &affected,
                            &mut aggregate,
                            &mut went_down,
                            &mut went_up,
                        );
                    }
                }
                control_event => {
                    // Key/pointer press or release (the only variants with
                    // `control()` targeting `Some`); capture is judged per
                    // binding inside the press/release helpers.
                    let control = control_event.control().expect(
                        "press/release variants always name a control; all other variants are matched above",
                    );
                    if control_event.is_press() {
                        self.press_control(
                            control,
                            routed.captured,
                            &mut aggregate,
                            &mut went_down,
                            &mut went_up,
                        );
                    } else {
                        self.release_control(
                            control,
                            routed.captured,
                            &mut aggregate,
                            &mut went_down,
                            &mut went_up,
                        );
                    }
                }
            }
        }

        let actions = self
            .schema
            .action_ids()
            .into_iter()
            .enumerate()
            .map(|(position, id)| ActionSnapshot {
                id,
                state: ActionState {
                    down: aggregate[position],
                    pressed: went_down[position],
                    released: went_up[position],
                },
            })
            .collect();
        SimulationInput {
            player: PlayerSlot::LOCAL,
            frame_index,
            tick: None,
            actions,
        }
    }
}

/// Converts one normalized platform event into the backend-neutral
/// vocabulary the mapper routes.
pub fn raw_event_from_platform(event: &canary_platform::InputEvent) -> RawInputEvent {
    match event {
        canary_platform::InputEvent::KeyPressed(key) => RawInputEvent::KeyPressed {
            key: KeyCode::from_platform_key(*key),
        },
        canary_platform::InputEvent::KeyReleased(key) => RawInputEvent::KeyReleased {
            key: KeyCode::from_platform_key(*key),
        },
    }
}

/// Drains every queued event from a platform [`canary_platform::InputSource`]
/// (including [`canary_platform::HeadlessInput`] injection in headless
/// tests) into backend-neutral raw events, preserving poll order.
pub fn drain_platform_events(source: &mut impl canary_platform::InputSource) -> Vec<RawInputEvent> {
    source.poll().iter().map(raw_event_from_platform).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{all_captured, all_open};
    use canary_ecs::World;
    use canary_platform::{HeadlessInput, InputEvent, InputSource, Key};

    const KEY_A: KeyCode = KeyCode::from_code(11);
    const KEY_B: KeyCode = KeyCode::from_code(12);

    fn press(key: KeyCode) -> RawInputEvent {
        RawInputEvent::KeyPressed { key }
    }

    fn release(key: KeyCode) -> RawInputEvent {
        RawInputEvent::KeyReleased { key }
    }

    /// A mapper over `["jump", "fire"]` with `KEY_A -> jump` and
    /// `KEY_B -> fire` gameplay bindings.
    fn two_action_mapper() -> (ActionId, ActionId, InputMapper) {
        let (schema, ids) = ActionSchema::declare(["jump", "fire"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        mapper
            .add_binding(Binding::gameplay(PhysicalControl::Key(KEY_A), ids[0]))
            .unwrap();
        mapper
            .add_binding(Binding::gameplay(PhysicalControl::Key(KEY_B), ids[1]))
            .unwrap();
        (ids[0], ids[1], mapper)
    }

    #[test]
    fn snapshot_lists_actions_in_schema_order_regardless_of_event_order() {
        let (jump, fire, mut mapper) = two_action_mapper();
        // Events arrive fire-first; the snapshot must still read jump-first.
        let snapshot = mapper.route(&all_open(vec![press(KEY_B), press(KEY_A)]), 3);
        assert_eq!(
            snapshot
                .actions
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<ActionId>>(),
            vec![jump, fire]
        );
        assert!(snapshot.was_pressed(jump));
        assert!(snapshot.was_pressed(fire));
        assert_eq!(snapshot.frame_index, 3);
        assert_eq!(snapshot.player, PlayerSlot::LOCAL);
    }

    #[test]
    fn duplicate_press_while_held_synthesizes_no_edge() {
        let (jump, _, mut mapper) = two_action_mapper();
        let first = mapper.route(&all_open(vec![press(KEY_A)]), 0);
        assert!(first.was_pressed(jump));

        // OS repeat / duplicate press on the next pass: steady hold only.
        let second = mapper.route(&all_open(vec![press(KEY_A)]), 1);
        assert!(second.is_down(jump));
        assert!(!second.was_pressed(jump));
        assert!(!second.was_released(jump));

        // An empty pass over unchanged input is fully quiet too.
        let third = mapper.route(&[], 2);
        assert!(third.is_down(jump));
        assert!(!third.was_pressed(jump));
        assert!(!third.was_released(jump));
    }

    #[test]
    fn stray_release_of_an_unheld_control_synthesizes_no_edge() {
        let (jump, _, mut mapper) = two_action_mapper();
        let snapshot = mapper.route(&all_open(vec![release(KEY_A)]), 0);
        assert!(!snapshot.is_down(jump));
        assert!(!snapshot.was_pressed(jump));
        assert!(!snapshot.was_released(jump));
    }

    #[test]
    fn multi_binding_aggregates_before_edge_derivation() {
        let (schema, ids) = ActionSchema::declare(["jump"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        let jump = ids[0];
        mapper
            .add_binding(Binding::gameplay(PhysicalControl::Key(KEY_A), jump))
            .unwrap();
        mapper
            .add_binding(Binding::gameplay(PhysicalControl::Key(KEY_B), jump))
            .unwrap();

        // First binding presses the aggregate.
        let first = mapper.route(&all_open(vec![press(KEY_A)]), 0);
        assert!(first.was_pressed(jump));
        assert!(first.is_down(jump));

        // Second binding while held: no new edge, still down.
        let second = mapper.route(&all_open(vec![press(KEY_B)]), 1);
        assert!(!second.was_pressed(jump));
        assert!(second.is_down(jump));

        // Releasing one of two held bindings: still down, no release edge.
        let third = mapper.route(&all_open(vec![release(KEY_A)]), 2);
        assert!(third.is_down(jump));
        assert!(!third.was_released(jump));

        // Releasing the last held binding: the aggregate releases.
        let fourth = mapper.route(&all_open(vec![release(KEY_B)]), 3);
        assert!(!fourth.is_down(jump));
        assert!(fourth.was_released(jump));
        assert!(!fourth.was_pressed(jump));
    }

    #[test]
    fn press_and_release_in_one_pass_reports_both_edges() {
        let (jump, _, mut mapper) = two_action_mapper();
        let snapshot = mapper.route(&all_open(vec![press(KEY_A), release(KEY_A)]), 0);
        assert!(!snapshot.is_down(jump));
        assert!(snapshot.was_pressed(jump));
        assert!(snapshot.was_released(jump));
    }

    #[test]
    fn focus_loss_synthesizes_releases_and_clears_the_latch() {
        let (jump, fire, mut mapper) = two_action_mapper();
        let _ = mapper.route(&all_open(vec![press(KEY_A), press(KEY_B)]), 0);

        let lost = mapper.route(&all_open(vec![RawInputEvent::FocusLost]), 1);
        assert!(!lost.is_down(jump));
        assert!(!lost.is_down(fire));
        assert!(lost.was_released(jump));
        assert!(lost.was_released(fire));
        assert!(!lost.was_pressed(jump));

        // The latch is forgotten: the next pass is quiet, and a later
        // press is a fresh edge rather than a duplicate.
        let quiet = mapper.route(&[], 2);
        assert!(!quiet.was_released(jump));
        let fresh = mapper.route(&all_open(vec![press(KEY_A)]), 3);
        assert!(fresh.was_pressed(jump));
    }

    #[test]
    fn pointer_leave_releases_pointer_holds_but_keeps_keyboard_holds() {
        let (schema, ids) = ActionSchema::declare(["grab", "jump"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        let (grab, jump) = (ids[0], ids[1]);
        mapper
            .add_binding(Binding::gameplay(
                PhysicalControl::Pointer(PointerButton::Primary),
                grab,
            ))
            .unwrap();
        mapper
            .add_binding(Binding::gameplay(PhysicalControl::Key(KEY_A), jump))
            .unwrap();

        let _ = mapper.route(
            &all_open(vec![
                RawInputEvent::PointerPressed {
                    button: PointerButton::Primary,
                },
                press(KEY_A),
            ]),
            0,
        );
        let left = mapper.route(
            &all_open(vec![RawInputEvent::PointerLeave {
                pointer_capture_held: false,
            }]),
            1,
        );
        assert!(!left.is_down(grab));
        assert!(left.was_released(grab));
        assert!(left.is_down(jump));
        assert!(!left.was_released(jump));
    }

    #[test]
    fn pointer_leave_is_a_no_op_while_pointer_capture_is_held() {
        let (schema, ids) = ActionSchema::declare(["grab"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        let grab = ids[0];
        mapper
            .add_binding(Binding::gameplay(
                PhysicalControl::Pointer(PointerButton::Primary),
                grab,
            ))
            .unwrap();

        let _ = mapper.route(
            &all_open(vec![RawInputEvent::PointerPressed {
                button: PointerButton::Primary,
            }]),
            0,
        );
        // A captured pointer cannot leave mid-gesture: the drag survives.
        let held = mapper.route(
            &all_open(vec![RawInputEvent::PointerLeave {
                pointer_capture_held: true,
            }]),
            1,
        );
        assert!(held.is_down(grab));
        assert!(!held.was_pressed(grab));
        assert!(!held.was_released(grab));
    }

    #[test]
    fn captured_events_drop_unless_a_pass_through_binding_covers_them() {
        let (schema, ids) = ActionSchema::declare(["jump", "menu"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        let (jump, menu) = (ids[0], ids[1]);
        mapper
            .add_binding(Binding::gameplay(PhysicalControl::Key(KEY_A), jump))
            .unwrap();
        mapper
            .add_binding(Binding::pass_through(PhysicalControl::Key(KEY_B), menu))
            .unwrap();

        let snapshot = mapper.route(&all_captured(vec![press(KEY_A), press(KEY_B)]), 0);
        // Consumed gameplay binding: excluded entirely.
        assert!(!snapshot.is_down(jump));
        assert!(!snapshot.was_pressed(jump));
        // Pass-through binding: forwarded despite capture.
        assert!(snapshot.is_down(menu));
        assert!(snapshot.was_pressed(menu));
    }

    #[test]
    fn consumed_press_latches_nothing_so_a_later_open_release_is_stray() {
        let (jump, _, mut mapper) = two_action_mapper();
        // Captured press is dropped: nothing latches.
        let dropped = mapper.route(&all_captured(vec![press(KEY_A)]), 0);
        assert!(!dropped.is_down(jump));

        // The later open release finds nothing held: no phantom edge.
        let stray = mapper.route(&all_open(vec![release(KEY_A)]), 1);
        assert!(!stray.is_down(jump));
        assert!(!stray.was_released(jump));
    }

    #[test]
    fn capture_taken_releases_gameplay_before_forgetting_the_hold() {
        let (jump, _, mut mapper) = two_action_mapper();
        let _ = mapper.route(&all_open(vec![press(KEY_A)]), 0);

        // The UI grabs the already-held key mid-hold: gameplay sees its
        // release now, not a stuck hold later.
        let taken = mapper.route(
            &all_open(vec![RawInputEvent::CaptureTaken {
                control: PhysicalControl::Key(KEY_A),
            }]),
            1,
        );
        assert!(!taken.is_down(jump));
        assert!(taken.was_released(jump));

        // The latch is forgotten: re-pressing is a fresh edge.
        let fresh = mapper.route(&all_open(vec![press(KEY_A)]), 2);
        assert!(fresh.was_pressed(jump));
    }

    #[test]
    fn snapshot_identity_is_frame_now_tick_later() {
        let (jump, _, mut mapper) = two_action_mapper();
        let mut snapshot = mapper.route(&all_open(vec![press(KEY_A)]), 41);
        // The mapper stamps frame_index at route time, never the tick.
        assert_eq!(snapshot.frame_index, 41);
        assert_eq!(snapshot.tick, None);
        assert!(snapshot.was_pressed(jump));

        // The runtime stamps the tick immediately before the scheduled pass.
        let mut world = World::new();
        world.advance_tick();
        let tick = world.change_tick();
        snapshot.stamp_tick(tick);
        assert_eq!(snapshot.tick, Some(tick));
    }

    #[test]
    fn headless_injection_reaches_the_mapper_with_no_window() {
        let mut source = HeadlessInput::new();
        source.inject(InputEvent::KeyPressed(Key::Space));

        // Drain in poll order and route: no window, no pump needed.
        let raw = drain_platform_events(&mut source);
        assert_eq!(raw.len(), 1);
        assert_eq!(
            raw[0],
            RawInputEvent::KeyPressed {
                key: KeyCode::from_platform_key(Key::Space),
            }
        );

        let (schema, ids) = ActionSchema::declare(["jump"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        let jump = ids[0];
        mapper
            .add_binding(Binding::gameplay(
                PhysicalControl::Key(KeyCode::from_platform_key(Key::Space)),
                jump,
            ))
            .unwrap();
        let snapshot = mapper.route(&all_open(raw), 0);
        assert!(snapshot.was_pressed(jump));
        assert!(source.poll().is_empty());
    }

    #[test]
    fn mapping_only_pass_dirties_no_component_change_detection() {
        #[derive(Debug, Clone, Copy, PartialEq)]
        struct Probe(u8);

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Probe(1)).unwrap();
        // Settle: two ticks with no writes, baseline captured between them.
        world.advance_tick();
        let baseline = world.change_tick();
        world.advance_tick();

        // A mapping-only pass over unchanged input, then the mandated
        // per-frame resource publish.
        let (schema, ids) = ActionSchema::declare(["jump"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        let jump = ids[0];
        mapper
            .add_binding(Binding::gameplay(PhysicalControl::Key(KEY_A), jump))
            .unwrap();
        let snapshot = mapper.route(&[], 9);
        assert!(!snapshot.was_pressed(jump));
        assert!(!snapshot.was_released(jump));
        snapshot.publish(&mut world);

        // Component change detection is untouched: the resource write is
        // not component data, so the quiet-tick probe stays clean while
        // the fresh snapshot is still delivered.
        assert!(
            world
                .query_changed_since::<Probe>(baseline)
                .next()
                .is_none(),
            "mapping + resource publish must not dirty component ticks"
        );
        let stored = world.resource::<SimulationInput>().unwrap();
        assert_eq!(stored.frame_index, 9);
        assert!(world.is_alive(entity));
    }

    #[test]
    fn unbound_actions_snapshot_as_up_in_schema_order() {
        let (schema, ids) = ActionSchema::declare(["jump", "fire", "crouch"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        mapper
            .add_binding(Binding::gameplay(PhysicalControl::Key(KEY_A), ids[0]))
            .unwrap();
        let snapshot = mapper.route(&all_open(vec![press(KEY_A)]), 0);
        assert_eq!(
            snapshot
                .actions
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<ActionId>>(),
            ids
        );
        assert!(snapshot.was_pressed(ids[0]));
        assert_eq!(snapshot.state(ids[1]), Some(ActionState::UP));
        assert_eq!(snapshot.state(ids[2]), Some(ActionState::UP));
    }

    #[test]
    fn binding_an_undeclared_action_is_a_typed_error() {
        let (schema, _) = ActionSchema::declare(["jump"]).unwrap();
        let mut mapper = InputMapper::new(schema);
        let (other, other_ids) = ActionSchema::declare(["fire"]).unwrap();
        let _ = other;
        assert_eq!(
            mapper.add_binding(Binding::gameplay(PhysicalControl::Key(KEY_A), other_ids[0])),
            Err(MappingError::UnknownAction { id: other_ids[0] })
        );
    }

    #[test]
    fn platform_key_table_is_stable_and_disjoint() {
        assert_eq!(KeyCode::from_platform_key(Key::A).code(), 0);
        assert_eq!(KeyCode::from_platform_key(Key::Space).code(), 67);
        assert_eq!(KeyCode::from_platform_key(Key::Backquote).code(), 82);
        let other = KeyCode::from_platform_key(Key::Other(5));
        assert!(other.code() >= KeyCode::OTHER_BASE);
        assert_ne!(
            KeyCode::from_platform_key(Key::A),
            KeyCode::from_platform_key(Key::Space)
        );
    }
}
