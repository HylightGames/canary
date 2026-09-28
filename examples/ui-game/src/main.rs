// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! `ui-game`: the `v0.0.13` first-game slice running in a real window —
//! mapped-action movement, a HUD (readout plus fire button) over the
//! scene, and frame-driver presentation, all on the public runtime
//! library.
//!
//! One outer frame, in roadmap §5 order: pump platform events, drive the
//! input frame (UI capture before gameplay mapping), run the simulation
//! pass, extract the HUD view, draw scene plus UI into the offscreen
//! target through the RHI, and blit-present. The scene draws first and the
//! UI paints over it in the same open pass (painter's algorithm, no
//! depth) — the UI reaches the screen through the same RHI path as the
//! scene, never a second graphics path.
//!
//! Two honest one-frame lags, both documented, neither a bug: the HUD
//! renders last frame's extracted state (the build pass runs inside the
//! input frame, before the pass), and the UI paints at logical resolution
//! scaled by the viewport — font glyphs rasterize at 1.0 pixels per point
//! (the adapter's tessellation constant), so a `scale_factor` above one
//! shows honest upscaling, not re-rasterization.
//!
//! Run until the window closes, or with a frame cap for scripted runs:
//!
//! ```sh
//! cargo run -p ui-game
//! cargo run -p ui-game -- 600
//! ```
//!
//! Move with WASD or arrows, fire with Space (gameplay edge) or the HUD
//! Fire button (UI intent — same `"fire"` boundary as the headless
//! harness). Close the window to exit early.

mod game;
mod hud;
mod scene;
mod shader;

use std::time::Duration;

use canary_ecs::World;
use canary_platform::{winit_backend::WinitWindow, Window, WindowDescriptor};
use canary_render::{
    ColorTargetDescriptor, CommandEncoder, FrameOutcome, PresentationError, RenderDevice,
    RenderPassDescriptor,
};
use canary_render_vulkan::{VulkanColorTarget, VulkanDevice, VulkanPresenter};
use canary_runtime::{FrameDriver, FrameParams, RuntimeBuilder};
use canary_scheduler::Schedule;
use canary_ui_core::{UiBuilder, UiPaint};
use canary_ui_egui::{EguiBackend, UiPaintStats, UiPainter, UiViewport, UI_PAINT_WGSL};

use game::{Position, Shots, STEP_MS};
use hud::{build_hud, HudState};
use scene::{player_ndc, ScenePipeline};

/// Default run length in outer frames (~10 s at 60 Hz FIFO presentation).
/// Progress log cadence in frames.
const LOG_EVERY: u32 = 60;
/// Scene clear color: dark blue the amber triangle reads against.
const SCENE_CLEAR: [f32; 4] = [0.05, 0.08, 0.16, 1.0];

/// Draws scene plus UI into `target` on `device`: open the pass (clear),
/// record the player triangle at this frame's position, apply the frame's
/// texture ops, paint the UI batches over the scene, submit.
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
    // has not finished reading is use-after-free — the same contract
    // `draw_baked_frame` documents, and the device is lost for real
    // (not just in theory) when a second frame reuses the target.
    let _player_held = scene.draw_player(device, &mut encoder, player_ndc);
    let drawn = painter.paint(device, &mut encoder, viewport, paint);
    encoder.end_render_pass();
    let stats = drawn.stats;
    device.submit_and_wait(encoder);
    // `_player_held` and `drawn` (the uploaded scene + batch buffers)
    // drop here, after submit.
    stats
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    canary_core::init_logging();
    // Optional frame cap from argv; absent means run until the window
    // closes. A cap keeps scripted proof runs bounded
    // (`cargo run -p ui-game -- 600`); interactive play passes none.
    let frames: Option<u32> = std::env::args().nth(1).and_then(|arg| arg.parse().ok());

    // --- Platform + presentation: a real window, a real swapchain.
    let mut window = WinitWindow::new(WindowDescriptor {
        title: String::from("Canary UI Game"),
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
    let mut world = World::new();
    let player = world.spawn();
    world.insert(player, Position { x: 0.0, y: 0.0 })?;
    world.insert(player, Shots(0))?;
    let (mapper, actions) = game::demo_input();
    let mut schedule = Schedule::new();
    game::register_move_player(&mut schedule, &actions);
    game::register_fire_on_edge(&mut schedule, actions.fire);
    game::register_apply_ui_intents(&mut schedule);
    let mut runtime = RuntimeBuilder::new().build(world)?;
    let mut driver = FrameDriver::new(EguiBackend::new(), mapper);

    // --- Pipelines: the scene's, plus the UI painter over its shader.
    let scene = ScenePipeline::new(presenter.device());
    let ui_vs = shader::compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Vertex);
    let ui_fs = shader::compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Fragment);
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
        // physical target by `pixels_per_point` (see module docs).
        let scale = window.scale_factor();
        let (extent_w, extent_h) = window.surface_extent().unwrap_or((target_w, target_h));
        // Pixel dims to `f32` at the sample boundary: the same documented
        // conversion the encoder itself uses for viewport math.
        let screen_w = extent_w as f32 / scale as f32;
        let screen_h = extent_h as f32 / scale as f32;

        let mut input = window.input_source();
        let mut run_schedule = |world: &mut World| schedule.run(world);
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
        // renders it next frame (see module docs).
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
        hud = HudState {
            shots: shots.0,
            player_x: position.x,
            player_y: position.y,
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
                player_ndc(position.x, position.y),
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
                            x = position.x,
                            shots = shots.0,
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
        shots = hud.shots,
        "ui-game run complete"
    );
    Ok(())
}
