//! The `egui` backend: input translation, one build pass, paint conversion.

use canary_platform::{InputEvent, Key};
use canary_ui_core::{
    CaptureResult, UiBackend, UiBuilder, UiClipRect, UiDrawBatch, UiFrameInput, UiFrameOutput,
    UiId, UiIntent, UiPaint, UiTriangle, UiVertex,
};

use crate::textures::{map_texture_id, TextureCache};
use crate::translate::{map_button, map_key};

/// `egui` implementation of [`UiBackend`].
///
/// Owns the `egui::Context` plus the integration state `egui` expects the
/// host to keep: wall-clock time, last pointer position, modifier state,
/// and the pressed key/button sets used to synthesize releases on
/// focus-loss and pointer-leave (so neither `egui` nor gameplay ever sees
/// a stuck control).
pub struct EguiBackend {
    /// The immediate-mode context; retained across frames.
    ctx: egui::Context,
    /// Accumulated UI time in seconds, fed to `egui` as monotonic time.
    time_seconds: f64,
    /// Last reported pointer position in logical pixels. Presses that
    /// arrive before any move hit-test at the origin.
    pointer_pos: egui::Pos2,
    /// Modifier state tracked from modifier-key transitions (the platform
    /// slice emits no `ModifiersChanged` of its own).
    modifiers: egui::Modifiers,
    /// Mapped keys currently down, for focus-loss release synthesis.
    pressed_keys: Vec<egui::Key>,
    /// Mapped buttons currently down, for leave/focus release synthesis.
    pressed_buttons: Vec<egui::PointerButton>,
    /// Live textures, for resolving partial atlas updates.
    textures: TextureCache,
}

impl EguiBackend {
    /// Creates a backend with a fresh `egui` context and empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ctx: egui::Context::default(),
            time_seconds: 0.0,
            pointer_pos: egui::Pos2::ZERO,
            modifiers: egui::Modifiers::default(),
            pressed_keys: Vec::new(),
            pressed_buttons: Vec::new(),
            textures: TextureCache::default(),
        }
    }

    /// Translates one frame of platform events to `egui` events, updating
    /// integration state. Unmapped keys and buttons past the fifth produce
    /// no events. `FocusLost` and `PointerLeft` synthesize releases for
    /// everything tracked as held, mirroring mapper semantics.
    fn push_events(&mut self, events: &[InputEvent], out: &mut Vec<egui::Event>) {
        for event in events {
            match event {
                InputEvent::KeyPressed(key) => self.push_key(key, true, out),
                InputEvent::KeyReleased(key) => self.push_key(key, false, out),
                InputEvent::PointerMoved { x, y } => {
                    self.pointer_pos = egui::pos2(*x, *y);
                    out.push(egui::Event::PointerMoved(self.pointer_pos));
                }
                InputEvent::PointerPressed(button) => {
                    if let Some(mapped) = map_button(button) {
                        if !self.pressed_buttons.contains(&mapped) {
                            self.pressed_buttons.push(mapped);
                        }
                        out.push(egui::Event::PointerButton {
                            pos: self.pointer_pos,
                            button: mapped,
                            pressed: true,
                            modifiers: self.modifiers,
                        });
                    }
                }
                InputEvent::PointerReleased(button) => {
                    if let Some(mapped) = map_button(button) {
                        self.pressed_buttons.retain(|held| *held != mapped);
                        out.push(egui::Event::PointerButton {
                            pos: self.pointer_pos,
                            button: mapped,
                            pressed: false,
                            modifiers: self.modifiers,
                        });
                    }
                }
                InputEvent::PointerLeft => {
                    for button in std::mem::take(&mut self.pressed_buttons) {
                        out.push(egui::Event::PointerButton {
                            pos: self.pointer_pos,
                            button,
                            pressed: false,
                            modifiers: self.modifiers,
                        });
                    }
                    out.push(egui::Event::PointerGone);
                }
                InputEvent::FocusLost => {
                    for key in std::mem::take(&mut self.pressed_keys) {
                        out.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: false,
                            repeat: false,
                            modifiers: self.modifiers,
                        });
                    }
                    for button in std::mem::take(&mut self.pressed_buttons) {
                        out.push(egui::Event::PointerButton {
                            pos: self.pointer_pos,
                            button,
                            pressed: false,
                            modifiers: self.modifiers,
                        });
                    }
                    out.push(egui::Event::WindowFocused(false));
                }
            }
        }
    }

    /// Translates one key transition. Modifier keys additionally maintain
    /// [`Self::modifiers`] and emit `ModifiersChanged`, the behavior
    /// `egui` integrations are expected to provide.
    fn push_key(&mut self, key: &Key, pressed: bool, out: &mut Vec<egui::Event>) {
        let Some(mapped) = map_key(key) else {
            return;
        };
        if is_modifier(key) {
            match key {
                Key::ShiftLeft | Key::ShiftRight => self.modifiers.shift = pressed,
                Key::ControlLeft | Key::ControlRight => self.modifiers.ctrl = pressed,
                Key::AltLeft | Key::AltRight => self.modifiers.alt = pressed,
                // Super keys emit the Key event below (so `egui` sees them
                // held) but feed no modifier slot on this adapter's
                // platforms; `command` follows Control (non-macOS rule).
                _ => {}
            }
            self.modifiers.command = self.modifiers.ctrl;
            out.push(egui::Event::ModifiersChanged(self.modifiers));
        }
        if pressed {
            if !self.pressed_keys.contains(&mapped) {
                self.pressed_keys.push(mapped);
            }
        } else {
            self.pressed_keys.retain(|held| *held != mapped);
        }
        out.push(egui::Event::Key {
            key: mapped,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: self.modifiers,
        });
    }
}

