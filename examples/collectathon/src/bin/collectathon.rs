//! `collectathon`: the windowed one-room game — physics-driven movement
//! on mapped actions, touch and collect-edge pickup collection with a
//! pickup voice, HUD score/goal over the scene, reset through the HUD
//! button or R.
//!
//! One outer frame, in roadmap §5 order: pump platform events, fold
//! measured wall time into bounded fixed sim ticks, drive the input frame
//! (UI capture before gameplay mapping), run the simulation ticks
//! (physics first, propagation, gameplay, audio trigger last), extract
//! the HUD view, bake the bridge scene at the presenter's aspect, draw
//! scene plus UI into the offscreen target through the RHI, and
//! blit-present. The scene draws first and the UI paints over it in the
//! same open pass (painter's algorithm, no depth).
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

use canary_assets::{AssetStore, Texture};
use canary_audio::{AudioConfig, RodioBackend};
use canary_ecs::World;
use canary_physics::FrameDelta;
use canary_platform::{winit_backend::WinitWindow, Window, WindowDescriptor};
use canary_render::{
    ColorTargetDescriptor, CommandEncoder, FrameOutcome, PresentationError, RenderDevice,
    RenderPassDescriptor,
};
use canary_render_ecs::{
    bake_scene_to_vertices_ortho, bake_textured_scene_to_vertices_ortho, extract_mesh_scene,
    extract_textured_scene, BakedFrame, BakedTexturedFrame,
};
use canary_render_vulkan::{VulkanColorTarget, VulkanDevice, VulkanPresenter};
use canary_runtime::{AuthoredSpawner, FrameDriver, FrameParams, RuntimeBuilder};
use canary_scheduler::Schedule;
use canary_ui_core::{UiBuilder, UiPaint};
use canary_ui_egui::{EguiBackend, UiPaintStats, UiPainter, UiViewport, UI_PAINT_WGSL};
use collectathon::assets::{self, ROOM_FILE};
use collectathon::game::{self, RenderHandles, SimHandles, STEP_MS};
use collectathon::hud::{build_hud, HudState};
use collectathon::scene::{collectathon_view, world_ndc, PickupDraw, ScenePipeline};
use collectathon::state::CollectathonDecoder;

/// Default run length in outer frames (~10 s at 60 Hz FIFO presentation).
/// Progress log cadence in frames.
const LOG_EVERY: u32 = 60;
/// Scene clear color: dark blue the amber player reads against.
const SCENE_CLEAR: [f32; 4] = [0.05, 0.08, 0.16, 1.0];

/// One frame's bridge-baked scene at the presenter's aspect.
///
/// `None` at the call site (no camera resolves for the target) means the
/// legacy hand-built scene draws instead — camera-less output stays
/// byte-identical.
struct BridgeScene {
    /// Mesh soup baked through the ortho camera at the target's aspect.
    soup: BakedFrame,
    /// Textured soup baked through the same camera and aspect.
    textured: BakedTexturedFrame,
    /// Cloned sprite image for the textured draw; `None` when the store
    /// no longer resolves the handle — the soup still draws, the textured
    /// quad skips rather than trapping the frame.
    sprite: Option<Texture>,
}

/// Draws scene plus UI into `target` on `device`: open the pass (clear),
/// record the bridge-baked scene (soup, then the sprite over it) or — when
/// no camera resolves — the legacy player quad and pickup diamonds,
/// apply the frame's texture ops, paint the UI batches over the scene,
/// submit.
///
/// Takes the presenter device as a parameter — never holds
/// [`VulkanPresenter::device`] across a present call (shared borrow versus
/// the presenter's `&mut` borrow), so the caller passes the reference down
/// and it expires when this returns.
/// One frame's scene inputs: the legacy fallback positions plus the
/// optional bridge bake (which wins when present).
#[derive(Clone, Copy)]
struct SceneDraw<'a> {
    /// Player position in NDC for the legacy fallback path.
    player_ndc: [f32; 2],
    /// Uncollected pickups for the legacy fallback path.
    pickups: &'a [PickupDraw],
    /// Bridge-baked soup plus sprite at the presenter's aspect; `None`
    /// draws the legacy scene instead.
    bridge: Option<&'a BridgeScene>,
}

fn draw_frame(
    device: &VulkanDevice,
    target: &VulkanColorTarget,
    scene: &ScenePipeline,
    painter: &mut UiPainter<VulkanDevice>,
    scene_draw: SceneDraw<'_>,
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
    // The bridge records soup first, then the sprite over it (the player's
    // flat quad shows through only where the texture skips); the legacy
    // path records only when no camera resolved.
    let _bridge_held = scene_draw.bridge.map(|baked| {
        let soup = scene.record_soup_frame(device, &mut encoder, &baked.soup);
        let textured = baked.sprite.as_ref().and_then(|sprite| {
            scene.record_textured_frame(device, &mut encoder, &baked.textured, sprite)
        });
        (soup, textured)
    });
    let _legacy_held = if _bridge_held.is_none() {
        Some(scene.draw_scene(
            device,
            &mut encoder,
            scene_draw.player_ndc,
            scene_draw.pickups,
        ))
    } else {
        None
    };
    let drawn = painter.paint(device, &mut encoder, viewport, paint);
    encoder.end_render_pass();
    let stats = drawn.stats;
    device.submit_and_wait(encoder);
    // `_bridge_held`, `_legacy_held`, and `drawn` (the uploaded scene +
    // batch buffers) drop here, after submit.
    stats
}

