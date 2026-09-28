//! UI-before-gameplay input wiring: one frame of platform events through
//! the UI backend's first refusal, then gameplay mapping.
//!
//! The frame driver calls [`drive_input_frame`] once per outer frame with
//! the drained platform events. The UI backend sees the events first and
//! reports what it captured; the mapper drops captured events unless a
//! game-declared pass-through binding covers them. Widget activations come
//! back as intents for the runtime to apply at the next simulation
//! boundary — never as direct `World` mutations.
//!
//! What this slice does **not** include: applying intents (the frame
//! driver's simulation-boundary step), tick stamping (the pass driver
//! stamps [`SimulationInput::tick`](canary_input::SimulationInput::tick)
//! immediately before the scheduled pass), or paint submission (the
//! presenter consumes [`InputFrameOutput::paint`]).

use canary_input::{
    raw_event_from_platform, InputMapper, RawInputEvent, RoutedEvent, SimulationInput,
};
use canary_ui_core::{CaptureResult, UiBackend, UiBuilder, UiFrameInput, UiIntent, UiPaint};

/// Everything about one outer frame's UI/input work: the UI frame the
/// backend advances, the game's widget build closure, and the frame
/// identity the mapper stamps on the resulting snapshot.
pub struct InputFrame<'a> {
    /// Platform events plus viewport, focus, and clock for the UI frame.
    pub ui_input: UiFrameInput<'a>,
    /// The game's widget build pass. Runs inside the backend's frame —
    /// the driver never builds widgets itself.
    pub build: &'a mut dyn FnMut(&mut dyn UiBuilder),
    /// Outer-loop identity stamped on the snapshot; the ECS tick is
    /// stamped later, immediately before the scheduled pass.
    pub frame_index: u64,
}

/// Everything one input frame produces: the logical snapshot for the
/// simulation, the widget activations awaiting the next simulation
/// boundary, and the UI paint for the presenter.
pub struct InputFrameOutput {
    /// Deterministic per-frame snapshot; `tick` is `None` until the
    /// pass driver stamps it immediately before the scheduled pass.
    pub input: SimulationInput,
    /// Widget activations in activation order, for the runtime to apply
    /// at the next declared simulation boundary.
    pub intents: Vec<UiIntent>,
    /// Tessellated widget output for the presenter.
    pub paint: UiPaint,
}

/// Drives one frame of UI-before-gameplay input: converts the drained
/// platform events, gives the UI backend first refusal, annotates each
/// raw event with the resulting capture verdict, and routes the full
/// ordered stream through the mapper.
///
/// The mapper receives the *full* stream with per-event flags — never a
/// pre-filtered stream — because capture is judged against bindings at
/// mapping time. Order is preserved end to end.
pub fn drive_input_frame(
    frame: InputFrame<'_>,
    backend: &mut impl UiBackend,
    mapper: &mut InputMapper,
) -> InputFrameOutput {
    let raw: Vec<RawInputEvent> = frame
        .ui_input
        .events
        .iter()
        .map(raw_event_from_platform)
        .collect();
    let ui_output = backend.run_frame(&frame.ui_input, frame.build);
    let routed = annotate_capture(&raw, ui_output.capture);
    let input = mapper.route(&routed, frame.frame_index);
    InputFrameOutput {
        input,
        intents: ui_output.intents,
        paint: ui_output.paint,
    }
}

