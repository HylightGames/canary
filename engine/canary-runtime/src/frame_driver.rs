//! Frame driver: one outer frame through UI-first input, zero or more
//! simulation passes, and per-pass resource publication.
//!
//! [`Runtime::drive_frame`] owns the roadmap §5 order for the input +
//! simulation half of the frame: open the frame, drain platform events,
//! route UI-before-gameplay ([`drive_input_frame`]), then run each fixed
//! simulation pass the caller asks for. Per pass, in order, the driver
//! stamps the tick and `sim_time` ([`Runtime::begin_sim_pass`]), publishes
//! the tick-stamped snapshot and the UI intents, and runs the schedule.
//! Every schedule invocation therefore observes a tick and `sim_time` that
//! advanced for it.
//!
//! Per-frame input is consumed once: the routed pressed/released edges and
//! the UI intents are published on the first pass only; catch-up passes
//! publish the same held state with the edges cleared and empty intents,
//! so one physical press cannot fire repeatedly. Event-only frames (no
//! passes) advance neither the tick nor `sim_time` (R-38) and publish no
//! tick-stamped snapshot.

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

/// How many fixed-step simulation passes one outer frame runs.
///
/// One outer presentation frame may need zero or several fixed-step
/// simulation passes to keep up with wall time. [`Runtime::drive_frame`]
/// owns that loop: each pass gets its own tick advance, `sim_time` advance,
/// snapshot publication, and UI-intent delivery. Callers must **not** loop
/// the schedule inside [`FrameParams::run_schedule`]; they pass the required
/// pass count here instead.
///
/// The zero state ([`SimPasses::NONE`], also [`Default`]) is an
/// event/presentation-only frame. A nonzero state is built with
/// [`SimPasses::fixed`], which normalizes `passes == 0` to [`SimPasses::NONE`]:
/// "no step" and "zero passes" are the same state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SimPasses {
    step: Option<Duration>,
    passes: u32,
}

impl SimPasses {
    /// An event/presentation-only frame: no tick or `sim_time` advance, no
    /// tick-stamped snapshot, and no schedule invocation.
    pub const NONE: Self = Self {
        step: None,
        passes: 0,
    };

    /// `passes` fixed passes of `step` each, run in order. A zero count
    /// normalizes to [`SimPasses::NONE`] (event-only), so the constructor
    /// never produces an inconsistent step/count pair.
    pub const fn fixed(step: Duration, passes: u32) -> Self {
        if passes == 0 {
            Self::NONE
        } else {
            Self {
                step: Some(step),
                passes,
            }
        }
    }

    /// Number of simulation passes this frame runs; `0` is event-only.
    pub const fn passes(self) -> u32 {
        self.passes
    }

    /// The fixed step per pass; `None` on event-only frames.
    pub const fn step(self) -> Option<Duration> {
        self.step
    }

    /// Whether this frame runs no simulation pass.
    pub const fn is_event_only(self) -> bool {
        self.passes == 0
    }
}

/// Everything [`Runtime::drive_frame`] needs beyond the driver itself,
/// as one named value (not eight parameters): the event source, the
/// viewport/focus/clock for the UI frame, the game's widget build pass,
/// and the simulation pass count/step with its schedule runner.
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
    /// Zero passes run an event/presentation-only frame; nonzero runs that
    /// many fixed passes of the given step, each advancing tick and
    /// `sim_time` exactly once.
    pub sim_passes: SimPasses,
    /// Runs the scheduled world pass. Invoked exactly once per simulation
    /// pass, in order, only when [`FrameParams::sim_passes`] is nonzero.
    /// Each invocation observes the tick and `sim_time` advanced for that
    /// pass; only the first sees the routed pressed/released edges and the
    /// frame's UI intents.
    pub run_schedule: &'a mut dyn FnMut(&mut World),
}

/// What one driven frame hands back to the consumer: the UI paint for the
/// extract/submit step (intents and the snapshot travel as resources for
/// the schedule to read during the pass) plus whether any sim pass ran.
pub struct DrivenFrame {
    /// Tessellated widget output for the presenter; empty when the UI
    /// drew nothing, in which case the consumer skips UI submission.
    pub paint: UiPaint,
    /// `true` exactly when [`FrameParams::sim_passes`] was nonzero and the
    /// schedule ran at least once.
    pub sim_ran: bool,
}

impl Runtime {
    /// Drives one outer frame in roadmap §5 order: open the frame, drain
    /// platform events, route UI-before-gameplay, then run each requested
    /// fixed simulation pass. Per pass, in order: advance the tick and
    /// `sim_time`, publish the tick-stamped snapshot, publish the frame's
    /// UI intents, run the schedule.
    ///
    /// Publication order is load-bearing: the snapshot and the
    /// [`UiIntents`] resource land *before* `run_schedule` runs, so systems
    /// observe this pass's input, never the previous frame's. The first
    /// pass publishes the routed pressed/released edges and the frame's
    /// intents; later catch-up passes publish held state with edges cleared
    /// and empty intents, so a single press fires once, not once per pass.
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
        let passes = params.sim_passes.passes();
        let step = params.sim_passes.step();
        let sim_ran = passes > 0;
        let snapshot = output.input;
        let mut intents = Some(output.intents);
        if self.world.is_some() {
            if passes == 0 {
                // Event-only frame (R-38): publish the frame's input with no
                // tick stamp and leave the tick and `sim_time` untouched.
                let world = self.world.as_mut().expect("world checked above");
                snapshot.publish(world);
                world.insert_resource(UiIntents {
                    intents: intents.take().unwrap_or_default(),
                });
            } else {
                for pass in 0..passes {
                    if let Some(step) = step {
                        self.begin_sim_pass(step);
                    }
                    let world = self.world.as_mut().expect("world checked above");
                    // The routed edges belong to the frame, not to each
                    // catch-up pass: only pass 0 carries them.
                    let mut pass_snapshot = snapshot.clone();
                    if pass > 0 {
                        pass_snapshot.clear_edges();
                    }
                    if let Some(context) = world.resource::<RunContext>() {
                        pass_snapshot.stamp_tick(context.tick);
                    }
                    pass_snapshot.publish(world);
                    world.insert_resource(UiIntents {
                        intents: intents.take().unwrap_or_default(),
                    });
                    (params.run_schedule)(world);
                }
            }
        }
        DrivenFrame {
            paint: output.paint,
            sim_ran,
        }
    }
}
