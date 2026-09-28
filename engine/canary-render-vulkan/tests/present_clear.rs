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
//! Beyond steady-state clears, the same proof drives the lifecycle
//! gates: swapchain recreation on extent *and* generation changes
//! (through a stub provider carrying the live window's real handles —
//! this environment's tiling window manager ignores client resize
//! requests, so a WM-driven resize is not testable here), suspend on
//! a zero extent, minimize/restore survival on the live window, and
//! explicit presenter-before-window teardown order.
//!
//! The same proof then covers content frames: an RHI-drawn offscreen
//! target (source pixels read back before the blit) presents through
//! `present_color_target`, a mismatched target extent refuses loudly,
//! and the resize → mismatch → recreate-target → present loop reports
//! the recreate flag on the presenting retry.
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
use canary_render::presentation::{AcquireStatus, FrameOutcome, SurfaceFormat};
use canary_render_vulkan::VulkanPresenter;

use std::time::{Duration, Instant};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const CLEAR_COLOR: [f32; 4] = [0.1, 0.2, 0.4, 1.0];

/// Polls until `accept` holds of the window's reported extent, or 5
/// seconds pass. Live window managers resize asynchronously, so the
/// resize/minimize phases below cannot assert on the first poll.
fn wait_for_extent(
    window: &mut canary_platform::winit_backend::WinitWindow,
    accept: impl Fn((u32, u32)) -> bool,
) -> Option<(u32, u32)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        window.poll_events();
        if let Some(extent) = window.surface_extent() {
            if accept(extent) {
                return Some(extent);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

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

    // Resize must recreate the swapchain. This environment's window
    // manager tiles new windows (the 320x240 request above arrives as
    // a larger tiled extent) and ignores client resize requests, so a
    // WM-driven resize is not testable here — instead the test drives
    // the presenter's recreate triggers directly through a stub
    // provider carrying the LIVE window's handles with a canned
    // extent/generation. The recreate decision is a pure function of
    // the reported extent and generation, so this proves the real
    // swapchain rebuild + acquire + present path; the WM-to-`Resized`
    // signal half is `winit` event delivery, not presenter logic.
    let live_handles = window
        .window_handles()
        .expect("a live window must offer surface handles for the stub provider");
    let live_extent = window
        .surface_extent()
        .expect("a live window must report an extent");
    let mut stub = StubProvider::new(
        window.descriptor().clone(),
        live_handles,
        live_extent,
        window.resize_generation(),
    );
    window.poll_events();
    match presenter.present_cleared_frame(&stub, CLEAR_COLOR) {
        Ok(FrameOutcome::Presented(report)) => assert!(
            !report.swapchain_recreated,
            "presenting at the unchanged extent/generation must not recreate"
        ),
        outcome => panic!("the stub at the live extent must present, got {outcome:?}"),
    }

    // Extent change with a live generation bump: the swapchain must
    // rebuild to the new extent, then present into it.
    stub.set_extent((480, 360));
    let mut saw_recreate = false;
    let mut presented_after_resize = false;
    for frame in 0..10 {
        match presenter
            .present_cleared_frame(&stub, CLEAR_COLOR)
            .unwrap_or_else(|error| panic!("resized frame {frame} failed fatally: {error}"))
        {
            FrameOutcome::Presented(report) => {
                eprintln!(
                    "resized frame {frame}: presented image {} (recreated={})",
                    report.image_index, report.swapchain_recreated
                );
                saw_recreate |= report.swapchain_recreated;
                presented_after_resize = true;
                break;
            }
            FrameOutcome::Skipped { status } => {
                eprintln!("resized frame {frame}: skipped ({status:?})");
                saw_recreate |= status == AcquireStatus::Outdated;
            }
        }
    }
    assert!(
        presented_after_resize,
        "a resized surface must present again"
    );
    assert!(
        saw_recreate,
        "an extent change {live_extent:?} -> (480, 360) must have recreated the swapchain"
    );
    assert_eq!(
        presenter.extent(),
        (480, 360),
        "the presenter must track the rebuilt swapchain's extent"
    );

    // Generation bump at an UNCHANGED extent must also recreate (this
    // is how the presenter learns about resizes the extent check
    // cannot see, e.g. same-size surface reallocations).
    stub.bump_generation();
    match presenter.present_cleared_frame(&stub, CLEAR_COLOR) {
        Ok(FrameOutcome::Presented(report)) => assert!(
            report.swapchain_recreated,
            "a generation bump must recreate even at an unchanged extent"
        ),
        outcome => panic!("the stub after a generation bump must present, got {outcome:?}"),
    }

    // A zero extent must suspend before touching the swapchain: this
    // is the branch a cooperating window manager reaches by reporting
    // 0x0 for a minimized window (this environment's WM never does —
    // see the minimize phase below — so the stub proves the branch
    // the live window cannot reach here).
    stub.set_extent((0, 0));
    match presenter.present_cleared_frame(&stub, CLEAR_COLOR) {
        Ok(FrameOutcome::Skipped { status }) => assert_eq!(
            status,
            AcquireStatus::Suspended,
            "a zero extent must suspend, got {status:?}"
        ),
        outcome => panic!("a zero extent must suspend, got {outcome:?}"),
    }

    // Minimizing must suspend, never fail: a zero extent skips the
    // frame; if this platform keeps reporting a nonzero extent while
    // minimized, presenting into it must still succeed.
    window.set_minimized_for_testing(true);
    let _ = wait_for_extent(&mut window, |_| true);
    window.poll_events();
    let minimized_extent = window.surface_extent();
    match presenter.present_cleared_frame(&window, CLEAR_COLOR) {
        Ok(FrameOutcome::Skipped { status }) => {
            eprintln!("minimized frame: skipped ({status:?}) at {minimized_extent:?}");
            assert_eq!(
                status,
                AcquireStatus::Suspended,
                "a minimized (zero-extent) window must suspend, got {status:?}"
            );
        }
        Ok(FrameOutcome::Presented(_)) => {
            eprintln!("minimized frame: presented at {minimized_extent:?}");
            assert!(
                minimized_extent.is_some_and(|extent| extent.0 > 0 && extent.1 > 0),
                "presenting while minimized is only valid at a nonzero extent, got {minimized_extent:?}"
            );
        }
        Err(error) => panic!("a minimized window must suspend or present, never fail: {error}"),
    }
    // Restore and prove the window presents again afterwards.
    window.set_minimized_for_testing(false);
    wait_for_extent(&mut window, |extent| extent.0 > 0 && extent.1 > 0)
        .expect("an un-minimized window must report a nonzero extent again");
    let mut presented_after_restore = false;
    for frame in 0..10 {
        window.poll_events();
        match presenter
            .present_cleared_frame(&window, CLEAR_COLOR)
            .unwrap_or_else(|error| panic!("restored frame {frame} failed fatally: {error}"))
        {
            FrameOutcome::Presented(_) => {
                presented_after_restore = true;
                break;
            }
            FrameOutcome::Skipped { .. } => {}
        }
    }
    assert!(
        presented_after_restore,
        "an un-minimized window must present again"
    );

    // Content frames: scene pixels drawn through the RHI into an
    // offscreen target must reach the window via blit + present. The
    // target is drawn with a covering red triangle (read back offscreen
    // first, so the source pixels are proven before the blit), then
    // presented; the frame must report Presented with a valid image.
    {
        use canary_render::{
            BufferDescriptor, ColorTargetDescriptor, PipelineDescriptor, RenderDevice,
            VertexAttribute, VertexFormat,
        };

        const CONTENT_WGSL: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec3<f32>,
}
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
}
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(input.position, 0.0, 1.0);
    out.color = input.color;
    return out;
}
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
"#;
        fn compile_content_stage(stage: naga::ShaderStage) -> Vec<u32> {
            let module =
                naga::front::wgsl::parse_str(CONTENT_WGSL).expect("failed to parse content WGSL");
            let mut validator = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            );
            let info = validator
                .validate(&module)
                .expect("content WGSL failed validation");
            let entry_point = match stage {
                naga::ShaderStage::Vertex => "vs_main",
                naga::ShaderStage::Fragment => "fs_main",
                _ => unreachable!("only vertex/fragment stages in this test's shader"),
            };
            let options = naga::back::spv::Options {
                lang_version: (1, 0),
                ..Default::default()
            };
            let pipeline_options = naga::back::spv::PipelineOptions {
                shader_stage: stage,
                entry_point: entry_point.to_string(),
            };
            let mut buffer = Vec::new();
            naga::back::spv::Writer::new(&options)
                .expect("failed to create SPIR-V writer")
                .write(&module, &info, Some(&pipeline_options), &None, &mut buffer)
                .expect("failed to write SPIR-V");
            buffer
        }

        let device = presenter.device();
        let (target_w, target_h) = presenter.extent();
        let mut target = device.create_color_target(&ColorTargetDescriptor {
            width: target_w,
            height: target_h,
        });
        // Covering triangle (strictly interior center): NDC positions,
        // flat red.
        let vertices: [f32; 15] = [
            -1.0, -1.0, 1.0, 0.0, 0.0, //
            3.0, -1.0, 1.0, 0.0, 0.0, //
            -1.0, 3.0, 1.0, 0.0, 0.0,
        ];
        let vertex_bytes: Vec<u8> = vertices
            .iter()
            .flat_map(|vertex: &f32| vertex.to_ne_bytes())
            .collect();
        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: "content-proof vertices",
            data: &vertex_bytes,
        });
        let vertex_spirv = compile_content_stage(naga::ShaderStage::Vertex);
        let fragment_spirv = compile_content_stage(naga::ShaderStage::Fragment);
        let vertex_attributes = [
            VertexAttribute {
                shader_location: 0,
                format: VertexFormat::Float32x2,
                offset: 0,
            },
            VertexAttribute {
                shader_location: 1,
                format: VertexFormat::Float32x3,
                offset: VertexFormat::Float32x2.size_bytes(),
            },
        ];
        let pipeline = device.create_pipeline(&PipelineDescriptor {
            label: "content-proof pipeline",
            vertex_shader_spirv: &vertex_spirv,
            vertex_entry_point: "vs_main",
            fragment_shader_spirv: &fragment_spirv,
            fragment_entry_point: "fs_main",
            vertex_stride: VertexFormat::Float32x2.size_bytes()
                + VertexFormat::Float32x3.size_bytes(),
            vertex_attributes: &vertex_attributes,
        });

        fn draw_red_content(
            device: &canary_render_vulkan::VulkanDevice,
            target: &canary_render_vulkan::VulkanColorTarget,
            pipeline: &canary_render_vulkan::VulkanPipeline,
            vertex_buffer: &canary_render_vulkan::VulkanBuffer,
        ) {
            use canary_render::{CommandEncoder, RenderDevice, RenderPassDescriptor};
            let mut encoder = device.create_command_encoder();
            encoder.begin_render_pass(
                target,
                &RenderPassDescriptor {
                    clear_color: [0.0, 0.0, 1.0, 1.0],
                },
            );
            encoder.set_pipeline(pipeline);
            encoder.set_vertex_buffer(vertex_buffer);
            encoder.draw(3);
            encoder.end_render_pass();
            device.submit_and_wait(encoder);
        }

        draw_red_content(device, &target, &pipeline, &vertex_buffer);
        let pixels = device.read_color_target_rgba8(&target);
        let center = {
            let idx = ((target_h / 2 * target_w + target_w / 2) * 4) as usize;
            [
                pixels[idx],
                pixels[idx + 1],
                pixels[idx + 2],
                pixels[idx + 3],
            ]
        };
        assert_eq!(
            center,
            [255, 0, 0, 255],
            "the content source must be red before the blit, got {center:?}"
        );

        match presenter.present_color_target(&window, &target) {
            Ok(FrameOutcome::Presented(report)) => {
                eprintln!(
                    "content frame: presented image {} (recreated={})",
                    report.image_index, report.swapchain_recreated
                );
                let index = usize::try_from(report.image_index)
                    .expect("a swapchain image index fits the address space");
                assert!(
                    index < presenter.image_count(),
                    "presented image {} is outside {} swapchain images",
                    report.image_index,
                    presenter.image_count()
                );
            }
            outcome => panic!("the red content target must present, got {outcome:?}"),
        }

        // A mismatched target extent must refuse loudly, never scale or
        // crop silently.
        let small = presenter
            .device()
            .create_color_target(&ColorTargetDescriptor {
                width: 64,
                height: 64,
            });
        match presenter.present_color_target(&window, &small) {
            Err(canary_render::presentation::PresentationError::ContentExtentMismatch {
                swapchain,
                content,
            }) => {
                assert_eq!(swapchain, presenter.extent());
                assert_eq!(content, (64, 64));
            }
            outcome => panic!("a 64x64 target must mismatch-refuse, got {outcome:?}"),
        }

        // Resize then re-present: the stub drives the swapchain to
        // (480, 360); the old target must mismatch, a recreated target
        // must present with the recreate flag set — the exact loop the
        // windowed sample runs every frame.
        stub.set_extent((480, 360));
        match presenter.present_color_target(&stub, &target) {
            Err(canary_render::presentation::PresentationError::ContentExtentMismatch {
                ..
            }) => {}
            outcome => panic!("the pre-resize target must mismatch after resize, got {outcome:?}"),
        }
        target = presenter
            .device()
            .create_color_target(&ColorTargetDescriptor {
                width: 480,
                height: 360,
            });
        draw_red_content(presenter.device(), &target, &pipeline, &vertex_buffer);
        let mut saw_content_recreate = false;
        for frame in 0..10 {
            match presenter
                .present_color_target(&stub, &target)
                .unwrap_or_else(|error| {
                    panic!("resized content frame {frame} failed fatally: {error}")
                }) {
                FrameOutcome::Presented(report) => {
                    eprintln!(
                        "resized content frame {frame}: presented image {} (recreated={})",
                        report.image_index, report.swapchain_recreated
                    );
                    saw_content_recreate |= report.swapchain_recreated;
                    break;
                }
                FrameOutcome::Skipped { status } => {
                    eprintln!("resized content frame {frame}: skipped ({status:?})");
                }
            }
        }
        assert!(
            saw_content_recreate,
            "re-presenting content after resize must recreate the swapchain"
        );
        assert_eq!(presenter.extent(), (480, 360));
    }

    // Explicit teardown order — presenter (device, swapchain, surface)
    // before the window whose handles it was built from — so a
    // use-after-destroy regression fails here, not in a `Drop` no test
    // watches.
    drop(presenter);
}

