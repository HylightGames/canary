// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Headless-consumer proof: the same public [`Runtime::drive_frame`] path
//! the `canary-runtime` binary harness runs, exercised here with a real
//! [`Schedule`] (not a closure stub) so the movement/fire systems, the
//! published [`SimulationInput`] resource, and the R-38 event-only
//! discipline are locked by tests, not just by harness logs.

use std::time::Duration;

use canary_ecs::World;
use canary_input::{
    ActionId, ActionSchema, Binding, InputMapper, KeyCode, PhysicalControl, SimulationInput,
};
use canary_platform::{HeadlessInput, InputEvent, Key as PlatformKey};
use canary_runtime::{FrameDriver, FrameParams, RunContext, Runtime, RuntimeBuilder};
use canary_scheduler::{Schedule, SystemAccess};
use canary_ui_core::{NullBackend, UiBuilder};

use canary_platform::PointerButton as PlatformPointerButton;

const STEP: Duration = Duration::from_millis(16);
const SPEED_PX_PER_SEC: f32 = 120.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Position {
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Shots(u32);

struct Actions {
    thrust: ActionId,
    fire: ActionId,
}

fn demo_schema() -> (InputMapper, Actions) {
    let (schema, ids) = ActionSchema::declare(["thrust", "fire"]).expect("schema declares");
    let mut mapper = InputMapper::new(schema);
    mapper
        .add_binding(Binding::gameplay(
            PhysicalControl::Key(KeyCode::from_platform_key(PlatformKey::D)),
            ids[0],
        ))
        .expect("thrust binding registers");
    mapper
        .add_binding(Binding::gameplay(
            PhysicalControl::Key(KeyCode::from_platform_key(PlatformKey::Space)),
            ids[1],
        ))
        .expect("fire key binding registers");
    mapper
        .add_binding(Binding::gameplay(
            PhysicalControl::Pointer(canary_input::PointerButton::Primary),
            ids[1],
        ))
        .expect("fire pointer binding registers");
    (
        mapper,
        Actions {
            thrust: ids[0],
            fire: ids[1],
        },
    )
}

/// Movement reads this frame's snapshot plus the pass step and integrates
/// one axis. Edge-free by construction: held `down` moves every pass,
/// nothing latches across frames inside the system.
fn register_move_thrust(schedule: &mut Schedule, thrust: ActionId) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .reads_resource::<RunContext>()
            .writes::<Position>(),
        move |world: &mut World| {
            let (down, step_secs) = {
                let snapshot = world
                    .resource::<SimulationInput>()
                    .expect("driver publishes the snapshot before the schedule runs");
                let context = world
                    .resource::<RunContext>()
                    .expect("driver stamps RunContext before the schedule runs");
                (snapshot.is_down(thrust), context.sim_step.as_secs_f32())
            };
            if !down {
                return;
            }
            let dx = SPEED_PX_PER_SEC * step_secs;
            let entities: Vec<_> = world.query::<Position>().map(|(e, _)| e).collect();
            for entity in entities {
                if let Some(position) = world.get_mut::<Position>(entity) {
                    position.x += dx;
                }
            }
        },
    );
}

/// Firing is edge-triggered: exactly one shot per press no matter how
/// long the control stays held.
fn register_fire_on_edge(schedule: &mut Schedule, fire: ActionId) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .writes::<Shots>(),
        move |world: &mut World| {
            let pressed = world
                .resource::<SimulationInput>()
                .expect("driver publishes the snapshot before the schedule runs")
                .was_pressed(fire);
            if !pressed {
                return;
            }
            let entities: Vec<_> = world.query::<Shots>().map(|(e, _)| e).collect();
            for entity in entities {
                if let Some(shots) = world.get_mut::<Shots>(entity) {
                    shots.0 += 1;
                }
            }
        },
    );
}

fn headless_runtime_with_player() -> Runtime {
    let mut world = World::new();
    spawn_player(&mut world);
    RuntimeBuilder::new()
        .build(world)
        .expect("headless runtime builds")
}

/// One owned headless run: the runtime, its driver, the event source,
/// and the schedule behind a one-argument [`Harness::drive`] (a five-
/// argument free helper would trip the parameter-bloat smell for no
/// reason — the frame's inputs are one struct downstream anyway).
struct Harness {
    runtime: Runtime,
    driver: FrameDriver<NullBackend>,
    input: HeadlessInput,
    schedule: Schedule,
}

