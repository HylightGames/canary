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

/// Records one command buffer's worth of GPU work. Allocated fresh per
/// [`VulkanDevice::create_command_encoder`] call and consumed by
/// [`VulkanDevice::submit_and_wait`] — `v0.0.6`'s scope is one
/// submission per frame with no reason yet to pool/reuse command
/// buffers across frames (there's no frame loop yet to reuse them in).
pub struct VulkanCommandEncoder<'a> {
    vk_device: &'a VulkanDevice,
    command_buffer: vk::CommandBuffer,
}

impl<'a> VulkanCommandEncoder<'a> {
    pub(crate) fn new(vk_device: &'a VulkanDevice) -> Self {
        let ai = vk::CommandBufferAllocateInfo::default()
            .command_pool(vk_device.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer = unsafe { vk_device.device.allocate_command_buffers(&ai) }
            .expect("failed to allocate command buffer")[0];

        let begin_info = vk::CommandBufferBeginInfo::default();
        unsafe {
            vk_device
                .device
                .begin_command_buffer(command_buffer, &begin_info)
        }
        .expect("failed to begin command buffer");

        Self {
            vk_device,
            command_buffer,
        }
    }

    /// Ends, submits, and blocks until this encoder's recorded commands
    /// finish executing — see [`VulkanDevice::submit_and_wait`]'s own
    /// docs for why blocking is fine at `v0.0.6`'s scope.
    pub(crate) fn submit_and_wait(self) {
        let device = &self.vk_device.device;
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
    }
}

impl<'a> CommandEncoder<VulkanDevice> for VulkanCommandEncoder<'a> {
    fn begin_render_pass(&mut self, target: &VulkanColorTarget, desc: &RenderPassDescriptor) {
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
    }

    fn set_pipeline(&mut self, pipeline: &VulkanPipeline) {
        unsafe {
            self.vk_device.device.cmd_bind_pipeline(
                self.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.pipeline,
            );
        }
    }

    fn set_vertex_buffer(&mut self, buffer: &VulkanBuffer) {
        unsafe {
            self.vk_device.device.cmd_bind_vertex_buffers(
                self.command_buffer,
                0,
                &[buffer.buffer],
                &[0],
            );
        }
    }

    fn draw(&mut self, vertex_count: u32) {
        unsafe {
            self.vk_device
                .device
                .cmd_draw(self.command_buffer, vertex_count, 1, 0, 0);
        }
    }

    fn end_render_pass(&mut self) {
        unsafe {
            self.vk_device
                .device
                .cmd_end_render_pass(self.command_buffer);
        }
    }
}
