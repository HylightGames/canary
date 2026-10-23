// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Frame-driver acceptance evidence: UI-first input routing, the optional
//! simulation pass, and per-frame resource publication across whole driven
//! frames — the R-38 tick/`sim_time` discipline, capture-respecting
//! gameplay mapping, and intent publication, all through the public
//! [`Runtime::drive_frame`](canary_runtime::Runtime::drive_frame) API.

use std::time::Duration;

use canary_ecs::World;
use canary_input::{
    ActionId, ActionSchema, Binding, InputMapper, KeyCode, PhysicalControl, SimulationInput,
};
use canary_platform::{HeadlessInput, InputEvent, InputSource, Key as PlatformKey};
use canary_runtime::{FrameDriver, FrameParams, RunContext, Runtime, RuntimeBuilder, SimPasses};
use canary_ui_core::{
    CaptureResult, NullBackend, UiBackend, UiBuilder, UiFrameInput, UiFrameOutput, UiId, UiIntent,
    UiIntents, UiPaint,
};

/// A backend with canned capture and intents for driver tests.
struct CannedBackend {
    capture: CaptureResult,
    intents: Vec<UiIntent>,
}

impl UiBackend for CannedBackend {
    fn run_frame(
        &mut self,
        _input: &UiFrameInput<'_>,
        _build: &mut dyn FnMut(&mut dyn UiBuilder),
    ) -> UiFrameOutput {
        UiFrameOutput {
            capture: self.capture,
            intents: std::mem::take(&mut self.intents),
            paint: UiPaint::default(),
        }
    }
}

fn jump_schema() -> (ActionSchema, ActionId) {
    let (schema, ids) = ActionSchema::declare(["jump"]).expect("schema declares");
    (schema, ids[0])
}

fn mapper_with_jump() -> (InputMapper, ActionId) {
    let (schema, jump) = jump_schema();
    let mut mapper = InputMapper::new(schema);
    mapper
        .add_binding(Binding::gameplay(
            PhysicalControl::Key(KeyCode::from_platform_key(PlatformKey::A)),
            jump,
        ))
        .expect("binding registers");
    (mapper, jump)
}

fn runtime() -> Runtime {
    RuntimeBuilder::new()
        .build(World::new())
        .expect("build must succeed")
}

fn params<'a>(
    input: &'a mut dyn InputSource,
    build: &'a mut dyn FnMut(&mut dyn UiBuilder),
    run_schedule: &'a mut dyn FnMut(&mut World),
    sim_passes: SimPasses,
) -> FrameParams<'a> {
    FrameParams {
        input,
        screen_width_px: 320.0,
        screen_height_px: 240.0,
        focused: true,
        frame_dt: Duration::from_millis(16),
        build,
        sim_passes,
        run_schedule,
    }
}

fn noop_build(_: &mut dyn UiBuilder) {}
fn noop_schedule(_: &mut World) {}

#[test]
fn event_only_frames_advance_neither_tick_nor_sim_time() {
    let mut runtime = runtime();
    let baseline = runtime.world().expect("world is owned").change_tick();
    let mut driver = FrameDriver::new(NullBackend, mapper_with_jump().0);
    let mut input = HeadlessInput::new();
    let mut build = noop_build;
    let mut schedule = noop_schedule;

    for _ in 0..2 {
        let driven = runtime.drive_frame(
            &mut driver,
            params(&mut input, &mut build, &mut schedule, SimPasses::NONE),
        );
        assert!(!driven.sim_ran);
        assert!(driven.paint.is_empty());
    }

    let world = runtime.world().expect("world is owned");
    assert_eq!(world.change_tick(), baseline);
    let context = runtime.run_context().expect("frames write RunContext");
    assert_eq!(context.frame_index, 2);
    assert_eq!(context.tick, baseline);
    assert_eq!(context.sim_time, Duration::ZERO);
    assert_eq!(context.sim_step, Duration::ZERO);
    let snapshot = world
        .resource::<SimulationInput>()
        .expect("driver publishes the snapshot");
    assert_eq!(snapshot.frame_index, 2);
    assert_eq!(snapshot.tick, None);
}

#[test]
fn sim_frames_advance_tick_and_sim_time() {
    let mut runtime = runtime();
    let baseline = runtime.world().expect("world is owned").change_tick();
    let mut driver = FrameDriver::new(NullBackend, mapper_with_jump().0);
    let mut input = HeadlessInput::new();
    let mut build = noop_build;
    let mut schedule = noop_schedule;
    let step = Duration::from_millis(16);

    for _ in 0..2 {
        let driven = runtime.drive_frame(
            &mut driver,
            params(
                &mut input,
                &mut build,
                &mut schedule,
                SimPasses::fixed(step, 1),
            ),
        );
        assert!(driven.sim_ran);
    }

    let world = runtime.world().expect("world is owned");
    assert_ne!(world.change_tick(), baseline);
    let context = runtime.run_context().expect("frames write RunContext");
    assert_eq!(context.frame_index, 2);
    assert_eq!(context.tick, world.change_tick());
    assert_eq!(context.sim_time, Duration::from_millis(32));
    assert_eq!(context.sim_step, step);
    let snapshot = world
        .resource::<SimulationInput>()
        .expect("driver publishes the snapshot");
    assert_eq!(snapshot.frame_index, 2);
    assert_eq!(snapshot.tick, Some(world.change_tick()));
}