impl Default for EguiBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl UiBackend for EguiBackend {
    fn run_frame(
        &mut self,
        input: &UiFrameInput<'_>,
        build: &mut dyn FnMut(&mut dyn UiBuilder),
    ) -> UiFrameOutput {
        let mut events = Vec::with_capacity(input.events.len() + 2);
        self.push_events(input.events, &mut events);
        self.time_seconds += input.dt_seconds;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(input.screen_width_px, input.screen_height_px),
            )),
            time: Some(self.time_seconds),
            focused: input.focused,
            events,
            ..Default::default()
        };

        let mut intents = Vec::new();
        // `run_ui` hands over a root `Ui`; the HUD builds inside a named
        // window so game widgets share one movable panel.
        let output = self.ctx.run_ui(raw, |ui| {
            egui::Window::new("canary-hud").show(ui.ctx(), |ui| {
                let mut builder = EguiBuilder {
                    ui,
                    intents: &mut intents,
                };
                build(&mut builder);
            });
        });

        let capture = CaptureResult {
            keyboard: self.ctx.egui_wants_keyboard_input(),
            pointer: self.ctx.egui_wants_pointer_input(),
        };
        let batches = self
            .ctx
            .tessellate(output.shapes, 1.0)
            .into_iter()
            .filter_map(primitive_to_batch)
            .collect();
        let paint = UiPaint {
            batches,
            textures: self.textures.apply(output.textures_delta),
        };
        UiFrameOutput {
            capture,
            intents,
            paint,
        }
    }
}

/// `true` for the eight modifier keys the adapter tracks into
/// `egui::Modifiers`. `matches!` supplies the implicit wildcard arm:
/// `Key` is `#[non_exhaustive]`, so future keys default to
/// non-modifiers. (An explicit `| _` inside the pattern would match
/// everything — the opposite of the documented behavior.)
fn is_modifier(key: &Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

/// The `UiBuilder` over one `egui` window's `Ui`: widget calls render
/// immediately, button activations become [`UiIntent::ButtonPressed`].
struct EguiBuilder<'a> {
    ui: &'a mut egui::Ui,
    intents: &'a mut Vec<UiIntent>,
}

