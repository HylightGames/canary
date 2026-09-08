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
use canary_render::BufferDescriptor;

use crate::device::{allocate_memory, VulkanDevice};

/// A GPU vertex buffer. `v0.0.6`'s scope: host-visible memory, uploaded
/// once at creation and never updated — see
/// [`canary_render::BufferDescriptor`]'s own docs for why. Uses raw
/// `vkAllocateMemory` per buffer rather than a sub-allocator; see
/// [`crate::device::allocate_memory`]'s docs for why that's fine at this
/// scope.
pub struct VulkanBuffer {
    device: Rc<ash::Device>,
    pub(crate) buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

impl VulkanBuffer {
    pub(crate) fn new(vk_device: &VulkanDevice, desc: &BufferDescriptor<'_>) -> Self {
        let device = &vk_device.device;

        let buffer_ci = vk::BufferCreateInfo::default()
            .size(desc.data.len() as u64)
            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&buffer_ci, None) }
            .unwrap_or_else(|e| panic!("failed to create buffer {:?}: {e}", desc.label));

        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let memory = allocate_memory(
            device,
            &vk_device.memory_properties,
            requirements,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .unwrap_or_else(|e| panic!("failed to allocate memory for buffer {:?}: {e}", desc.label));

        unsafe { device.bind_buffer_memory(buffer, memory, 0) }
            .unwrap_or_else(|e| panic!("failed to bind memory for buffer {:?}: {e}", desc.label));

        unsafe {
            let ptr = device
                .map_memory(memory, 0, requirements.size, vk::MemoryMapFlags::empty())
                .unwrap_or_else(|e| panic!("failed to map buffer {:?}: {e}", desc.label));
            std::ptr::copy_nonoverlapping(desc.data.as_ptr(), ptr as *mut u8, desc.data.len());
            device.unmap_memory(memory);
        }

        Self {
            device: Rc::clone(device),
            buffer,
            memory,
        }
    }
}

impl Drop for VulkanBuffer {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}
