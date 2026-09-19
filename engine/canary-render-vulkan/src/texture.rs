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
use canary_render::TextureDescriptor;

use crate::device::{allocate_memory, VulkanDevice};

/// A GPU sampled texture: an optimal-tiling image plus its view, the
/// backend's one default sampler, and the single descriptor set that
/// binds both into a textured pipeline's layout.
///
/// # Why this shape, and what is deferred
///
/// Upload mirrors [`crate::buffer::VulkanBuffer`]'s staging pattern
/// (host-visible staging buffer → `memcpy` → device-local image) because
/// the image itself lives in non-host-visible optimal tiling — the GPU
/// cannot read it otherwise, and this backend has no shared staging
/// infrastructure to reuse. Exactly one mip level, one layer, no
/// array: mipmaps belong to the deferred materials system. The sampler
/// is linear-min/mag with clamp-to-edge addressing, anisotropy off, and
/// LOD pinned to level 0 — the one default this release offers, stated
/// here so a caller needing anything else knows to stop: sampler
/// choice beyond this default is out of scope by the Phase 3b contract.
/// Two descriptor bindings carry the one texture (binding 0: the
/// sampled image; binding 1: the sampler) because WGSL's `texture_2d`
/// plus `sampler` compile to separate image/sampler descriptors rather
/// than a single combined one — see
/// [`canary_render::RenderDevice::create_textured_pipeline`]'s docs.
/// The descriptor pool is owned per texture (one set, two
/// descriptors): real engines sub-allocate sets from shared pools, but
/// this release creates a handful of textures total, so one pool each
/// is correct without being clever.
pub struct VulkanTexture {
    device: Rc<ash::Device>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    sampler: vk::Sampler,
    pool: vk::DescriptorPool,
    pub(crate) set: vk::DescriptorSet,
}