impl UiBuilder for EguiBuilder<'_> {
    fn label(&mut self, text: &str) {
        self.ui.label(text);
    }

    fn button(&mut self, id: UiId, label: &str) -> bool {
        // Layout telemetry at `debug`: automation (integration tests,
        // scripted live runs) needs the widget rect to aim pointer
        // input, and the builder is the only layer that knows it.
        let response = self.ui.button(label);
        tracing::debug!(?id, rect = ?response.rect, "ui button placed");
        if response.clicked() {
            self.intents.push(UiIntent::ButtonPressed(id));
            true
        } else {
            false
        }
    }
}

/// Converts one tessellated primitive to a draw batch, or `None` for paint
/// callbacks (no first-game widget emits them; the consumer contract is
/// triangles-only).
///
/// UVs pass through untouched: `egui` 0.36 normalizes glyph UVs against
/// the font atlas in its tessellator and emits `WHITE_UV` (0, 0) for
/// untextured shapes, so adapter-side division would double-normalize
/// into garbage — this passthrough is load-bearing, pinned by
/// `batch_uvs_pass_through_verbatim` below.
fn primitive_to_batch(primitive: egui::ClippedPrimitive) -> Option<UiDrawBatch> {
    let egui::ClippedPrimitive {
        clip_rect,
        primitive,
    } = primitive;
    let egui::epaint::Primitive::Mesh(mesh) = primitive else {
        return None;
    };
    let triangles = mesh
        .indices
        .chunks_exact(3)
        .filter_map(|index| {
            let vertex = |i: u32| mesh.vertices.get(usize::try_from(i).ok()?).copied();
            Some(UiTriangle {
                vertices: [
                    vertex_to_ui(vertex(index[0])?),
                    vertex_to_ui(vertex(index[1])?),
                    vertex_to_ui(vertex(index[2])?),
                ],
            })
        })
        .collect();
    Some(UiDrawBatch {
        clip: UiClipRect {
            min_x: clip_rect.min.x,
            min_y: clip_rect.min.y,
            max_x: clip_rect.max.x,
            max_y: clip_rect.max.y,
        },
        texture: map_texture_id(mesh.texture_id),
        triangles,
    })
}

