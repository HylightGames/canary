// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The presentation seam's cleared-frame proof: a real `winit` window →
//! a real window surface → a real swapchain → acquired, cleared, and
//! presented frames — not "it compiled" or "it didn't panic."
//!
//! `#[ignore]`d by default: unlike the rest of this crate's tests,
//! these need *both* a live X11 display (`DISPLAY` pointing at a
//! running X server, real or `Xvfb`) *and* a Vulkan ICD with window
//! presentation (`mesa-vulkan-drivers`' lavapipe qualifies) — neither
//! is guaranteed in every environment `cargo test` runs in. Run
//! explicitly, with both available:
//!
//! ```sh
//! Xvfb :99 -screen 0 1280x1024x24 &
//! DISPLAY=:99 cargo test -p canary-render-vulkan --features presentation \
//!     --test present_clear -- --ignored
//! ```
//!
//! Linux-only for now: the window is built with
//! `WinitWindow::new_for_testing` (the `any_thread` escape hatch `cargo
//! test` needs — see `winit_backend.rs`), which exists only on Linux.
//! An equivalent macOS/Windows proof is real, intended future work.
//!
//! One `#[test]` function, not one per concern: `winit` allows a single
//! `EventLoop` per process ever, so splitting across functions fails
//! with `EventLoopError::RecreationAttempt` (see
//! `canary-platform`'s `winit_backend_window.rs`, same constraint).

#![cfg(all(feature = "presentation", target_os = "linux"))]

use canary_platform::surface::SurfaceHandlesProvider;
use canary_platform::{Window, WindowDescriptor};
use canary_render::presentation::{FrameOutcome, SurfaceFormat};
use canary_render_vulkan::VulkanPresenter;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const CLEAR_COLOR: [f32; 4] = [0.1, 0.2, 0.4, 1.0];

#[test]
#[ignore = "needs a live X11 display and a presenting Vulkan ICD (mesa lavapipe); see this file's module docs"]
fn presents_cleared_frames_through_window_surface_and_swapchain() {
    let mut window = canary_platform::winit_backend::WinitWindow::new_for_testing(
        WindowDescriptor {
            title: String::from("canary-present-clear-proof"),
            width: WIDTH,
            height: HEIGHT,
        },
    )
    .expect(
        "failed to create the proof window -- is a display available (DISPLAY set, Xvfb running?)",
    );
    window.poll_events();
    assert!(
        window.supports_presentation(),
        "a live winit window must report itself presentation-capable"
    );
    let extent = window
        .surface_extent()
        .expect("a live winit window must report a surface extent");
    assert!(
        extent.0 > 0 && extent.1 > 0,
        "a visible proof window must not report a minimized (zero) extent, got {extent:?}"
    );
    assert!(
        window.window_handles().is_some(),
        "a live winit window must offer surface handles"
    );

    let mut presenter = VulkanPresenter::new(&window).unwrap_or_else(|error| {
        panic!(
            "failed to build the presentation stack -- is a presenting Vulkan ICD installed \
             (mesa-vulkan-drivers for lavapipe) with X11 presentation support? {error}"
        )
    });
    // The negotiated format follows Canary policy (preferred pair when
    // the driver offers it — lavapipe does).
    assert_eq!(
        presenter.surface_format(),
        SurfaceFormat {
            format_code: SurfaceFormat::PREFERRED_FORMAT_CODE,
            color_space_code: SurfaceFormat::PREFERRED_COLOR_SPACE_CODE,
        },
        "the swapchain must use the preferred format pair when offered"
    );
    assert!(
        presenter.image_count() >= 2,
        "double-buffering is the minimum useful swapchain, got {} images",
        presenter.image_count()
    );

    // Several frames: the first exercises acquire on a fresh swapchain,
    // the rest prove steady-state present works repeatedly (and would
    // surface a suboptimal/recreate path on drivers that report one).
    let mut presented = 0;
    for frame in 0..5 {
        window.poll_events();
        match presenter
            .present_cleared_frame(&window, CLEAR_COLOR)
            .unwrap_or_else(|error| panic!("cleared frame {frame} failed fatally: {error}"))
        {
            FrameOutcome::Presented(report) => {
                let index = usize::try_from(report.image_index)
                    .expect("a swapchain image index fits the address space");
                assert!(
                    index < presenter.image_count(),
                    "presented image {} is outside {} swapchain images",
                    report.image_index,
                    presenter.image_count()
                );
                presented += 1;
            }
            FrameOutcome::Skipped { status } => panic!(
                "a visible, steady window must not skip frames, but frame {frame} skipped: {status:?}"
            ),
        }
    }
    assert_eq!(presented, 5, "every proof frame must present");
}