/// Annotates each raw event with the UI backend's capture verdict.
///
/// Class membership is structural: key transitions belong to the
/// keyboard class, pointer presses/releases to the pointer class.
/// System and routing events ([`RawInputEvent::PointerMoved`],
/// [`RawInputEvent::PointerLeave`], [`RawInputEvent::FocusLost`],
/// [`RawInputEvent::CaptureTaken`]) are never UI-consumable — they
/// always arrive open. Position carries no digital hold state, and
/// leave/focus/capture-taken exist precisely to *release* gameplay
/// holds, so consuming them would stick input rather than route it.
fn annotate_capture(events: &[RawInputEvent], capture: CaptureResult) -> Vec<RoutedEvent> {
    events
        .iter()
        .map(|event| {
            let captured = match event {
                RawInputEvent::KeyPressed { .. } | RawInputEvent::KeyReleased { .. } => {
                    capture.keyboard
                }
                RawInputEvent::PointerPressed { .. } | RawInputEvent::PointerReleased { .. } => {
                    capture.pointer
                }
                RawInputEvent::PointerMoved { .. }
                | RawInputEvent::PointerLeave { .. }
                | RawInputEvent::FocusLost
                | RawInputEvent::CaptureTaken { .. } => false,
            };
            RoutedEvent {
                event: *event,
                captured,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_input::{
        ActionId, ActionSchema, Binding, KeyCode, PhysicalControl,
        PointerButton as InputPointerButton,
    };
    use canary_platform::{InputEvent, Key as PlatformKey, PointerButton as PlatformPointerButton};
    use canary_ui_core::{UiClipRect, UiDrawBatch, UiFrameOutput, UiId, UiTextureId};

    /// A canned UI backend: reports a fixed capture verdict, returns
    /// fixed intents and paint, and runs the game's build closure
    /// against a recording builder so tests can prove the driver hands
    /// widget construction to the backend.
    struct StubBackend {
        capture: CaptureResult,
        intents: Vec<UiIntent>,
        paint: UiPaint,
        labels_built: Vec<String>,
    }

    impl StubBackend {
        fn new(capture: CaptureResult) -> Self {
            Self {
                capture,
                intents: Vec::new(),
                paint: UiPaint::default(),
                labels_built: Vec::new(),
            }
        }
    }

    struct RecordingBuilder<'a> {
        labels: &'a mut Vec<String>,
    }

    impl UiBuilder for RecordingBuilder<'_> {
        fn label(&mut self, text: &str) {
            self.labels.push(text.to_string());
        }

        fn button(&mut self, _id: UiId, _label: &str) -> bool {
            false
        }
    }

    impl UiBackend for StubBackend {
        fn run_frame(
            &mut self,
            _input: &UiFrameInput<'_>,
            build: &mut dyn FnMut(&mut dyn UiBuilder),
        ) -> UiFrameOutput {
            let mut recorder = RecordingBuilder {
                labels: &mut self.labels_built,
            };
            build(&mut recorder);
            UiFrameOutput {
                capture: self.capture,
                intents: std::mem::take(&mut self.intents),
                paint: std::mem::take(&mut self.paint),
            }
        }
    }

    /// Jump on the `B` key, fire on the primary pointer button.
    fn two_action_mapper() -> (ActionId, ActionId, InputMapper) {
        let (schema, ids) = ActionSchema::declare(["jump", "fire"]).expect("two actions declare");
        let (jump, fire) = (ids[0], ids[1]);
        let mut mapper = InputMapper::new(schema);
        mapper
            .add_binding(Binding::gameplay(
                PhysicalControl::Key(KeyCode::from_code(1)),
                jump,
            ))
            .expect("jump binding registers");
        mapper
            .add_binding(Binding::gameplay(
                PhysicalControl::Pointer(InputPointerButton::Primary),
                fire,
            ))
            .expect("fire binding registers");
        (jump, fire, mapper)
    }

    fn frame_for<'a>(
        events: &'a [InputEvent],
        build: &'a mut dyn FnMut(&mut dyn UiBuilder),
        frame_index: u64,
    ) -> InputFrame<'a> {
        InputFrame {
            ui_input: UiFrameInput {
                events,
                screen_width_px: 320.0,
                screen_height_px: 240.0,
                focused: true,
                dt_seconds: 1.0 / 60.0,
            },
            build,
            frame_index,
        }
    }

    fn non_empty_paint() -> UiPaint {
        UiPaint {
            batches: vec![UiDrawBatch {
                clip: UiClipRect {
                    min_x: 0.0,
                    min_y: 0.0,
                    max_x: 320.0,
                    max_y: 240.0,
                },
                texture: UiTextureId::Managed(1),
                triangles: Vec::new(),
            }],
            textures: Vec::new(),
        }
    }

    #[test]
    fn captured_keyboard_event_produces_no_gameplay_action() {
        let (jump, _fire, mut mapper) = two_action_mapper();
        let mut backend = StubBackend::new(CaptureResult {
            keyboard: true,
            ..CaptureResult::none()
        });
        let events = [InputEvent::KeyPressed(PlatformKey::B)];
        let mut build = |ui: &mut dyn UiBuilder| ui.label("hud");
        let output =
            drive_input_frame(frame_for(&events, &mut build, 7), &mut backend, &mut mapper);

        assert!(!output.input.was_pressed(jump));
        assert!(!output.input.is_down(jump));
        assert_eq!(output.input.frame_index, 7);
        assert_eq!(output.input.tick, None);
        assert_eq!(backend.labels_built, vec!["hud".to_string()]);
    }

    #[test]
    fn pass_through_binding_fires_despite_capture() {
        let (schema, ids) = ActionSchema::declare(["jump"]).expect("jump declares");
        let mut mapper = InputMapper::new(schema);
        mapper
            .add_binding(Binding::pass_through(
                PhysicalControl::Key(KeyCode::from_code(1)),
                ids[0],
            ))
            .expect("pass-through binding registers");
        let mut backend = StubBackend::new(CaptureResult::all());
        let events = [InputEvent::KeyPressed(PlatformKey::B)];
        let mut build = |_ui: &mut dyn UiBuilder| {};
        let output =
            drive_input_frame(frame_for(&events, &mut build, 3), &mut backend, &mut mapper);

        assert!(output.input.was_pressed(ids[0]));
    }

    #[test]
    fn open_pointer_event_fires_while_keyboard_captured() {
        let (jump, fire, mut mapper) = two_action_mapper();
        let mut backend = StubBackend::new(CaptureResult {
            keyboard: true,
            ..CaptureResult::none()
        });
        let events = [
            InputEvent::KeyPressed(PlatformKey::B),
            InputEvent::PointerPressed(PlatformPointerButton::Primary),
        ];
        let mut build = |_ui: &mut dyn UiBuilder| {};
        let output =
            drive_input_frame(frame_for(&events, &mut build, 9), &mut backend, &mut mapper);

        assert!(!output.input.was_pressed(jump));
        assert!(output.input.was_pressed(fire));
    }

    #[test]
    fn intents_and_paint_flow_to_output_in_activation_order() {
        let (_jump, _fire, mut mapper) = two_action_mapper();
        let mut backend = StubBackend::new(CaptureResult::none());
        backend.intents = vec![
            UiIntent::ButtonPressed(UiId::new("pause")),
            UiIntent::ButtonPressed(UiId::new("resume")),
        ];
        backend.paint = non_empty_paint();
        let events = [InputEvent::PointerMoved { x: 10.0, y: 20.0 }];
        let mut build = |_ui: &mut dyn UiBuilder| {};
        let output = drive_input_frame(
            frame_for(&events, &mut build, 11),
            &mut backend,
            &mut mapper,
        );

        assert_eq!(
            output.intents,
            vec![
                UiIntent::ButtonPressed(UiId::new("pause")),
                UiIntent::ButtonPressed(UiId::new("resume")),
            ]
        );
        assert!(!output.paint.is_empty());
        assert_eq!(output.paint.batches.len(), 1);
    }

    #[test]
    fn system_events_are_never_annotated_captured() {
        let routed = annotate_capture(
            &[
                RawInputEvent::PointerMoved { x: 1.0, y: 2.0 },
                RawInputEvent::PointerLeave {
                    pointer_capture_held: false,
                },
                RawInputEvent::FocusLost,
            ],
            CaptureResult::all(),
        );
        assert_eq!(routed.len(), 3);
        for event in &routed {
            assert!(!event.captured);
        }
    }
}
