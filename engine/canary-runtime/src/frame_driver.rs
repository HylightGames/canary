//! Frame driver: one outer frame through UI-first input, an optional
//! simulation pass, and per-frame resource publication.
//!
//! [`Runtime::drive_frame`] owns the roadmap §5 order for the input +
//! simulation half of the frame: open the frame, drain platform events,
//! route UI-before-gameplay ([`drive_input_frame`]), publish the snapshot
//! and intents as resources, then — only when the caller supplies a
//! simulation step — stamp the tick, open the simulation pass
//! ([`Runtime::begin_sim_pass`]), and run the schedule. Event-only frames
//! (no step) advance neither the tick nor `sim_time` (R-38).

use std::time::Duration;

use canary_ecs::World;
use canary_input::InputMapper;
use canary_platform::InputSource;
use canary_ui_core::{UiBackend, UiBuilder, UiFrameInput, UiIntents, UiPaint};

use super::input_frame::drive_input_frame;
use super::{InputFrame, RunContext, Runtime};

/// Stateful per-run input owner: the UI backend plus the gameplay mapper.
///
/// Both hold frame-to-frame state (the backend's widget/input tracking,
/// the mapper's held-control latches), so one driver lives for the whole
/// run and [`Runtime::drive_frame`] borrows it mutably each frame.
pub struct FrameDriver<B: UiBackend> {
    backend: B,
    mapper: InputMapper,
}

impl<B: UiBackend> FrameDriver<B> {
    /// Takes ownership of the backend and the (binding-registered) mapper
    /// for one run. Register bindings on the mapper before constructing.
    pub fn new(backend: B, mapper: InputMapper) -> Self {
        Self { backend, mapper }
    }
}

/// Everything [`Runtime::drive_frame`] needs beyond the driver itself,
/// as one named value (not eight parameters): the event source, the
/// viewport/focus/clock for the UI frame, the game's widget build pass,
/// and the optional simulation step with its schedule runner.
pub struct FrameParams<'a> {
    /// Drained once per frame, poll order preserved.
    pub input: &'a mut dyn InputSource,
    /// Viewport width in logical pixels.
    pub screen_width_px: f32,
    /// Viewport height in logical pixels.
    pub screen_height_px: f32,
    /// Window focus for the UI frame (the driver owns this flag; the
    /// platform emits `FocusLost` only, never a regain event).
    pub focused: bool,
    /// Wall-clock outer-frame time; UI clock and `RunContext::frame_dt`.
    pub frame_dt: Duration,
    /// The game's widget build pass; runs inside the backend's frame.
    pub build: &'a mut dyn FnMut(&mut dyn UiBuilder),
    /// `Some` step runs the simulation pass (tick + `sim_time` advance,
    /// schedule runs); `None` is an event/presentation-only frame.
    pub sim_step: Option<Duration>,
    /// Runs the scheduled world pass; invoked at most once per frame,
    /// only when `sim_step` is `Some`.
    pub run_schedule: &'a mut dyn FnMut(&mut World),
}

/// What one driven frame hands back to the consumer: the UI paint for the
/// extract/submit step (intents and the snapshot travel as resources for
/// the schedule to read during the pass) plus whether a sim pass ran.
pub struct DrivenFrame {
    /// Tessellated widget output for the presenter; empty when the UI
    /// drew nothing, in which case the consumer skips UI submission.
    pub paint: UiPaint,
    /// `true` exactly when `sim_step` was `Some` and the schedule ran.
    pub sim_ran: bool,
}

impl Runtime {
    /// Drives one outer frame in roadmap §5 order: open the frame, drain
    /// platform events, route UI-before-gameplay, publish the snapshot
    /// and intents as resources, then — only for a `Some` step — stamp
    /// the tick, open the simulation pass, and run the schedule.
    ///
    /// Publication order is load-bearing: the snapshot (tick-stamped on
    /// sim frames, `None` otherwise) and the [`UiIntents`] resource land
    /// *before* `run_schedule` runs, so systems observe this frame's
    /// input during the pass, never the previous frame's.
    pub fn drive_frame<B: UiBackend>(
        &mut self,
        driver: &mut FrameDriver<B>,
        params: FrameParams<'_>,
    ) -> DrivenFrame {
        self.begin_frame(params.frame_dt);
        let frame_index = self
            .run_context()
            .map(|context| context.frame_index)
            .unwrap_or_default();
        let events = params.input.poll();
        let ui_input = UiFrameInput {
            events: &events,
            screen_width_px: params.screen_width_px,
            screen_height_px: params.screen_height_px,
            focused: params.focused,
            dt_seconds: params.frame_dt.as_secs_f64(),
        };
        let output = drive_input_frame(
            InputFrame {
                ui_input,
                build: params.build,
                frame_index,
            },
            &mut driver.backend,
            &mut driver.mapper,
        );
        let sim_ran = params.sim_step.is_some();
        if let Some(step) = params.sim_step {
            self.begin_sim_pass(step);
        }
        if let Some(world) = self.world.as_mut() {
            let mut snapshot = output.input;
            if sim_ran {
                if let Some(context) = world.resource::<RunContext>() {
                    snapshot.stamp_tick(context.tick);
                }
            }
            snapshot.publish(world);
            world.insert_resource(UiIntents {
                intents: output.intents,
            });
            if sim_ran {
                (params.run_schedule)(world);
            }
        }
        DrivenFrame {
            paint: output.paint,
            sim_ran,
        }
    }
}
