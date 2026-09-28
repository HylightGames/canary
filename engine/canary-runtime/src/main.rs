// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine headless consumer harness.
//!
//! Runs the deterministic movement/fire demo on the public runtime
//! library ([`Runtime::drive_frame`]): one input frame drains headless
//! platform events, routes UI-before-gameplay with a [`NullBackend`]
//! (no widgets headless, so nothing is ever captured), publishes the
//! [`SimulationInput`] snapshot, stamps the tick, and runs the scheduled
//! simulation pass — exactly the §5 order the windowed game sample reuses
//! with a real backend. The scripted injections below are fixed, so this
//! run is deterministic: thrust held, fire edges on key and pointer,
//! focus loss clearing a held key, then a stray post-focus release.

use std::time::Duration;

use canary_ecs::World;
use canary_input::{
    ActionId, ActionSchema, Binding, InputMapper, KeyCode, PhysicalControl,
    PointerButton as InputPointerButton, SimulationInput,
};
use canary_platform::{
    HeadlessInput, HeadlessWindow, InputEvent, InputSource, Key as PlatformKey,
    PointerButton as PlatformPointerButton, Window, WindowDescriptor,
};
use canary_plugin_api::NativePluginLoader;
use canary_runtime::{FrameDriver, FrameParams, RunContext, RuntimeBuilder};
use canary_scheduler::{Schedule, SystemAccess};
use canary_ui_core::{NullBackend, UiBuilder, UiId, UiIntent, UiIntents};

/// Fixed simulation step: one pass per outer frame, no fixed-step runner
/// (deferred past `.13` by the roadmap).
const STEP: Duration = Duration::from_millis(16);
/// Player speed in logical pixels per second.
const SPEED_PX_PER_SEC: f32 = 120.0;
/// Scripted run length in outer frames.
const FRAMES: u32 = 60;
/// The UI button id the windowed sample will use; the intents stage below
/// already honors it so the headless and windowed consumers share the
/// simulation boundary contract.
const FIRE_BUTTON: UiId = UiId::new("fire");

/// Demo player position in logical pixels.
#[derive(Debug, Clone, Copy)]
struct Position {
    x: f32,
    y: f32,
}

/// Demo shot counter: edge-triggered, one per fire press.
#[derive(Debug, Clone, Copy)]
struct Shots(u32);

/// The demo's digital actions, in schema-declaration order.
struct Actions {
    up: ActionId,
    down: ActionId,
    left: ActionId,
    right: ActionId,
    fire: ActionId,
}

/// Declares the demo schema and binds both WASD and arrows to movement
/// (multiple bindings per action) plus Space and pointer-primary to fire.
fn demo_input() -> (InputMapper, Actions) {
    let (schema, ids) =
        ActionSchema::declare(["up", "down", "left", "right", "fire"]).expect("schema declares");
    let mut mapper = InputMapper::new(schema);
    let mut bind = |key: PlatformKey, action: ActionId| {
        mapper
            .add_binding(Binding::gameplay(
                PhysicalControl::Key(KeyCode::from_platform_key(key)),
                action,
            ))
            .expect("movement binding registers");
    };
    bind(PlatformKey::W, ids[0]);
    bind(PlatformKey::ArrowUp, ids[0]);
    bind(PlatformKey::S, ids[1]);
    bind(PlatformKey::ArrowDown, ids[1]);
    bind(PlatformKey::A, ids[2]);
    bind(PlatformKey::ArrowLeft, ids[2]);
    bind(PlatformKey::D, ids[3]);
    bind(PlatformKey::ArrowRight, ids[3]);
    bind(PlatformKey::Space, ids[4]);
    mapper
        .add_binding(Binding::gameplay(
            PhysicalControl::Pointer(InputPointerButton::Primary),
            ids[4],
        ))
        .expect("pointer fire binding registers");
    (
        mapper,
        Actions {
            up: ids[0],
            down: ids[1],
            left: ids[2],
            right: ids[3],
            fire: ids[4],
        },
    )
}

/// Integrates held movement actions at the pass step. Held `down` moves
/// every pass; nothing latches inside the system.
fn register_move_player(schedule: &mut Schedule, actions: &Actions) {
    let (up, down, left, right) = (actions.up, actions.down, actions.left, actions.right);
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .reads_resource::<RunContext>()
            .writes::<Position>(),
        move |world: &mut World| {
            let snapshot = world
                .resource::<SimulationInput>()
                .expect("driver publishes the snapshot before the schedule runs");
            let (dx, dy) = (
                f32::from(snapshot.is_down(right)) - f32::from(snapshot.is_down(left)),
                f32::from(snapshot.is_down(down)) - f32::from(snapshot.is_down(up)),
            );
            if dx == 0.0 && dy == 0.0 {
                return;
            }
            let context = world
                .resource::<RunContext>()
                .expect("driver stamps RunContext before the schedule runs");
            let step = SPEED_PX_PER_SEC * context.sim_step.as_secs_f32();
            let entities: Vec<_> = world
                .query::<Position>()
                .map(|(entity, _)| entity)
                .collect();
            for entity in entities {
                if let Some(position) = world.get_mut::<Position>(entity) {
                    position.x += dx * step;
                    position.y += dy * step;
                }
            }
        },
    );
}

/// Fires once per fire press edge, however long the control stays held.
fn register_fire_on_edge(schedule: &mut Schedule, fire: ActionId) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .writes::<Shots>(),
        move |world: &mut World| {
            if !world
                .resource::<SimulationInput>()
                .expect("driver publishes the snapshot before the schedule runs")
                .was_pressed(fire)
            {
                return;
            }
            let entities: Vec<_> = world.query::<Shots>().map(|(entity, _)| entity).collect();
            for entity in entities {
                if let Some(shots) = world.get_mut::<Shots>(entity) {
                    shots.0 += 1;
                }
            }
        },
    );
}

