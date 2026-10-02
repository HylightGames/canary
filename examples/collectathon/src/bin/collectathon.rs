//! `collectathon`: the windowed one-room game — physics-driven movement
//! on mapped actions, touch and collect-edge pickup collection with a
//! pickup voice, HUD score/goal over the scene, reset through the HUD
//! button or R.
//!
//! One outer frame, in roadmap §5 order: pump platform events, drive the
//! input frame (UI capture before gameplay mapping), run the simulation
//! pass (physics first, propagation, gameplay, audio trigger last),
//! extract the HUD view, draw scene plus UI into the offscreen target
//! through the RHI, and blit-present. The scene draws first and the UI
//! paints over it in the same open pass (painter's algorithm, no depth).
//!
//! Two honest one-frame lags, both documented, neither a bug: the HUD
//! renders last frame's extracted state (the build pass runs inside the
//! input frame, before the pass), and velocity input applies to the
//! solver tick after it is written (physics steps before the input system
//! writes).
//!
//! Run until the window closes, or with a frame cap for scripted runs:
//!
//! ```sh
//! cargo run -p collectathon --bin collectathon
//! cargo run -p collectathon --bin collectathon -- 600
//! ```
//!
//! Move with WASD or arrows, collect at range with Space (gameplay edge)
//! or primary pointer, reset the room with R or the HUD Reset button
//! (UI intent — same boundary the headless twin shares). Audio opens the
//! OS default device when one exists and degrades to the decode-only
//! headless backend otherwise — never a panic on a silent machine. Close
//! the window to exit early.

use std::sync::Mutex;
use std::time::Duration;

use canary_audio::{AudioConfig, RodioBackend};
use canary_ecs::World;
use canary_physics::FrameDelta;
use canary_platform::{winit_backend::WinitWindow, Window, WindowDescriptor};
use canary_render::{
    ColorTargetDescriptor, CommandEncoder, FrameOutcome, PresentationError, RenderDevice,
    RenderPassDescriptor,
};
use canary_render_vulkan::{VulkanColorTarget, VulkanDevice, VulkanPresenter};
use canary_runtime::{AuthoredSpawner, FrameDriver, FrameParams, RuntimeBuilder};
use canary_scheduler::Schedule;
use canary_ui_core::{UiBuilder, UiPaint};
use canary_ui_egui::{EguiBackend, UiPaintStats, UiPainter, UiViewport, UI_PAINT_WGSL};
use collectathon::assets::{self, ROOM_FILE};
use collectathon::game::{self, SimHandles, STEP_MS};
use collectathon::hud::{build_hud, HudState};
use collectathon::scene::{world_ndc, PickupDraw, ScenePipeline};
use collectathon::state::CollectathonDecoder;

/// Default run length in outer frames (~10 s at 60 Hz FIFO presentation).
/// Progress log cadence in frames.
const LOG_EVERY: u32 = 60;
/// Scene clear color: dark blue the amber player reads against.
const SCENE_CLEAR: [f32; 4] = [0.05, 0.08, 0.16, 1.0];

/// Draws scene plus UI into `target` on `device`: open the pass (clear),
/// record the player quad and pickup diamonds at this frame's positions,
/// apply the frame's texture ops, paint the UI batches over the scene,
/// submit.
///
/// Takes the presenter device as a parameter — never holds
/// [`VulkanPresenter::device`] across a present call (shared borrow versus
/// the presenter's `&mut` borrow), so the caller passes the reference down
/// and it expires when this returns.
fn draw_frame(
    device: &VulkanDevice,
    target: &VulkanColorTarget,
    scene: &ScenePipeline,
    painter: &mut UiPainter<VulkanDevice>,
    player_ndc: [f32; 2],
    pickups: &[PickupDraw],
    paint: &UiPaint,
    viewport: UiViewport,
) -> UiPaintStats {
    painter.apply_texture_ops(device, &paint.textures);
    let mut encoder = device.create_command_encoder();
    encoder.begin_render_pass(
        target,
        &RenderPassDescriptor {
            clear_color: SCENE_CLEAR,
        },
    );
    // Held across `submit_and_wait` below: destroying a buffer the GPU
    // has not finished reading is use-after-free — the device is lost for
    // real (not just in theory) when a second frame reuses the target.
    let _scene_held = scene.draw_scene(device, &mut encoder, player_ndc, pickups);
    let drawn = painter.paint(device, &mut encoder, viewport, paint);
    encoder.end_render_pass();
    let stats = drawn.stats;
    device.submit_and_wait(encoder);
    // `_scene_held` and `drawn` (the uploaded scene + batch buffers)
    // drop here, after submit.
    stats
}