/// Converts one `egui` vertex. `egui` colors are gamma-space bytes;
/// `Rgba::from` converts to linear floats, matching `UiVertex`'s
/// linear-RGBA contract. Positions are logical pixels already; UVs are
/// already 0.0-1.0 (see [`primitive_to_batch`]).
fn vertex_to_ui(vertex: egui::epaint::Vertex) -> UiVertex {
    UiVertex {
        position: [vertex.pos.x, vertex.pos.y],
        uv: [vertex.uv.x, vertex.uv.y],
        color: egui::Rgba::from(vertex.color).to_array(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_platform::PointerButton;

    fn frame_input(events: &[InputEvent]) -> UiFrameInput<'_> {
        UiFrameInput {
            events,
            screen_width_px: 800.0,
            screen_height_px: 600.0,
            focused: true,
            dt_seconds: 1.0 / 60.0,
        }
    }

    fn translate(backend: &mut EguiBackend, events: &[InputEvent]) -> Vec<egui::Event> {
        let mut out = Vec::new();
        backend.push_events(events, &mut out);
        out
    }

    #[test]
    fn unmapped_keys_and_buttons_emit_nothing() {
        let mut backend = EguiBackend::new();
        let events = translate(
            &mut backend,
            &[
                InputEvent::KeyPressed(Key::CapsLock),
                InputEvent::KeyPressed(Key::Other(1)),
                InputEvent::PointerPressed(PointerButton::Other(9)),
            ],
        );
        assert!(events.is_empty());
    }

    #[test]
    fn modifier_press_tracks_modifiers_and_announces_change() {
        let mut backend = EguiBackend::new();
        let events = translate(&mut backend, &[InputEvent::KeyPressed(Key::ShiftLeft)]);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            egui::Event::ModifiersChanged(modifiers) if modifiers.shift
        ));
        assert!(matches!(events[1], egui::Event::Key { pressed: true, .. }));

        let events = translate(&mut backend, &[InputEvent::KeyReleased(Key::ShiftLeft)]);
        assert!(matches!(
            events[0],
            egui::Event::ModifiersChanged(modifiers) if !modifiers.shift
        ));
    }

    #[test]
    fn non_modifier_key_emits_key_event_without_modifiers_change() {
        assert!(!is_modifier(&Key::A));

        let mut backend = EguiBackend::new();
        let events = translate(&mut backend, &[InputEvent::KeyPressed(Key::A)]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], egui::Event::Key { pressed: true, .. }));
    }

    #[test]
    fn focus_loss_synthesizes_releases_then_unfocuses_once() {
        let mut backend = EguiBackend::new();
        translate(
            &mut backend,
            &[
                InputEvent::KeyPressed(Key::A),
                InputEvent::PointerPressed(PointerButton::Primary),
            ],
        );

        let events = translate(&mut backend, &[InputEvent::FocusLost]);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], egui::Event::Key { pressed: false, .. }));
        assert!(matches!(
            events[1],
            egui::Event::PointerButton { pressed: false, .. }
        ));
        assert!(matches!(events[2], egui::Event::WindowFocused(false)));

        // Tracked sets cleared: a second focus-loss emits no releases.
        let events = translate(&mut backend, &[InputEvent::FocusLost]);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], egui::Event::WindowFocused(false)));
    }

    #[test]
    fn pointer_leave_releases_held_buttons_then_reports_gone() {
        let mut backend = EguiBackend::new();
        translate(
            &mut backend,
            &[InputEvent::PointerPressed(PointerButton::Primary)],
        );
        let events = translate(&mut backend, &[InputEvent::PointerLeft]);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            egui::Event::PointerButton { pressed: false, .. }
        ));
        assert!(matches!(events[1], egui::Event::PointerGone));
    }

    #[test]
    fn widgets_produce_paint_and_a_font_upload() {
        let mut backend = EguiBackend::new();
        // Frame 1 only measures the window (shapes are `Noop`s); the font
        // atlas upload is the observable work. Frame 2 paints real shapes.
        let first = backend.run_frame(&frame_input(&[]), &mut |ui| {
            ui.label("score 1");
            ui.button(UiId::new("pause"), "Pause");
        });
        assert!(first.intents.is_empty());
        assert!(
            first
                .paint
                .textures
                .iter()
                .any(|op| matches!(op, canary_ui_core::UiTextureOp::Set { .. })),
            "first frame must upload the font atlas"
        );
        let output = backend.run_frame(&frame_input(&[]), &mut |ui| {
            ui.label("score 1");
            ui.button(UiId::new("pause"), "Pause");
        });
        assert!(output.intents.is_empty());
        assert!(!output.paint.batches.is_empty());
    }

    #[test]
    fn batch_uvs_pass_through_verbatim() {
        // A hand-built mesh with known UVs: conversion must not rescale.
        // (An adapter-side divide by the atlas size would double-normalize
        // `egui`'s already-normalized UVs into garbage — this pins the
        // passthrough. See `primitive_to_batch`.)
        let mesh = egui::epaint::Mesh {
            indices: vec![0, 1, 2],
            vertices: vec![
                egui::epaint::Vertex {
                    pos: egui::pos2(10.0, 20.0),
                    uv: egui::pos2(0.5, 0.25),
                    color: egui::Color32::WHITE,
                },
                egui::epaint::Vertex {
                    pos: egui::pos2(30.0, 20.0),
                    uv: egui::pos2(0.75, 0.25),
                    color: egui::Color32::WHITE,
                },
                egui::epaint::Vertex {
                    pos: egui::pos2(10.0, 40.0),
                    uv: egui::pos2(0.5, 0.5),
                    color: egui::Color32::WHITE,
                },
            ],
            texture_id: egui::TextureId::Managed(0),
        };
        let primitive = egui::ClippedPrimitive {
            clip_rect: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0)),
            primitive: egui::epaint::Primitive::Mesh(mesh),
        };
        let batch = primitive_to_batch(primitive).expect("a mesh converts");
        let uvs: Vec<[f32; 2]> = batch.triangles[0]
            .vertices
            .iter()
            .map(|vertex| vertex.uv)
            .collect();
        assert_eq!(uvs, vec![[0.5, 0.25], [0.75, 0.25], [0.5, 0.5]]);
    }

    #[test]
    fn live_batch_uvs_stay_in_unit_space() {
        let mut backend = EguiBackend::new();
        let build = &mut |ui: &mut dyn UiBuilder| {
            ui.label("score 1");
            ui.button(UiId::new("pause"), "Pause");
        };
        backend.run_frame(&frame_input(&[]), build);
        let output = backend.run_frame(&frame_input(&[]), build);
        assert!(!output.paint.batches.is_empty());
        // Smoke only: real glyph UVs must honor `UiVertex`'s 0.0-1.0
        // contract end to end.
        for batch in &output.paint.batches {
            for triangle in &batch.triangles {
                for vertex in &triangle.vertices {
                    for uv in vertex.uv {
                        assert!((0.0..=1.0).contains(&uv), "UV {uv} escapes unit space");
                    }
                }
            }
        }
    }

    #[test]
    fn pointer_far_from_ui_captures_nothing() {
        let mut backend = EguiBackend::new();
        let output = backend.run_frame(
            &frame_input(&[InputEvent::PointerMoved { x: 799.0, y: 599.0 }]),
            &mut |ui| {
                ui.label("score 1");
                ui.button(UiId::new("pause"), "Pause");
            },
        );
        assert!(!output.capture.any());
        assert!(output.intents.is_empty());
    }

    #[test]
    fn clicking_the_button_returns_its_intent() {
        let mut backend = EguiBackend::new();
        let mut build = |ui: &mut dyn UiBuilder| {
            ui.label("score 1");
            ui.button(UiId::new("pause"), "Pause");
        };
        // Self-calibrating click: scan a coarse grid for pointer capture
        // (hover over UI), then press and release there. Window placement
        // is `egui`-internal, so the test probes instead of assuming.
        // One warm-up frame first: the window only measures on frame 1 and
        // paints real shapes (with hover hit-testing) from frame 2 on.
        // Scan bottom-up: widget content always sits below the title bar,
        // and rapid title-bar presses collapse the window (`egui`
        // double-click), which would destroy the button before the scan
        // reaches it. Visiting content rows first clicks the button while
        // the window is still expanded; background presses are harmless.
        backend.run_frame(&frame_input(&[]), &mut build);
        let mut clicked = false;
        'scan: for row in (0..24).rev() {
            for col in 0..32 {
                let (x, y) = (col as f32 * 25.0 + 12.0, row as f32 * 25.0 + 12.0);
                let output = backend.run_frame(
                    &frame_input(&[InputEvent::PointerMoved { x, y }]),
                    &mut build,
                );
                if !output.capture.pointer {
                    continue;
                }
                for event in [
                    InputEvent::PointerPressed(PointerButton::Primary),
                    InputEvent::PointerReleased(PointerButton::Primary),
                ] {
                    let output = backend.run_frame(&frame_input(&[event]), &mut build);
                    if output.intents == vec![UiIntent::ButtonPressed(UiId::new("pause"))] {
                        clicked = true;
                        break 'scan;
                    }
                }
            }
        }
        assert!(clicked, "no grid point produced the button intent");
    }
}