/// A [`SurfaceHandlesProvider`] carrying a live window's real handles
/// with a scripted extent and resize generation, so the proof can
/// drive the presenter's recreate triggers without depending on a
/// cooperating window manager (see the resize phase above).
struct StubProvider {
    descriptor: WindowDescriptor,
    handles: canary_platform::surface::WindowHandles,
    extent: (u32, u32),
    generation: u64,
}

impl StubProvider {
    fn new(
        descriptor: WindowDescriptor,
        handles: canary_platform::surface::WindowHandles,
        extent: (u32, u32),
        generation: u64,
    ) -> Self {
        Self {
            descriptor,
            handles,
            extent,
            generation,
        }
    }

    fn set_extent(&mut self, extent: (u32, u32)) {
        self.extent = extent;
        self.generation = self.generation.saturating_add(1);
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }
}

impl Window for StubProvider {
    fn descriptor(&self) -> &WindowDescriptor {
        &self.descriptor
    }

    fn should_close(&self) -> bool {
        false
    }

    fn poll_events(&mut self) {}

    fn surface_extent(&self) -> Option<(u32, u32)> {
        Some(self.extent)
    }

    fn resize_generation(&self) -> u64 {
        self.generation
    }
}

impl SurfaceHandlesProvider for StubProvider {
    fn window_handles(&self) -> Option<canary_platform::surface::WindowHandles> {
        Some(self.handles)
    }
}
