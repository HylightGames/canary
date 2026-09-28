// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The painter's pixel proof: hand-built [`UiPaint`](canary_ui_core::UiPaint)
//! submits through a real RHI backend and reads back real pixels —
//! texture upload, NDC mapping, per-batch scissor, and the skip paths.
//!
//! Deterministic by construction: no fonts, no `egui` frame — one 1x1 red
//! texture, one covering triangle, explicit clip rects. Font-atlas end to
//! end (tessellate → paint → pixels) is the windowed sample's live proof,
//! not this test's.
//!
//! `#[ignore]`d by default, same as the backend's own proof tests: needs a
//! real Vulkan ICD. Run explicitly:
//!
//! ```sh
//! cargo test -p canary-ui-egui --test ui_paint_pixels -- --ignored
//! ```

use canary_render::{ColorTargetDescriptor, CommandEncoder, RenderDevice, RenderPassDescriptor};
use canary_render_vulkan::VulkanDevice;
use canary_ui_core::{
    UiClipRect, UiDrawBatch, UiPaint, UiTextureId, UiTextureOp, UiTriangle, UiVertex,
};
use canary_ui_egui::{UiPainter, UiViewport, UI_PAINT_WGSL};

/// Same `naga` WGSL → SPIR-V path as the backend's own proof harnesses:
/// parse, validate, cross-compile one stage.
fn compile_stage(source: &str, stage: naga::ShaderStage) -> Vec<u32> {
    let module = naga::front::wgsl::parse_str(source).expect("failed to parse WGSL");
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    let info = validator.validate(&module).expect("WGSL failed validation");

    let entry_point = match stage {
        naga::ShaderStage::Vertex => "vs_main",
        naga::ShaderStage::Fragment => "fs_main",
        // Wildcard, not an exhaustive variant list: naga grows `ShaderStage`
        // over majors; this harness only ever compiles vertex + fragment.
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

fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * width + x) * 4) as usize;
    [
        pixels[idx],
        pixels[idx + 1],
        pixels[idx + 2],
        pixels[idx + 3],
    ]
}

/// One opaque red vertex: the test texture is 1x1 red, so UVs are pinned
/// to the single texel and the vertex color carries the red.
fn red_vertex(x: f32, y: f32) -> UiVertex {
    UiVertex {
        position: [x, y],
        uv: [0.0, 0.0],
        color: [1.0, 0.0, 0.0, 1.0],
    }
}

/// A triangle covering the whole 64x64 logical viewport with margin:
/// logical (0,0), (128,0), (0,128) — NDC (-1,1), (3,1), (-1,-3), so the
/// center is strictly interior (an exact half-viewport triangle would put
/// it on the hypotenuse edge, where fill rules may exclude it).
fn covering_triangle() -> UiTriangle {
    UiTriangle {
        vertices: [
            red_vertex(0.0, 0.0),
            red_vertex(128.0, 0.0),
            red_vertex(0.0, 128.0),
        ],
    }
}

fn fullscreen_clip() -> UiClipRect {
    UiClipRect {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 64.0,
        max_y: 64.0,
    }
}

const VIEWPORT: UiViewport = UiViewport {
    target_width_px: 64,
    target_height_px: 64,
    pixels_per_point: 1.0,
};

/// Paints `paint` (after `ops`) over a blue clear and reads back every
/// pixel. Returns the pixels plus the painter's stats for the frame.
fn paint_over_blue(
    device: &VulkanDevice,
    painter: &mut UiPainter<VulkanDevice>,
    ops: &[UiTextureOp],
    paint: &UiPaint,
) -> (Vec<u8>, canary_ui_egui::UiPaintStats) {
    painter.apply_texture_ops(device, ops);
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: 64,
        height: 64,
    });
    let stats = paint_one_pass(device, painter, &target, paint);
    let pixels = device.read_color_target_rgba8(&target);
    (pixels, stats)
}

/// One paint + submit pass on the caller's target: begin, paint, end,
/// submit with the uploads held across it, then release. Split out so the
/// target-reuse regression test below can run two passes on ONE target —
/// a fresh target per pass cannot reproduce the incident this guards.
fn paint_one_pass(
    device: &VulkanDevice,
    painter: &mut UiPainter<VulkanDevice>,
    target: &canary_render_vulkan::VulkanColorTarget,
    paint: &UiPaint,
) -> canary_ui_egui::UiPaintStats {
    let mut encoder = device.create_command_encoder();
    encoder.begin_render_pass(
        target,
        &RenderPassDescriptor {
            clear_color: [0.0, 0.0, 1.0, 1.0],
        },
    );
    let draw = painter.paint(device, &mut encoder, VIEWPORT, paint);
    encoder.end_render_pass();
    let stats = draw.stats;
    // `draw` (not `draw.stats` alone) is held across submit: dropping the
    // uploads before the GPU reads them is use-after-free.
    device.submit_and_wait(encoder);
    drop(draw);
    stats
}