impl VulkanTexture {
    pub(crate) fn new(vk_device: &VulkanDevice, desc: &TextureDescriptor<'_>) -> Self {
        assert!(
            desc.width > 0 && desc.height > 0,
            "texture dimensions must be nonzero"
        );
        // Widen *before* multiplying: `width * height * 4` in `u32`
        // wraps past ~1024px-square textures in release (panics in
        // debug), silently under-counting the expected bytes. `u64`
        // holds any `u32 × u32 × 4` product exactly.
        let expected_bytes = u64::from(desc.width) * u64::from(desc.height) * 4;
        assert_eq!(
            desc.rgba8.len() as u64,
            expected_bytes,
            "texture {:?} claims {}x{} but carries {} bytes, expected {expected_bytes}",
            desc.label,
            desc.width,
            desc.height,
            desc.rgba8.len()
        );
        let device = &vk_device.device;

        let image_ci = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .extent(vk::Extent3D {
                width: desc.width,
                height: desc.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: `image_ci` is fully specified above (format, extent,
        // usage, tiling all set); the returned handle is destroyed in
        // `Drop` after all uses complete (`submit_and_wait` blocks, and
        // the device outlives every texture by the drop-order contract).
        let image = unsafe { device.create_image(&image_ci, None) }
            .unwrap_or_else(|e| panic!("failed to create texture image {:?}: {e}", desc.label));

        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let memory = allocate_memory(
            device,
            &vk_device.memory_properties,
            requirements,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .unwrap_or_else(|e| panic!("failed to allocate texture memory {:?}: {e}", desc.label));
        // SAFETY: `memory` satisfies this image's own requirements (it
        // was allocated against them); binding is once, before any use.
        unsafe { device.bind_image_memory(image, memory, 0) }
            .unwrap_or_else(|e| panic!("failed to bind texture memory {:?}: {e}", desc.label));

        upload_via_staging(vk_device, image, desc);

        let view_ci = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        // SAFETY: view references the live `image` above with a matching
        // format and subresource range; destroyed in `Drop` before the
        // image itself.
        let view = unsafe { device.create_image_view(&view_ci, None) }
            .unwrap_or_else(|e| panic!("failed to create texture view {:?}: {e}", desc.label));

        // The one default sampler: linear filtering, clamp-to-edge on
        // all axes, no anisotropy, no comparison, LOD pinned to the
        // single level. Any caller needing nearest, repeat, mirrors, or
        // mipmapped sampling is asking for the deferred materials
        // system, not for a flag on this constructor.
        let sampler_ci = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .mip_lod_bias(0.0)
            .anisotropy_enable(false)
            .compare_enable(false)
            .min_lod(0.0)
            .max_lod(0.0)
            .border_color(vk::BorderColor::FLOAT_OPAQUE_BLACK)
            .unnormalized_coordinates(false);
        // SAFETY: all fields are valid enum values or neutral zeros for
        // a basic sampling use; destroyed in `Drop`.
        let sampler = unsafe { device.create_sampler(&sampler_ci, None) }
            .unwrap_or_else(|e| panic!("failed to create texture sampler {:?}: {e}", desc.label));

        // One private pool per texture: exactly one set holding one
        // sampled image plus one sampler. Shared pools with
        // sub-allocation are the real-engine answer once textures are
        // created per frame or per material; at this release's handful
        // total, a pool each is the honest minimum. One pool-size entry
        // per descriptor type actually allocated (the layout splits the
        // texture into sampled-image + sampler, not a combined
        // image-sampler, so the pool must name both types).
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: 1,
            },
        ];
        let pool_ci = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(&pool_sizes)
            .max_sets(1);
        // SAFETY: `pool_ci` requests a nonzero, fully specified pool;
        // destroyed in `Drop`, which implicitly frees the set.
        let pool = unsafe { device.create_descriptor_pool(&pool_ci, None) }
            .unwrap_or_else(|e| panic!("failed to create texture pool {:?}: {e}", desc.label));

        let set_layouts = [vk_device.texture_set_layout];
        let set_ai = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&set_layouts);
        // SAFETY: the pool was created with capacity for exactly this
        // one set of this one layout; the set dies with the pool.
        let set = unsafe { device.allocate_descriptor_sets(&set_ai) }
            .unwrap_or_else(|e| panic!("failed to allocate texture set {:?}: {e}", desc.label))[0];

        let image_info = vk::DescriptorImageInfo {
            sampler: vk::Sampler::null(),
            image_view: view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        };
        let sampler_info = vk::DescriptorImageInfo {
            sampler,
            image_view: vk::ImageView::null(),
            image_layout: vk::ImageLayout::UNDEFINED,
        };
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&image_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(std::slice::from_ref(&sampler_info)),
        ];
        // SAFETY: both writes target the freshly allocated `set` with
        // matching binding numbers/types to the shared layout, and the
        // referenced view/sampler outlive the set (destroyed after the
        // pool in `Drop`).
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        Self {
            device: Rc::clone(device),
            image,
            memory,
            view,
            sampler,
            pool,
            set,
        }
    }
}