impl Harness {
    fn new(mapper: InputMapper, thrust: ActionId, fire: ActionId) -> Self {
        let mut schedule = Schedule::new();
        register_move_thrust(&mut schedule, thrust);
        register_fire_on_edge(&mut schedule, fire);
        Self {
            runtime: headless_runtime_with_player(),
            driver: FrameDriver::new(NullBackend, mapper),
            input: HeadlessInput::new(),
            schedule,
        }
    }

    fn drive(&mut self, sim_step: Option<Duration>) {
        let Self {
            runtime,
            driver,
            input,
            schedule,
        } = self;
        let mut build = |_: &mut dyn UiBuilder| {};
        let mut run_schedule = |world: &mut World| schedule.run(world);
        runtime.drive_frame(
            driver,
            FrameParams {
                input,
                screen_width_px: 320.0,
                screen_height_px: 240.0,
                focused: true,
                frame_dt: STEP,
                build: &mut build,
                sim_step,
                run_schedule: &mut run_schedule,
            },
        );
    }

    fn player_state(&self) -> (Position, Shots) {
        player_state(&self.runtime)
    }
}

fn spawn_player(world: &mut World) {
    let entity = world.spawn();
    world
        .insert(entity, Position { x: 0.0, y: 0.0 })
        .expect("position insert succeeds");
    world
        .insert(entity, Shots(0))
        .expect("shots insert succeeds");
}

fn player_state(runtime: &Runtime) -> (Position, Shots) {
    let world = runtime.world().expect("world is owned");
    let position = world
        .query::<Position>()
        .next()
        .map(|(_, position)| *position)
        .expect("player exists");
    let shots = world
        .query::<Shots>()
        .next()
        .map(|(_, shots)| *shots)
        .expect("shots exist");
    (position, shots)
}

#[test]
fn headless_frames_move_fire_and_settle_edges() {
    let (mapper, actions) = demo_schema();
    let mut harness = Harness::new(mapper, actions.thrust, actions.fire);

    // Hold thrust for ten passes: constant velocity, no latching.
    harness.input.inject(InputEvent::KeyPressed(PlatformKey::D));
    for _ in 0..10 {
        harness.drive(Some(STEP));
    }
    let (position, shots) = harness.player_state();
    assert!((position.x - 19.2).abs() < 1e-4, "x = {position:?}");
    assert_eq!(shots, Shots(0));

    // Release thrust, press fire: the player stops and exactly one shot
    // fires on the press edge.
    harness
        .input
        .inject(InputEvent::KeyReleased(PlatformKey::D));
    harness
        .input
        .inject(InputEvent::KeyPressed(PlatformKey::Space));
    harness.drive(Some(STEP));
    let (position, shots) = harness.player_state();
    assert!((position.x - 19.2).abs() < 1e-4, "x = {position:?}");
    assert_eq!(shots, Shots(1));

    // Release fire and coast: the edge settles, no repeat shots.
    harness
        .input
        .inject(InputEvent::KeyReleased(PlatformKey::Space));
    harness.drive(Some(STEP));
    let (position, shots) = harness.player_state();
    assert!((position.x - 19.2).abs() < 1e-4, "x = {position:?}");
    assert_eq!(shots, Shots(1));
    let world = harness.runtime.world().expect("world is owned");
    let snapshot = world
        .resource::<SimulationInput>()
        .expect("driver publishes the snapshot");
    assert!(!snapshot.was_pressed(actions.fire));
    assert!(snapshot.was_released(actions.fire));
    assert_eq!(snapshot.tick, Some(world.change_tick()));
}

#[test]
fn headless_pointer_press_fires_bound_action() {
    let (mapper, actions) = demo_schema();
    let mut harness = Harness::new(mapper, actions.thrust, actions.fire);

    harness
        .input
        .inject(InputEvent::PointerPressed(PlatformPointerButton::Primary));
    harness.drive(Some(STEP));
    let (_, shots) = harness.player_state();
    assert_eq!(shots, Shots(1));
}

#[test]
fn headless_event_only_frames_advance_neither_tick_nor_sim_time() {
    let (mapper, actions) = demo_schema();
    let mut harness = Harness::new(mapper, actions.thrust, actions.fire);

    harness.drive(None);
    harness.drive(None);

    let context = harness
        .runtime
        .run_context()
        .expect("frames write RunContext");
    assert_eq!(context.frame_index, 2);
    assert_eq!(context.sim_time, Duration::ZERO);
    let world = harness.runtime.world().expect("world is owned");
    assert_eq!(context.tick, world.change_tick());
    let snapshot = world
        .resource::<SimulationInput>()
        .expect("driver publishes the snapshot");
    assert_eq!(snapshot.tick, None);
    let (position, _) = harness.player_state();
    assert_eq!(position, Position { x: 0.0, y: 0.0 });
}