#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn paint_uploads_texture_maps_ndc_and_respects_scissor() {
    let device = VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (this sandbox needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    });
    let vertex_spirv = compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Vertex);
    let fragment_spirv = compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Fragment);
    let mut painter = UiPainter::new(&device, &vertex_spirv, &fragment_spirv);

    // The red 1x1 upload feeding both frames.
    let red_upload = UiTextureOp::Set {
        id: UiTextureId::Managed(0),
        width: 1,
        height: 1,
        pixels: vec![255, 0, 0, 255],
    };

    // Frame A: fullscreen batch — the covering triangle must paint the
    // center pixel red, proving upload + NDC mapping + texture bind.
    let paint = UiPaint {
        batches: vec![UiDrawBatch {
            clip: fullscreen_clip(),
            texture: UiTextureId::Managed(0),
            triangles: vec![covering_triangle()],
        }],
        textures: Vec::new(),
    };
    let (pixels, stats) = paint_over_blue(&device, &mut painter, &[red_upload], &paint);
    assert_eq!(stats.batches_drawn, 1);
    assert_eq!(stats.batches_skipped, 0);
    assert_eq!(stats.triangles_drawn, 1);
    assert_eq!(
        pixel_at(&pixels, 64, 32, 32),
        [255, 0, 0, 255],
        "the covering triangle must paint the center red"
    );

    // Frame B: 1px top-left clip — the center must stay blue (scissor
    // holds) while the corner paints red. No new upload: the texture
    // persists in the painter across frames.
    let clipped = UiPaint {
        batches: vec![UiDrawBatch {
            clip: UiClipRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 1.0,
                max_y: 1.0,
            },
            texture: UiTextureId::Managed(0),
            triangles: vec![covering_triangle()],
        }],
        textures: Vec::new(),
    };
    let (pixels, stats) = paint_over_blue(&device, &mut painter, &[], &clipped);
    assert_eq!(stats.batches_drawn, 1);
    assert_eq!(
        pixel_at(&pixels, 64, 32, 32),
        [0, 0, 255, 255],
        "the scissored batch must leave the center blue"
    );
    assert_eq!(
        pixel_at(&pixels, 64, 0, 0),
        [255, 0, 0, 255],
        "the scissored batch must paint inside its 1px clip"
    );
}

#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn paint_skips_unknown_textures_and_empty_batches() {
    let device = VulkanDevice::new().unwrap_or_else(|e| {
        panic!("failed to create a real Vulkan device -- is a Vulkan ICD installed? {e}")
    });
    let vertex_spirv = compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Vertex);
    let fragment_spirv = compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Fragment);
    let mut painter = UiPainter::new(&device, &vertex_spirv, &fragment_spirv);

    // No uploads at all: the unknown-texture batch and the empty batch
    // must both skip, and the target must stay clear-blue.
    let paint = UiPaint {
        batches: vec![
            UiDrawBatch {
                clip: fullscreen_clip(),
                texture: UiTextureId::User(9),
                triangles: vec![covering_triangle()],
            },
            UiDrawBatch {
                clip: fullscreen_clip(),
                texture: UiTextureId::Managed(0),
                triangles: Vec::new(),
            },
        ],
        textures: Vec::new(),
    };
    let (pixels, stats) = paint_over_blue(&device, &mut painter, &[], &paint);
    assert_eq!(stats.batches_drawn, 0);
    assert_eq!(stats.batches_skipped, 2);
    assert_eq!(stats.triangles_drawn, 0);
    assert_eq!(pixel_at(&pixels, 64, 32, 32), [0, 0, 255, 255]);
    assert_eq!(pixel_at(&pixels, 64, 0, 0), [0, 0, 255, 255]);
}

/// Target-reuse regression: the windowed sample lost its device on the
/// second frame because it dropped a per-frame vertex buffer before
/// submit; the next frame then reused the target while the driver had
/// already recycled the freed block (fresh targets never reproduce it —
/// the killer needs the reuse). Two paint + submit passes on ONE target,
/// each pass's uploads held across its submit, must both survive with
/// the center pixel red at the end. Dropping a pass's uploads before its
/// submit kills the device on a reused target — this test goes red with
/// it.
#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
fn reused_target_with_held_buffers_survives_two_passes() {
    let device = VulkanDevice::new().unwrap_or_else(|e| {
        panic!("failed to create a real Vulkan device -- is a Vulkan ICD installed? {e}")
    });
    let vertex_spirv = compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Vertex);
    let fragment_spirv = compile_stage(UI_PAINT_WGSL, naga::ShaderStage::Fragment);
    let mut painter = UiPainter::new(&device, &vertex_spirv, &fragment_spirv);

    painter.apply_texture_ops(
        &device,
        &[UiTextureOp::Set {
            id: UiTextureId::Managed(0),
            width: 1,
            height: 1,
            pixels: vec![255, 0, 0, 255],
        }],
    );
    let paint = UiPaint {
        batches: vec![UiDrawBatch {
            clip: fullscreen_clip(),
            texture: UiTextureId::Managed(0),
            triangles: vec![covering_triangle()],
        }],
        textures: Vec::new(),
    };
    let target = device.create_color_target(&ColorTargetDescriptor {
        width: 64,
        height: 64,
    });
    for pass in 0..2 {
        let stats = paint_one_pass(&device, &mut painter, &target, &paint);
        assert_eq!(
            stats.batches_drawn, 1,
            "pass {pass}: the fullscreen batch must draw"
        );
    }
    let pixels = device.read_color_target_rgba8(&target);
    assert_eq!(
        pixel_at(&pixels, 64, 32, 32),
        [255, 0, 0, 255],
        "the second pass on the reused target must leave the center red"
    );
}