/// Builds the game world: registers the game schemas, loads assets and
/// the room, spawns through the authored seam, attaches the
/// simulation-owned components, and publishes the asset stores plus the
/// audio backend as resources.
fn build_world(audio_backend: RodioBackend) -> Result<World, Box<dyn std::error::Error>> {
    let mut world = World::new();
    game::register_game_components(&mut world)?;
    let game_assets = assets::load_game_assets()?;
    let pickup_sound = game_assets.pickup_sound;
    let room = assets::load_room(&assets::asset_dir().join(ROOM_FILE))?;
    let decoder = CollectathonDecoder;
    let spawner = AuthoredSpawner::new(&decoder, &assets::resolve_asset);
    let report = spawner.spawn(&mut world, &room)?;
    game::attach_simulation_components(&mut world, &report, &SimHandles { pickup_sound })?;
    world.insert_resource(game_assets.meshes);
    world.insert_resource(game_assets.textures);
    world.insert_resource(game_assets.sounds);
    world.insert_resource(AudioConfig::default());
    world.insert_resource(Mutex::new(audio_backend));
    Ok(world)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    canary_core::init_logging();
    // Optional frame cap from argv; absent means run until the window
    // closes. A cap keeps scripted proof runs bounded
    // (`cargo run -p collectathon --bin collectathon -- 600`); interactive
    // play passes none.
    let frames: Option<u32> = std::env::args().nth(1).and_then(|arg| arg.parse().ok());

    // --- Audio first: the OS default device when one exists, the
    // decode-only headless backend otherwise. A silent machine (CI, a
    // server closet) is a supported configuration, not an error — the
    // game plays on mute and the trigger path still actuates voices.
    let audio_backend = match RodioBackend::try_new() {
        Ok(backend) => {
            tracing::info!("audio device opened (audible backend)");
            backend
        }
        Err(error) => {
            eprintln!("collectathon: no audio device ({error}); continuing headless (decode-only)");
            tracing::warn!(%error, "audio device unavailable, continuing headless");
            RodioBackend::headless()
        }
    };

    // --- Platform + presentation: a real window, a real swapchain.
    let mut window = WinitWindow::new(WindowDescriptor {
        title: String::from("Canary Collectathon"),
        width: 960,
        height: 600,
    })?;
    let mut presenter = VulkanPresenter::new(&window)?;
    let (mut target_w, mut target_h) = presenter.extent();
    let mut target: VulkanColorTarget =
        presenter
            .device()
            .create_color_target(&ColorTargetDescriptor {
                width: target_w,
                height: target_h,
            });
    // The offscreen target's live size, tracked beside the handle:
    // `VulkanColorTarget` exposes no public extent, and this loop is
    // the only code that ever re-creates it.
    let mut target_size = (target_w, target_h);

    // --- Game state, input profile, and schedule on the public runtime.
    // Registration order is the ordering mechanism (solo-write staging):
    // physics FIRST, propagation second, gameplay next, audio trigger
    // last — see `game::register_gameplay`.
    let world = build_world(audio_backend)?;
    let (mapper, actions) = collectathon::declare_input();
    let mut schedule = Schedule::new();
    canary_physics::register_physics_step(&mut schedule);
    canary_transform::register_transform_propagation(&mut schedule);
    game::register_gameplay(&mut schedule, &actions);
    canary_audio::register_audio_trigger::<RodioBackend>(&mut schedule);
    let mut runtime = RuntimeBuilder::new().build(world)?;
    let mut driver = FrameDriver::new(EguiBackend::new(), mapper);

    // --- Pipelines: the scene's, plus the UI painter over its shader.
    let scene = ScenePipeline::new(presenter.device());
    let ui_vs = collectathon::shader::compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Vertex);
    let ui_fs = collectathon::shader::compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Fragment);
    let mut painter = UiPainter::new(presenter.device(), &ui_vs, &ui_fs);

    // --- The frame loop.
    let step = Duration::from_millis(STEP_MS);
    let mut hud = HudState::default();
    let mut presented = 0u32;
    let mut skipped = 0u32;
    let mut frame: u32 = 0;
    loop {
        window.poll_events();
        if window.should_close() {
            tracing::info!(frame, "window close requested, exiting early");
            break;
        }
        if let Some(limit) = frames {
            if frame >= limit {
                break;
            }
        }
        // Logical viewport for the UI frame; the painter scales to the
        // physical target by `pixels_per_point` (see ui-game's module
        // docs for the upscaling caveat).
        let scale = window.scale_factor();
        let (extent_w, extent_h) = window.surface_extent().unwrap_or((target_w, target_h));
        // Pixel dims to `f32` at the sample boundary: the same documented
        // conversion the encoder itself uses for viewport math.
        let screen_w = extent_w as f32 / scale as f32;
        let screen_h = extent_h as f32 / scale as f32;

        let mut input = window.input_source();
        // The physics step reads `FrameDelta`, which no driver stamps —
        // the owning subsystem inserts it before every run. This closure
        // is that subsystem: bridge the fixed sim step into the
        // resource, then run the schedule.
        let mut run_schedule = |world: &mut World| {
            world.insert_resource(FrameDelta::new(step));
            schedule.run(world);
        };
        let mut build = |builder: &mut dyn UiBuilder| build_hud(builder, &hud);
        let driven = runtime.drive_frame(
            &mut driver,
            FrameParams {
                input: &mut input,
                screen_width_px: screen_w,
                screen_height_px: screen_h,
                focused: window.is_focused(),
                frame_dt: step,
                build: &mut build,
                sim_step: Some(step),
                run_schedule: &mut run_schedule,
            },
        );
        debug_assert!(driven.sim_ran, "this sample simulates every frame");

        // Extract this frame's view: the scene draws it now, the HUD
        // renders it next frame (see the hud module docs).
        let world = runtime.world().expect("world is owned");
        let (player_x, player_y) = world
            .query::<collectathon::Player>()
            .next()
            .map(|(_, player)| (player.x, player.y))
            .expect("player exists");
        let score = world
            .query::<collectathon::Score>()
            .next()
            .map(|(_, score)| score.points)
            .expect("score exists");
        let (collected, total) = world
            .resource::<collectathon::state::GameStats>()
            .map(|stats| (stats.collected, stats.goal))
            .expect("setup publishes run stats");
        let pickups: Vec<PickupDraw> = world
            .query::<collectathon::Pickup>()
            .filter_map(|(_, pickup)| {
                (!pickup.collected).then_some(PickupDraw {
                    x: pickup.x,
                    y: pickup.y,
                    is_goal: pickup.is_goal,
                })
            })
            .collect();
        hud = HudState {
            score,
            collected,
            total,
            goal_reached: total > 0 && collected >= total,
            player_x,
            player_y,
        };

        // Resize: the presenter's extent is authoritative for the
        // swapchain — the window manager may tile the window to a size
        // the request never named. Re-create the offscreen target at
        // the presenter's extent, and if a resize races between here
        // and the present, re-create from the mismatch report and
        // retry once — the presenter never scales silently.
        (target_w, target_h) = presenter.extent();
        if target_size != (target_w, target_h) {
            target_size = (target_w, target_h);
            target = presenter
                .device()
                .create_color_target(&ColorTargetDescriptor {
                    width: target_w,
                    height: target_h,
                });
            tracing::info!(
                width = target_w,
                height = target_h,
                "offscreen target re-created"
            );
        }

        let mut attempt = 0;
        loop {
            let viewport = UiViewport {
                target_width_px: target_w,
                target_height_px: target_h,
                pixels_per_point: scale as f32,
            };
            let stats = draw_frame(
                presenter.device(),
                &target,
                &scene,
                &mut painter,
                world_ndc(player_x, player_y),
                &pickups,
                &driven.paint,
                viewport,
            );
            match presenter.present_color_target(&window, &target) {
                Ok(FrameOutcome::Presented(report)) => {
                    presented += 1;
                    if frame % LOG_EVERY == 0 {
                        tracing::info!(
                            frame,
                            image = report.image_index,
                            x = player_x,
                            score,
                            ui_batches = stats.batches_drawn,
                            "presented frame"
                        );
                    }
                    break;
                }
                Ok(FrameOutcome::Skipped { status }) => {
                    skipped += 1;
                    tracing::debug!(frame, ?status, "frame skipped");
                    break;
                }
                Err(PresentationError::ContentExtentMismatch { swapchain, .. }) if attempt == 0 => {
                    attempt += 1;
                    (target_w, target_h) = swapchain;
                    target_size = swapchain;
                    target = presenter
                        .device()
                        .create_color_target(&ColorTargetDescriptor {
                            width: target_w,
                            height: target_h,
                        });
                    tracing::info!(
                        width = target_w,
                        height = target_h,
                        "offscreen target re-created after present race"
                    );
                }
                Err(other) => return Err(other.into()),
            }
        }
        frame += 1;
    }

    tracing::info!(
        frame,
        presented,
        skipped,
        x = hud.player_x,
        score = hud.score,
        "collectathon run complete"
    );
    Ok(())
}