/// Builds the game world: registers the game schemas, loads assets and
/// the room, spawns through the authored seam, attaches the
/// simulation-owned components plus the file-loaded renderables, and
/// publishes the asset stores plus the audio backend as resources.
///
/// Returns the world with the render handles the frame loop's bridge bake
/// needs (the sprite handle for the textured draw): the handles are
/// `Copy`, so they survive the stores moving into the world.
fn build_world(
    audio_backend: RodioBackend,
) -> Result<(World, RenderHandles), Box<dyn std::error::Error>> {
    let mut world = World::new();
    game::register_game_components(&mut world)?;
    let game_assets = assets::load_game_assets()?;
    let pickup_sound = game_assets.pickup_sound;
    let room = assets::load_room(&assets::asset_dir().join(ROOM_FILE))?;
    let decoder = CollectathonDecoder;
    let spawner = AuthoredSpawner::new(&decoder, &assets::resolve_asset);
    let report = spawner.spawn(&mut world, &room)?;
    game::attach_simulation_components(&mut world, &report, &SimHandles { pickup_sound })?;
    let render = RenderHandles {
        player_mesh: game_assets.player_mesh,
        goal_mesh: game_assets.goal_mesh,
        sprite: game_assets.sprite,
    };
    game::attach_render_components(&mut world, &report, &render)?;
    world.insert_resource(game_assets.meshes);
    world.insert_resource(game_assets.textures);
    world.insert_resource(game_assets.sounds);
    world.insert_resource(AudioConfig::default());
    world.insert_resource(Mutex::new(audio_backend));
    Ok((world, render))
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
    let (world, render_handles) = build_world(audio_backend)?;
    let sprite_handle = render_handles.sprite;
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
    // Bounded fixed-step simulation over wall time: measured elapsed
    // accumulates into whole `STEP_MS` ticks, and each presentation frame
    // runs 0..=MAX_STEPS_PER_FRAME ticks, keeping the leftover. Capping
    // both the per-frame intake and the per-frame steps means a hitch
    // sheds debt instead of spiraling (no catch-up avalanche), and a fast
    // display yields event-only frames instead of a faster game. The
    // headless twin keeps its explicit one-step-per-frame script — proof
    // runs never consult this clock.
    const MAX_STEPS_PER_FRAME: u32 = 4;
    let mut accumulator = Duration::ZERO;
    let mut last_frame = std::time::Instant::now();
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
        // Measured wall time since the last presentation frame, clamped to
        // the per-frame step budget so a suspend/resume gap never enters
        // the accumulator as debt.
        let now = std::time::Instant::now();
        let frame_elapsed = now
            .duration_since(last_frame)
            .min(step * MAX_STEPS_PER_FRAME);
        last_frame = now;
        accumulator += frame_elapsed;
        let mut steps_this_frame = 0u32;
        while accumulator >= step && steps_this_frame < MAX_STEPS_PER_FRAME {
            accumulator -= step;
            steps_this_frame += 1;
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
        // is that subsystem: bridge each accumulated fixed sim step into
        // the resource, then run the schedule. Event-only frames (a fast
        // display outrunning the step) run zero passes.
        let mut run_schedule = |world: &mut World| {
            for _ in 0..steps_this_frame {
                world.insert_resource(FrameDelta::new(step));
                schedule.run(world);
            }
        };
        let mut build = |builder: &mut dyn UiBuilder| build_hud(builder, &hud);
        let driven = runtime.drive_frame(
            &mut driver,
            FrameParams {
                input: &mut input,
                screen_width_px: screen_w,
                screen_height_px: screen_h,
                focused: window.is_focused(),
                frame_dt: frame_elapsed,
                build: &mut build,
                sim_step: if steps_this_frame > 0 {
                    Some(step)
                } else {
                    None
                },
                run_schedule: &mut run_schedule,
            },
        );
        debug_assert_eq!(
            driven.sim_ran,
            steps_this_frame > 0,
            "event-only frames run no sim pass"
        );

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

        // Bridge bake at the presenter's aspect: propagation already ran
        // inside the schedule (physics first, propagation second), so the
        // globals extracted here are this frame's. Explicit extract plus
        // bake after `drive_frame` — the GPU never enters the schedule.
        // `target_h == 0` divides to a non-finite aspect, which
        // `collectathon_view` rejects, so the ratio needs no separate
        // guard: unusable targets fall back to the legacy scene.
        let aspect = target_w as f32 / target_h as f32;
        let bridge = collectathon_view(aspect).map(|view| {
            let mesh_items = extract_mesh_scene(world);
            let soup = BakedFrame {
                vertices: bake_scene_to_vertices_ortho(&mesh_items, view, aspect),
            };
            let textured_items = extract_textured_scene(world);
            let textured = BakedTexturedFrame {
                vertices: bake_textured_scene_to_vertices_ortho(&textured_items, view, aspect),
            };
            let sprite = world
                .resource::<AssetStore<Texture>>()
                .and_then(|store| store.get(sprite_handle).cloned());
            BridgeScene {
                soup,
                textured,
                sprite,
            }
        });

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
                SceneDraw {
                    player_ndc: world_ndc(player_x, player_y),
                    pickups: &pickups,
                    bridge: bridge.as_ref(),
                },
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