/// Copies `desc.rgba8` into `image` through a transient host-visible
/// staging buffer, leaving the image in `SHADER_READ_ONLY_OPTIMAL`.
///
/// The image lives in optimal-tiling device-local memory the CPU
/// cannot map, so the bytes travel CPU → staging buffer (`memcpy`)
/// → image (`cmd_copy_buffer_to_image`) on a one-off command buffer
/// that is submitted and waited on before this returns — after which
/// the staging resources are destroyed immediately. Per-texture
/// transient staging (rather than a shared upload ring) is the honest
/// minimum at this release's texture count; an upload ring is later
/// asset-milestone work, not something to pre-build here.
fn upload_via_staging(vk_device: &VulkanDevice, image: vk::Image, desc: &TextureDescriptor<'_>) {
    let device = &vk_device.device;
    let size = desc.rgba8.len() as u64;

    let staging_ci = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: buffer fully specified; destroyed below after the
    // waited-on copy completes.
    let staging = unsafe { device.create_buffer(&staging_ci, None) }.unwrap_or_else(|e| {
        panic!(
            "failed to create texture staging buffer {:?}: {e}",
            desc.label
        )
    });
    let requirements = unsafe { device.get_buffer_memory_requirements(staging) };
    let staging_memory = allocate_memory(
        device,
        &vk_device.memory_properties,
        requirements,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )
    .unwrap_or_else(|e| {
        panic!(
            "failed to allocate texture staging memory {:?}: {e}",
            desc.label
        )
    });
    // SAFETY: bound once against the buffer's own requirements, before use.
    unsafe { device.bind_buffer_memory(staging, staging_memory, 0) }.unwrap_or_else(|e| {
        panic!(
            "failed to bind texture staging memory {:?}: {e}",
            desc.label
        )
    });

    // SAFETY: `HOST_COHERENT` needs no explicit flush/invalidate; the
    // mapped range covers exactly `desc.rgba8.len()` bytes and the copy
    // length matches, so no over-read or over-write is possible.
    unsafe {
        let ptr = device
            .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
            .unwrap_or_else(|e| panic!("failed to map texture staging {:?}: {e}", desc.label));
        std::ptr::copy_nonoverlapping(desc.rgba8.as_ptr(), ptr as *mut u8, desc.rgba8.len());
        device.unmap_memory(staging_memory);
    }

    let command_buffer_ai = vk::CommandBufferAllocateInfo::default()
        .command_pool(vk_device.command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: pool, level, and count are valid; the buffer is freed
    // below after the waited-on submit.
    let command_buffer = unsafe { device.allocate_command_buffers(&command_buffer_ai) }
        .unwrap_or_else(|e| {
            panic!(
                "failed to allocate texture upload commands {:?}: {e}",
                desc.label
            )
        })[0];

    // SAFETY: every recorded command references live objects (the new
    // image, the staging buffer) with correct subresource ranges, and
    // the two layout transitions bracket the copy so each stage sees
    // the image in the layout it requires. Fences-free waiting via
    // `queue_wait_idle` matches this crate's existing one-off style
    // (see `VulkanColorTarget::read_rgba8`).
    unsafe {
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        device
            .begin_command_buffer(command_buffer, &begin_info)
            .unwrap_or_else(|e| panic!("failed to begin texture upload {:?}: {e}", desc.label));

        let to_transfer = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        device.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer],
        );

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
                width: desc.width,
                height: desc.height,
                depth: 1,
            });
        device.cmd_copy_buffer_to_image(
            command_buffer,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[copy_region],
        );

        let to_readable = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ);
        device.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_readable],
        );

        device
            .end_command_buffer(command_buffer)
            .unwrap_or_else(|e| panic!("failed to end texture upload {:?}: {e}", desc.label));

        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        device
            .queue_submit(vk_device.queue, &[submit_info], vk::Fence::null())
            .unwrap_or_else(|e| panic!("failed to submit texture upload {:?}: {e}", desc.label));
        device
            .queue_wait_idle(vk_device.queue)
            .unwrap_or_else(|e| panic!("failed to wait for texture upload {:?}: {e}", desc.label));
        device.free_command_buffers(vk_device.command_pool, &command_buffers);
    }

    // SAFETY: the waited-on copy is complete, so nothing references the
    // staging resources anymore; destroying them here keeps per-texture
    // upload cost transient rather than leaked.
    unsafe {
        device.destroy_buffer(staging, None);
        device.free_memory(staging_memory, None);
    }
}

impl Drop for VulkanTexture {
    fn drop(&mut self) {
        // SAFETY: `submit_and_wait` blocks every queue use before
        // returning, so no in-flight work references this texture when
        // user code drops it. Pool first (frees the set implicitly),
        // then sampler/view/image/memory — the reverse of creation, and
        // all before the device itself by the drop-order contract.
        unsafe {
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}
