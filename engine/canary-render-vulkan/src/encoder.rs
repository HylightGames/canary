// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use ash::vk;
use canary_render::{CommandEncoder, RenderPassDescriptor};

use crate::buffer::VulkanBuffer;
use crate::color_target::VulkanColorTarget;
use crate::device::VulkanDevice;
use crate::pipeline::VulkanPipeline;
use crate::texture::VulkanTexture;

/// Records one command buffer's worth of GPU work. Allocated fresh per
/// [`VulkanDevice::create_command_encoder`] call and consumed by
/// [`VulkanDevice::submit_and_wait`] — `v0.0.6`'s scope is one
/// submission per frame with no reason yet to pool/reuse command
/// buffers across frames (there's no frame loop yet to reuse them in).
pub struct VulkanCommandEncoder<'a> {
    vk_device: &'a VulkanDevice,
    command_buffer: vk::CommandBuffer,
    /// The layout of the most recently bound pipeline plus whether that
    /// pipeline carries the texture layout — or `None` before any
    /// `set_pipeline` call.
    ///
    /// Descriptor sets bind against a pipeline layout, not into the
    /// void: [`CommandEncoder::set_texture`](canary_render::CommandEncoder::set_texture)
    /// needs the current pipeline's layout, so `set_pipeline` records
    /// it here. A `set_texture` before any `set_pipeline`, or after a
    /// pipeline built *without* the texture layout, is a caller
    /// ordering violation, reported loudly rather than recorded as
    /// undefined work (binding set 0 into a set-less layout segfaults
    /// at least one real driver — observed on llvmpipe while
    /// hardening — so host-side refusal is soundness, not polish).
    bound: Option<BoundPipeline>,
    /// Whether a render pass is currently open (between
    /// `begin_render_pass` and `end_render_pass`) and whether a vertex
    /// buffer has been bound in it. Tracked so `draw` can refuse
    /// unbound draws loudly (recording `vkCmdDraw` with no pipeline is
    /// driver-undefined, in the same class as the `set_texture` ordering
    /// violation this struct already refuses) and so `Drop` can close
    /// out an abandoned encoder without leaking its command buffer.
    pass_open: bool,
    vertex_buffer_bound: bool,
}

/// What [`VulkanCommandEncoder`] remembers about the bound pipeline:
/// its layout to bind descriptor sets into, and whether that layout
/// actually holds the texture set.
#[derive(Debug, Clone, Copy)]
struct BoundPipeline {
    layout: vk::PipelineLayout,
    textured: bool,
}