#[test]
fn mixed_frames_accumulate_only_sim_steps() {
    let mut runtime = runtime();
    let mut driver = FrameDriver::new(NullBackend, mapper_with_jump().0);
    let mut input = HeadlessInput::new();
    let mut build = noop_build;
    let mut schedule = noop_schedule;

    runtime.drive_frame(
        &mut driver,
        params(&mut input, &mut build, &mut schedule, SimPasses::NONE),
    );
    runtime.drive_frame(
        &mut driver,
        params(
            &mut input,
            &mut build,
            &mut schedule,
            SimPasses::fixed(Duration::from_millis(10), 1),
        ),
    );
    runtime.drive_frame(
        &mut driver,
        params(&mut input, &mut build, &mut schedule, SimPasses::NONE),
    );

    let context = runtime.run_context().expect("frames write RunContext");
    assert_eq!(context.frame_index, 3);
    assert_eq!(context.sim_time, Duration::from_millis(10));
    assert_eq!(context.sim_step, Duration::ZERO);
}

#[test]
fn pressed_key_fires_bound_action_with_frame_and_tick() {
    let mut runtime = runtime();
    let (mapper, jump) = mapper_with_jump();
    let mut driver = FrameDriver::new(NullBackend, mapper);
    let mut input = HeadlessInput::new();
    input.inject(InputEvent::KeyPressed(PlatformKey::A));
    let mut build = noop_build;
    let mut schedule_ran = false;
    let mut schedule = |_: &mut World| schedule_ran = true;

    runtime.drive_frame(
        &mut driver,
        params(
            &mut input,
            &mut build,
            &mut schedule,
            SimPasses::fixed(Duration::from_millis(16), 1),
        ),
    );

    assert!(schedule_ran, "a sim frame must run the schedule");
    let world = runtime.world().expect("world is owned");
    let snapshot = world
        .resource::<SimulationInput>()
        .expect("driver publishes the snapshot");
    assert!(snapshot.is_down(jump));
    assert!(snapshot.was_pressed(jump));
    assert_eq!(snapshot.frame_index, 1);
    assert_eq!(snapshot.tick, Some(world.change_tick()));
}

#[test]
fn release_edge_visible_on_next_sim_frame() {
    let mut runtime = runtime();
    let (mapper, jump) = mapper_with_jump();
    let mut driver = FrameDriver::new(NullBackend, mapper);
    let mut input = HeadlessInput::new();
    let mut build = noop_build;
    let mut schedule = noop_schedule;
    let step = SimPasses::fixed(Duration::from_millis(16), 1);

    input.inject(InputEvent::KeyPressed(PlatformKey::A));
    runtime.drive_frame(
        &mut driver,
        params(&mut input, &mut build, &mut schedule, step),
    );
    input.inject(InputEvent::KeyReleased(PlatformKey::A));
    runtime.drive_frame(
        &mut driver,
        params(&mut input, &mut build, &mut schedule, step),
    );

    let world = runtime.world().expect("world is owned");
    let snapshot = world
        .resource::<SimulationInput>()
        .expect("driver publishes the snapshot");
    assert!(!snapshot.is_down(jump));
    assert!(snapshot.was_released(jump));
    assert_eq!(snapshot.frame_index, 2);
}

#[test]
fn captured_press_does_not_reach_gameplay() {
    let mut runtime = runtime();
    let (mapper, jump) = mapper_with_jump();
    let mut driver = FrameDriver::new(
        CannedBackend {
            capture: CaptureResult::all(),
            intents: Vec::new(),
        },
        mapper,
    );
    let mut input = HeadlessInput::new();
    input.inject(InputEvent::KeyPressed(PlatformKey::A));
    let mut build = noop_build;
    let mut schedule = noop_schedule;

    runtime.drive_frame(
        &mut driver,
        params(
            &mut input,
            &mut build,
            &mut schedule,
            SimPasses::fixed(Duration::from_millis(16), 1),
        ),
    );

    let world = runtime.world().expect("world is owned");
    let snapshot = world
        .resource::<SimulationInput>()
        .expect("driver publishes the snapshot");
    assert!(!snapshot.is_down(jump));
}

