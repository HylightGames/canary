// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::rc::Rc;

use ash::vk;
use canary_render::ColorTargetDescriptor;

use crate::device::{allocate_memory, VulkanDevice, COLOR_FORMAT};

/// An offscreen color render target: a GPU image, its view, and a
/// framebuffer against [`VulkanDevice`]'s single shared render pass (see
/// that module's docs for why one shared render pass suffices for
/// `v0.0.6`'s scope). No swapchain, no window surface — see
/// [`canary_render::ColorTargetDescriptor`]'s own docs.
pub struct VulkanColorTarget {
    device: Rc<ash::Device>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    pub(crate) framebuffer: vk::Framebuffer,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl VulkanColorTarget {
    pub(crate) fn new(vk_device: &VulkanDevice, desc: &ColorTargetDescriptor) -> Self {
        assert!(
            desc.width > 0 && desc.height > 0,
            "color target dimensions must be nonzero"
        );
        let device = &vk_device.device;

        let image_ci = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(COLOR_FORMAT)
            .extent(vk::Extent3D {
                width: desc.width,
                height: desc.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = unsafe { device.create_image(&image_ci, None) }
            .expect("failed to create color target image");

        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let memory = allocate_memory(
            device,
            &vk_device.memory_properties,
            requirements,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .expect("failed to allocate color target memory");
        unsafe { device.bind_image_memory(image, memory, 0) }
            .expect("failed to bind color target memory");

        let view_ci = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(COLOR_FORMAT)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_ci, None) }
            .expect("failed to create color target image view");

        let attachments = [view];
        let framebuffer_ci = vk::FramebufferCreateInfo::default()
            .render_pass(vk_device.render_pass)
            .attachments(&attachments)
            .width(desc.width)
            .height(desc.height)
            .layers(1);
        let framebuffer = unsafe { device.create_framebuffer(&framebuffer_ci, None) }
            .expect("failed to create color target framebuffer");

        Self {
            device: Rc::clone(device),
            image,
            memory,
            view,
            framebuffer,
            width: desc.width,
            height: desc.height,
        }
    }

    /// Copies this target's current contents to a temporary host-visible
    /// staging buffer and reads it back as tightly packed 8-bit RGBA.
    /// Records and submits its own one-off command buffer — `v0.0.6`'s
    /// scope is a single readback after a single frame, not a
    /// steady-state render loop that would want to avoid the
    /// `queue_wait_idle` this implies on every call.
    pub(crate) fn read_rgba8(&self, vk_device: &VulkanDevice) -> Vec<u8> {
        let device = &vk_device.device;
        let size = (self.width * self.height * 4) as u64;

        let staging_buffer_ci = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let staging_buffer = unsafe { device.create_buffer(&staging_buffer_ci, None) }
            .expect("failed to create readback staging buffer");
        let requirements = unsafe { device.get_buffer_memory_requirements(staging_buffer) };
        let staging_memory = allocate_memory(
            device,
            &vk_device.memory_properties,
            requirements,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .expect("failed to allocate readback staging memory");
        unsafe { device.bind_buffer_memory(staging_buffer, staging_memory, 0) }
            .expect("failed to bind readback staging memory");

        let command_buffer_ai = vk::CommandBufferAllocateInfo::default()
            .command_pool(vk_device.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer = unsafe { device.allocate_command_buffers(&command_buffer_ai) }
            .expect("failed to allocate readback command buffer")[0];

        unsafe {
            let begin_info = vk::CommandBufferBeginInfo::default();
            device
                .begin_command_buffer(command_buffer, &begin_info)
                .expect("failed to begin readback command buffer");

            let copy_region = vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
                .image_extent(vk::Extent3D {
                    width: self.width,
                    height: self.height,
                    depth: 1,
                });
            // No layout-transition barrier needed here: the render pass
            // that last wrote this image already ends in
            // TRANSFER_SRC_OPTIMAL (see device.rs's create_render_pass).
            device.cmd_copy_image_to_buffer(
                command_buffer,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                staging_buffer,
                &[copy_region],
            );

            device
                .end_command_buffer(command_buffer)
                .expect("failed to end readback command buffer");

            let command_buffers = [command_buffer];
            let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
            device
                .queue_submit(vk_device.queue, &[submit_info], vk::Fence::null())
                .expect("failed to submit readback command buffer");
            device
                .queue_wait_idle(vk_device.queue)
                .expect("failed to wait for readback to complete");
            device.free_command_buffers(vk_device.command_pool, &command_buffers);
        }

        let pixels = unsafe {
            let ptr = device
                .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
                .expect("failed to map readback staging memory");
            let data = std::slice::from_raw_parts(ptr as *const u8, size as usize).to_vec();
            device.unmap_memory(staging_memory);
            data
        };

        unsafe {
            device.destroy_buffer(staging_buffer, None);
            device.free_memory(staging_memory, None);
        }

        pixels
    }
}

impl Drop for VulkanColorTarget {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_framebuffer(self.framebuffer, None);
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}