impl<'a> VulkanCommandEncoder<'a> {
    pub(crate) fn new(vk_device: &'a VulkanDevice) -> Self {
        let ai = vk::CommandBufferAllocateInfo::default()
            .command_pool(vk_device.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: fully specified allocate-info against this device's
        // own pool; returns error (panics below) rather than a null
        // handle on failure.
        let command_buffer = unsafe { vk_device.device.allocate_command_buffers(&ai) }
            .expect("failed to allocate command buffer")[0];

        let begin_info = vk::CommandBufferBeginInfo::default();
        // SAFETY: freshly allocated primary buffer, never begun;
        // default begin-info requests no extensions or inheritance.
        unsafe {
            vk_device
                .device
                .begin_command_buffer(command_buffer, &begin_info)
        }
        .expect("failed to begin command buffer");

        Self {
            vk_device,
            command_buffer,
            bound: None,
            pass_open: false,
            vertex_buffer_bound: false,
        }
    }

    /// Ends, submits, and blocks until this encoder's recorded commands
    /// finish executing — see [`VulkanDevice::submit_and_wait`]'s own
    /// docs for why blocking is fine at `v0.0.6`'s scope.
    pub(crate) fn submit_and_wait(self) {
        let device = &self.vk_device.device;
        // SAFETY: `command_buffer` was begun in `new()` and has only
        // had valid record calls since (each refused loudly on misuse
        // above); ending, submitting, waiting, and freeing it here is
        // the single owner path (`Drop` handles only the abandoned
        // path — see below — and this call `forget`s `self` after).
        unsafe {
            device
                .end_command_buffer(self.command_buffer)
                .expect("failed to end command buffer");

            let command_buffers = [self.command_buffer];
            let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
            device
                .queue_submit(self.vk_device.queue, &[submit_info], vk::Fence::null())
                .expect("failed to submit command buffer");
            device
                .queue_wait_idle(self.vk_device.queue)
                .expect("failed to wait for submitted work to complete");
            device.free_command_buffers(self.vk_device.command_pool, &command_buffers);
        }
        // The command buffer is freed above; `Drop` must not free it
        // again. `submit_and_wait` consumes `self` precisely so there
        // is exactly one owner of the submission decision — forgetting
        // here hands lifetime responsibility back with no second free.
        std::mem::forget(self);
        // `pass_open`/`vertex_buffer_bound` die with the forgotten
        // value; they were only ever needed for the abandoned path.
    }
}

impl<'a> Drop for VulkanCommandEncoder<'a> {
    /// Frees an encoder that never reached [`VulkanCommandEncoder::submit_and_wait`]
    /// (panic mid-pass, early return, `catch_unwind` across test code).
    /// Without this, the allocated command buffer — and any begun but
    /// unended render pass recorded into it — leaks when the pool is
    /// eventually destroyed. Ending an open pass here keeps the buffer
    /// valid to free; a real driver accepts `end` + free without
    /// submit, which is exactly the abandoned-encoder case (nothing was
    /// ever presented, so no layout transition is owed to anyone).
    ///
    // SAFETY: every `unsafe` call below operates on `self.command_buffer`,
    // allocated from `self.vk_device.command_pool` in `new()` and not yet
    // freed (only `submit_and_wait` frees it, and that path forgets
    // `self` so this `Drop` never runs for it). `end_command_buffer` /
    // `end_render_pass` / `free_command_buffers` on a valid, owned,
    // begun buffer is sound; the device outlives the encoder via the
    // `&'a VulkanDevice` borrow (abandoning the encoder cannot drop
    // the device first — borrowck enforces the order).
    fn drop(&mut self) {
        let device = &self.vk_device.device;
        unsafe {
            if self.pass_open {
                device.cmd_end_render_pass(self.command_buffer);
                self.pass_open = false;
            }
            // `Drop` must never panic: this path runs for abandoned
            // encoders, which includes unwinding from an earlier panic
            // (panic mid-pass, `catch_unwind` across test code) — a
            // failed `end` here would panic during that unwind and
            // abort the process. The buffer is freed regardless; a
            // driver that rejects the `end` gets a freed-never-
            // submitted buffer, which is exactly the abandoned case.
            let _ = device.end_command_buffer(self.command_buffer);
            device.free_command_buffers(self.vk_device.command_pool, &[self.command_buffer]);
        }
    }
}

impl<'a> CommandEncoder<VulkanDevice> for VulkanCommandEncoder<'a> {
    fn begin_render_pass(&mut self, target: &VulkanColorTarget, desc: &RenderPassDescriptor) {
        target.mark_drawn();
        let device = &self.vk_device.device;
        let clear_value = vk::ClearValue {
            color: vk::ClearColorValue {
                float32: desc.clear_color,
            },
        };
        let clear_values = [clear_value];
        let render_area = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: target.width,
                height: target.height,
            },
        };
        let render_pass_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.vk_device.render_pass)
            .framebuffer(target.framebuffer)
            .render_area(render_area)
            .clear_values(&clear_values);

        // SAFETY: render pass + framebuffer + area + clear values are
        // all live and mutually compatible (shared pass, target-sized
        // area); viewport/scissor are set to the same target rect
        // immediately after, so no draw can execute outside them.
        unsafe {
            device.cmd_begin_render_pass(
                self.command_buffer,
                &render_pass_begin,
                vk::SubpassContents::INLINE,
            );

            // Dynamic viewport/scissor, set here (where the target's real
            // size is known) rather than baked into the pipeline -- see
            // VulkanPipeline's own docs for why.
            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: target.width as f32,
                height: target.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            device.cmd_set_viewport(self.command_buffer, 0, &[viewport]);
            device.cmd_set_scissor(self.command_buffer, 0, &[render_area]);
        }
        self.pass_open = true;
        self.vertex_buffer_bound = false;
    }

    fn set_pipeline(&mut self, pipeline: &VulkanPipeline) {
        self.bound = Some(BoundPipeline {
            layout: pipeline.layout,
            textured: pipeline.textured,
        });
        // SAFETY: `command_buffer` is begun (constructor) and the
        // pipeline/layout handles are live (borrowed `pipeline`
        // outlives this call); recording bind commands is always valid
        // on a begun buffer regardless of pass state.
        unsafe {
            self.vk_device.device.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.pipeline,
            );
        }
    }

    fn set_vertex_buffer(&mut self, buffer: &VulkanBuffer) {
        // SAFETY: same begun-buffer reasoning as `set_pipeline`;
        // `buffer.buffer` is live (borrowed `buffer` outlives the
        // call) and offset 0 is in-bounds for any nonzero buffer
        // (zero-byte buffers are refused at creation).
        unsafe {
            self.vk_device.device.cmd_bind_vertex_buffers(
                self.command_buffer,
                0,
                &[buffer.buffer],
                &[0],
            );
        }
        self.vertex_buffer_bound = true;
    }

    fn set_texture(&mut self, texture: &VulkanTexture) {
        let bound = self.bound.expect(
            "set_texture requires a bound pipeline: call set_pipeline \
              (with a textured pipeline) before set_texture",
        );
        if !bound.textured {
            panic!(
                "set_texture requires a textured pipeline (one created by \
                 create_textured_pipeline): the bound pipeline's layout holds \
                 no descriptor sets to bind into"
            );
        }
        let layout = bound.layout;
        // SAFETY: `layout` is the currently bound *textured* pipeline's
        // own layout (recorded in `set_pipeline`, refused above when the
        // pipeline is not textured), set 0 of which is
        // the shared texture layout the texture's set was allocated
        // from — so binding set 0 here always matches. No dynamic
        // offsets; one set, first set.
        unsafe {
            self.vk_device.device.cmd_bind_descriptor_sets(
                self.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                layout,
                0,
                &[texture.set],
                &[],
            );
        }
    }

    fn draw(&mut self, vertex_count: u32) {
        // Recording `vkCmdDraw` with no pipeline bound is
        // driver-undefined (same class as the `set_texture` ordering
        // violation refused above) — fail loudly on the host instead.
        // A missing vertex buffer is likewise refused: drawing from
        // binding 0 with nothing bound reads garbage or faults,
        // depending on the driver.
        self.bound.expect(
            "draw requires a bound pipeline: call set_pipeline \
              before draw",
        );
        assert!(
            self.vertex_buffer_bound,
            "draw requires a bound vertex buffer: call set_vertex_buffer before draw"
        );
        // SAFETY: pipeline + vertex buffer presence just verified
        // above; `cmd_draw` with valid bindings on a begun buffer
        // inside an open pass is well-defined.
        unsafe {
            self.vk_device
                .device
                .cmd_draw(self.command_buffer, vertex_count, 1, 0, 0);
        }
    }

    fn end_render_pass(&mut self) {
        self.pass_open = false;
        // SAFETY: ends the pass `begin_render_pass` began on the same
        // begun buffer; symmetric open/close pairing is the only
        // requirement, and both sides live in this impl.
        unsafe {
            self.vk_device
                .device
                .cmd_end_render_pass(self.command_buffer);
        }
    }
}
