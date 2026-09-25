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
/// this crate's `allocate_memory` helper docs for why that's fine at this
/// scope.
pub struct VulkanBuffer {
    device: Rc<ash::Device>,
    pub(crate) buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

impl VulkanBuffer {
    pub(crate) fn new(vk_device: &VulkanDevice, desc: &BufferDescriptor<'_>) -> Self {
        // A zero-byte buffer is driver-undefined (`size == 0` violates
        // buffer-creation validity, and mapping zero bytes violates map
        // validity) — fail loudly like the sibling texture and color
        // target constructors do for empty dimensions, rather than
        // recording undefined work through an infallible API. Callers
        // with nothing to upload skip the draw instead (see the
        // bridge's `is_empty` guards).
        assert!(
            !desc.data.is_empty(),
            "buffer {:?} must carry nonzero bytes",
            desc.label
        );
        let device = &vk_device.device;

        let buffer_ci = vk::BufferCreateInfo::default()
            .size(desc.data.len() as u64)
            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: `buffer_ci` is a fully specified create-info (valid
        // size from the assert above, no chained structs); a valid
        // `ash::Device` accepts it or returns an error, which panics
        // loudly below instead of proceeding with a null handle.
        let buffer = unsafe { device.create_buffer(&buffer_ci, None) }
            .unwrap_or_else(|e| panic!("failed to create buffer {:?}: {e}", desc.label));

        // SAFETY: `buffer` is a live, just-created `VkBuffer` owned by
        // this call; querying its own requirements cannot alias or dangle.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let memory = allocate_memory(
            device,
            &vk_device.memory_properties,
            requirements,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .unwrap_or_else(|e| panic!("failed to allocate memory for buffer {:?}: {e}", desc.label));

        // SAFETY: `buffer`/`memory` are live and owned here; binding is
        // a driver bookkeeping call with no aliasing surface.
        unsafe { device.bind_buffer_memory(buffer, memory, 0) }
            .unwrap_or_else(|e| panic!("failed to bind memory for buffer {:?}: {e}", desc.label));

        // SAFETY: `memory` is HOST_VISIBLE+HOST_COHERENT (requested
        // above) and large enough by construction (`requirements.size`
        // covers `data.len()`); the copy stays in bounds on both ends
        // and unmap follows before any GPU use.
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
        // SAFETY: `buffer`/`memory` are still owned here (freed exactly
        // once — creation is the only other site, and it moves them
        // into `Self`), and the device outlives every resource via the
        // `Rc<ash::Device>` clone (see the drop-order contract on
        // `VulkanDevice`).
        unsafe {
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}