#[test]
fn intents_publish_as_resource_in_order() {
    let mut runtime = runtime();
    let mut driver = FrameDriver::new(
        CannedBackend {
            capture: CaptureResult::none(),
            intents: vec![
                UiIntent::ButtonPressed(UiId::new("a")),
                UiIntent::ButtonPressed(UiId::new("b")),
            ],
        },
        mapper_with_jump().0,
    );
    let mut input = HeadlessInput::new();
    let mut build = noop_build;
    let mut schedule = noop_schedule;

    let driven = runtime.drive_frame(
        &mut driver,
        params(
            &mut input,
            &mut build,
            &mut schedule,
            SimPasses::fixed(Duration::from_millis(16), 1),
        ),
    );

    assert!(driven.sim_ran);
    let world = runtime.world().expect("world is owned");
    let intents = world
        .resource::<UiIntents>()
        .expect("driver publishes intents");
    assert_eq!(
        intents.intents,
        vec![
            UiIntent::ButtonPressed(UiId::new("a")),
            UiIntent::ButtonPressed(UiId::new("b")),
        ]
    );
}

#[test]
fn four_passes_advance_tick_monotonically_and_accumulate_sim_time() {
    let mut runtime = runtime();
    let mut driver = FrameDriver::new(NullBackend, mapper_with_jump().0);
    let mut input = HeadlessInput::new();
    let mut build = noop_build;
    let step = Duration::from_millis(16);
    let mut observed_ticks = Vec::new();
    let mut observed_times = Vec::new();
    let mut schedule = |world: &mut World| {
        let context = world
            .resource::<RunContext>()
            .expect("driver stamps RunContext before the schedule runs");
        observed_ticks.push(context.tick);
        observed_times.push(context.sim_time);
    };

    let driven = runtime.drive_frame(
        &mut driver,
        params(
            &mut input,
            &mut build,
            &mut schedule,
            SimPasses::fixed(step, 4),
        ),
    );

    assert!(driven.sim_ran);
    assert_eq!(
        observed_ticks.len(),
        4,
        "four passes run the schedule four times"
    );
    for window in observed_ticks.windows(2) {
        assert_ne!(window[0], window[1], "each pass advances to a new tick");
    }
    assert_eq!(
        observed_times,
        vec![
            step,
            step.checked_mul(2).expect("test step scales"),
            step.checked_mul(3).expect("test step scales"),
            step.checked_mul(4).expect("test step scales"),
        ]
    );
    let context = runtime.run_context().expect("frames write RunContext");
    assert_eq!(
        context.sim_time,
        step.checked_mul(4).expect("test step scales"),
        "sim_time equals step times pass count"
    );
    assert_eq!(context.tick, observed_ticks[3]);
}

#[test]
fn catch_up_passes_see_held_but_not_edges_or_intents() {
    let mut runtime = runtime();
    let (mapper, jump) = mapper_with_jump();
    let mut driver = FrameDriver::new(
        CannedBackend {
            capture: CaptureResult::none(),
            intents: vec![UiIntent::ButtonPressed(UiId::new("reset"))],
        },
        mapper,
    );
    let mut input = HeadlessInput::new();
    input.inject(InputEvent::KeyPressed(PlatformKey::A));
    let mut build = noop_build;
    let step = Duration::from_millis(16);
    let mut pressed_per_pass = Vec::new();
    let mut held_per_pass = Vec::new();
    let mut intents_per_pass = Vec::new();
    let mut schedule = |world: &mut World| {
        let snapshot = world
            .resource::<SimulationInput>()
            .expect("driver publishes the snapshot before the schedule runs");
        pressed_per_pass.push(snapshot.was_pressed(jump));
        held_per_pass.push(snapshot.is_down(jump));
        let intents = world
            .resource::<UiIntents>()
            .expect("driver publishes intents before the schedule runs");
        intents_per_pass.push(intents.intents.len());
    };

    runtime.drive_frame(
        &mut driver,
        params(
            &mut input,
            &mut build,
            &mut schedule,
            SimPasses::fixed(step, 4),
        ),
    );

    assert_eq!(pressed_per_pass, vec![true, false, false, false]);
    assert_eq!(held_per_pass, vec![true, true, true, true]);
    assert_eq!(intents_per_pass, vec![1, 0, 0, 0]);
}

#[test]
fn sim_passes_fixed_normalizes_zero_to_event_only() {
    let step = Duration::from_millis(16);
    assert_eq!(SimPasses::fixed(step, 0), SimPasses::NONE);
    assert!(SimPasses::fixed(step, 0).is_event_only());
    assert_eq!(SimPasses::NONE.passes(), 0);
    assert_eq!(SimPasses::NONE.step(), None);
    assert_eq!(SimPasses::fixed(step, 2).passes(), 2);
    assert_eq!(SimPasses::fixed(step, 2).step(), Some(step));
}