/// Applies UI-originated intents at the declared simulation boundary: a
/// widget callback never touches the world; the intent lands here, in the
/// pass. Headless runs a [`NullBackend`] so this stage idles — the
/// windowed sample feeds it real button presses.
fn register_apply_ui_intents(schedule: &mut Schedule) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<UiIntents>()
            .writes::<Shots>(),
        |world: &mut World| {
            let fired = world
                .resource::<UiIntents>()
                .expect("driver publishes intents before the schedule runs")
                .intents
                .contains(&UiIntent::ButtonPressed(FIRE_BUTTON));
            if !fired {
                return;
            }
            let entities: Vec<_> = world.query::<Shots>().map(|(entity, _)| entity).collect();
            for entity in entities {
                if let Some(shots) = world.get_mut::<Shots>(entity) {
                    shots.0 += 1;
                }
            }
        },
    );
}

/// Scripted injections per frame: thrust held from frame 0, fire edges on
/// Space (30/31) and pointer (40/41), focus loss clearing the held thrust
/// at 45, and a stray post-focus thrust release at 50.
fn inject_scripted_frame(input: &mut HeadlessInput, frame: u32) {
    match frame {
        0 => input.inject(InputEvent::KeyPressed(PlatformKey::D)),
        30 => input.inject(InputEvent::KeyPressed(PlatformKey::Space)),
        31 => input.inject(InputEvent::KeyReleased(PlatformKey::Space)),
        40 => input.inject(InputEvent::PointerPressed(PlatformPointerButton::Primary)),
        41 => input.inject(InputEvent::PointerReleased(PlatformPointerButton::Primary)),
        45 => input.inject(InputEvent::FocusLost),
        50 => input.inject(InputEvent::KeyReleased(PlatformKey::D)),
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    canary_core::init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Canary Engine booting");

    // --- Platform abstraction: the trait boundary, headless by harness
    // choice (see docs/architecture/platform-abstraction.md) — real
    // `winit`-backed windowing lives behind `canary-platform`'s
    // `winit-backend` feature and is exercised by the windowed sample.
    let mut window = HeadlessWindow::new(WindowDescriptor::default());
    let mut input = HeadlessInput::new();
    window.poll_events();
    // `InputSource::poll` is infallible: it drains the queued events
    // into a `Vec` (never a `Result`), so there is no error to handle.
    // The queue is empty this early in startup, so the drained batch is
    // deliberately discarded.
    let _drained: Vec<InputEvent> = input.poll();
    tracing::info!(
        title = %window.descriptor().title,
        "platform layer initialized (headless by harness choice)"
    );

    // --- Game state, input profile, and schedule, all on the public
    // runtime library: the world is pre-populated, then owned by the
    // runtime for the whole run.
    let mut world = World::new();
    let player = world.spawn();
    world.insert(player, Position { x: 0.0, y: 0.0 })?;
    world.insert(player, Shots(0))?;
    let (mapper, actions) = demo_input();
    let mut schedule = Schedule::new();
    register_move_player(&mut schedule, &actions);
    register_fire_on_edge(&mut schedule, actions.fire);
    register_apply_ui_intents(&mut schedule);
    let mut runtime = RuntimeBuilder::new().build(world)?;
    let mut driver = FrameDriver::new(NullBackend, mapper);

    // --- Scripted deterministic run: one sim pass per outer frame.
    let mut build = |_: &mut dyn UiBuilder| {};
    for frame in 0..FRAMES {
        inject_scripted_frame(&mut input, frame);
        let mut run_schedule = |world: &mut World| schedule.run(world);
        let driven = runtime.drive_frame(
            &mut driver,
            FrameParams {
                input: &mut input,
                screen_width_px: 320.0,
                screen_height_px: 240.0,
                focused: true,
                frame_dt: STEP,
                build: &mut build,
                sim_step: Some(STEP),
                run_schedule: &mut run_schedule,
            },
        );
        debug_assert!(driven.sim_ran);
        if frame % 15 == 0 || frame == FRAMES - 1 {
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
            tracing::info!(
                frame,
                x = position.x,
                y = position.y,
                shots = shots.0,
                "headless frame"
            );
        }
    }
    let context = runtime.run_context().expect("frames write RunContext");
    tracing::info!(
        frames = FRAMES,
        tick = ?context.tick,
        sim_time_ms = context.sim_time.as_millis(),
        "headless run complete"
    );

    // --- Plugin loader: no plugins ship with this foundation, but prove
    // the native (Tier B) loader is constructible and that checking a
    // plugin directory doesn't panic. See
    // docs/architecture/plugin-system.md.
    let loader = NativePluginLoader::new();
    let plugin_dir = std::path::Path::new("plugins");
    let has_plugins = plugin_dir.is_dir()
        && plugin_dir
            .read_dir()
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
    if has_plugins {
        tracing::warn!(
            "found files under `plugins/`, but automatic directory loading isn't wired up yet \
             (see docs/roadmap/v0.0.1-roadmap.md) -- load them explicitly via NativePluginLoader"
        );
    } else {
        tracing::info!(
            dir = %plugin_dir.display(),
            "no plugins found (none ship with this foundation; the loader itself is exercised by canary-plugin-api's own tests)"
        );
    }
    let _ = loader;

    tracing::info!("Canary Engine shutting down cleanly");
    Ok(())
}
