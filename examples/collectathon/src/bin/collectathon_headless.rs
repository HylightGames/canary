//! `collectathon_headless`: the deterministic headless twin — same
//! schema, bindings, and systems as the windowed game, driven with
//! scripted input for CI-friendly integration checks.
//!
//! The script holds left from frame 0 (driving toward the authored shard
//! at `(-60, 20)`), taps collect on frame 33 (the pulse edge gathers it
//! at extended range), and releases left on frame 60. Ninety frames later
//! the run prints the score and exits 0. Audio runs decode-only
//! ([`RodioBackend::default`]): the trigger path actuates voices against
//! the headless backend, so collection still flips voice state without a
//! sound card.
//!
//! ```sh
//! cargo run -p collectathon --bin collectathon_headless
//! ```

use std::time::Duration;

use canary_audio::RodioBackend;
use canary_ecs::World;
use canary_physics::FrameDelta;
use canary_platform::{
    HeadlessInput, HeadlessWindow, InputEvent, InputSource, Key as PlatformKey, Window,
    WindowDescriptor,
};
use canary_runtime::{AuthoredSpawner, FrameDriver, FrameParams, RuntimeBuilder};
use canary_scheduler::Schedule;
use canary_ui_core::{NullBackend, UiBuilder};
use collectathon::assets::{self, ROOM_FILE};
use collectathon::game::{self, SimHandles, STEP_MS};
use collectathon::state::{CollectathonDecoder, GameStats};

/// Scripted run length in outer frames.
const FRAMES: u32 = 90;

/// Scripted injections per frame: hold left from frame 0, collect edge on
/// frame 33 (released 34), release left on frame 60.
fn inject_scripted_frame(input: &mut HeadlessInput, frame: u32) {
    match frame {
        0 => input.inject(InputEvent::KeyPressed(PlatformKey::A)),
        33 => input.inject(InputEvent::KeyPressed(PlatformKey::Space)),
        34 => input.inject(InputEvent::KeyReleased(PlatformKey::Space)),
        60 => input.inject(InputEvent::KeyReleased(PlatformKey::A)),
        _ => {}
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("collectathon_headless: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    canary_core::init_logging();

    // --- Platform abstraction: the trait boundary, headless by harness
    // choice — real `winit`-backed windowing lives behind
    // `canary-platform`'s `winit-backend` feature and is exercised by
    // the windowed game binary.
    let mut window = HeadlessWindow::new(WindowDescriptor::default());
    let mut input = HeadlessInput::new();
    window.poll_events();
    // `InputSource::poll` is infallible: it drains the queued events
    // into a `Vec` (never a `Result`), so there is no error to handle.
    // The queue is empty this early in startup, so the drained batch is
    // deliberately discarded.
    let _drained: Vec<InputEvent> = input.poll();

    // --- Game state, input profile, and schedule, all on the public
    // runtime library: the room spawns through the authored seam, then
    // the runtime owns the world for the whole run. Registration order
    // matches the windowed game (physics, propagation, gameplay, audio
    // trigger); the audio backend is the decode-only default, ensured
    // by the trigger system itself.
    let mut world = World::new();
    game::register_game_components(&mut world)?;
    let game_assets = assets::load_game_assets()?;
    let room = assets::load_room(&assets::asset_dir().join(ROOM_FILE))?;
    let decoder = CollectathonDecoder;
    let spawner = AuthoredSpawner::new(&decoder, &assets::resolve_asset);
    let report = spawner.spawn(&mut world, &room)?;
    game::attach_simulation_components(
        &mut world,
        &report,
        &SimHandles {
            pickup_sound: game_assets.pickup_sound,
        },
    )?;
    world.insert_resource(game_assets.sounds);
    let (mapper, actions) = collectathon::declare_input();
    let mut schedule = Schedule::new();
    canary_physics::register_physics_step(&mut schedule);
    canary_transform::register_transform_propagation(&mut schedule);
    game::register_gameplay(&mut schedule, &actions);
    canary_audio::register_audio_trigger::<RodioBackend>(&mut schedule);
    let mut runtime = RuntimeBuilder::new().build(world)?;
    let mut driver = FrameDriver::new(NullBackend, mapper);

    // --- Scripted deterministic run: one sim pass per outer frame.
    let step = Duration::from_millis(STEP_MS);
    let mut build = |_: &mut dyn UiBuilder| {};
    for frame in 0..FRAMES {
        inject_scripted_frame(&mut input, frame);
        // Same `FrameDelta` bridge as the windowed game: the physics
        // step reads the resource, the driver stamps `RunContext` only.
        let mut run_schedule = |world: &mut World| {
            world.insert_resource(FrameDelta::new(step));
            schedule.run(world);
        };
        let driven = runtime.drive_frame(
            &mut driver,
            FrameParams {
                input: &mut input,
                screen_width_px: 320.0,
                screen_height_px: 240.0,
                focused: true,
                frame_dt: step,
                build: &mut build,
                sim_step: Some(step),
                run_schedule: &mut run_schedule,
            },
        );
        debug_assert!(driven.sim_ran);
    }

    let world = runtime.world().expect("world is owned");
    let score = world
        .query::<collectathon::Score>()
        .next()
        .map(|(_, score)| score.points)
        .expect("score exists");
    let stats = world
        .resource::<GameStats>()
        .expect("setup publishes run stats");
    let player = world
        .query::<collectathon::Player>()
        .next()
        .map(|(_, player)| (player.x, player.y))
        .expect("player exists");
    println!(
        "collectathon_headless: score {score} collected {}/{} player ({:.1}, {:.1}) after {FRAMES} frames",
        stats.collected, stats.goal, player.0, player.1
    );
    Ok(())
}
