// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::ffi::CString;
use std::rc::Rc;

use ash::vk;
use canary_render::{BufferDescriptor, ColorTargetDescriptor, PipelineDescriptor, RenderDevice};

use crate::buffer::VulkanBuffer;
use crate::color_target::VulkanColorTarget;
use crate::encoder::VulkanCommandEncoder;
use crate::pipeline::VulkanPipeline;

/// The single color format this backend supports for `v0.0.6`'s scope.
/// Fixed (not a per-target choice) so a single shared render pass,
/// created once by [`VulkanDevice::new`], is compatible with every
/// [`VulkanColorTarget`] and [`VulkanPipeline`] this backend creates --
/// avoiding any render-pass-compatibility bookkeeping this release's
/// milestone has no real need for. A per-target format choice is real,
/// likely future work once something (a real render graph with multiple
/// attachment formats) actually needs it.
pub(crate) const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

/// The Vulkan RHI backend. Owns the instance, logical device, queue, a
/// single shared render pass (see the crate-internal `COLOR_FORMAT` constant's docs for why one
/// suffices for `v0.0.6`), and a command pool.
///
/// The logical device handle is wrapped in an `Rc` so that every
/// resource this creates ([`VulkanBuffer`], [`VulkanColorTarget`],
/// [`VulkanPipeline`]) can clean itself up in its own `Drop`
/// implementation without needing an explicit lifetime tying it back to
/// `VulkanDevice` — real resource cleanup, not deferred to process exit,
/// matching this project's standards elsewhere even though this
/// release's own tests are short-lived enough that it wouldn't be
/// immediately visible if it leaked.
pub struct VulkanDevice {
    _entry: ash::Entry,
    pub(crate) instance: ash::Instance,
    pub(crate) device: Rc<ash::Device>,
    pub(crate) queue: vk::Queue,
    pub(crate) command_pool: vk::CommandPool,
    pub(crate) render_pass: vk::RenderPass,
    pub(crate) memory_properties: vk::PhysicalDeviceMemoryProperties,
}

/// An error creating the Vulkan backend itself (instance, device,
/// queue, ...). Resource creation *after* this succeeds is currently
/// infallible at the trait level — see [`RenderDevice`]'s own docs for
/// why that's a deliberate `v0.0.6` simplification, not an oversight.
#[derive(Debug, thiserror::Error)]
pub enum VulkanInitError {
    /// The Vulkan loader itself couldn't be found/loaded (e.g. no
    /// `libvulkan.so`/`vulkan-1.dll` present).
    #[error("failed to load the Vulkan entry point: {0}")]
    EntryLoad(#[source] ash::LoadingError),
    /// A Vulkan API call failed. Wraps `ash`'s own result type rather
    /// than re-describing every possible `vk::Result` variant.
    #[error("a Vulkan API call failed: {0}")]
    Vulkan(#[source] vk::Result),
    /// No physical device was enumerable at all — e.g. no ICD installed
    /// (this project's own sandbox needs `mesa-vulkan-drivers` for
    /// exactly this reason; see `docs/architecture/platform-abstraction.md`).
    #[error("no Vulkan physical device is available")]
    NoPhysicalDevice,
    /// No queue family on the selected physical device supports
    /// graphics operations.
    #[error("the selected physical device has no graphics-capable queue family")]
    NoGraphicsQueueFamily,
}

impl From<vk::Result> for VulkanInitError {
    fn from(result: vk::Result) -> Self {
        VulkanInitError::Vulkan(result)
    }
}

fn find_memory_type(
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    type_filter: u32,
    required_properties: vk::MemoryPropertyFlags,
) -> u32 {
    (0..memory_properties.memory_type_count)
        .find(|&i| {
            let matches_filter = (type_filter & (1 << i)) != 0;
            let has_properties = memory_properties.memory_types[i as usize]
                .property_flags
                .contains(required_properties);
            matches_filter && has_properties
        })
        .unwrap_or_else(|| {
            panic!(
                "no memory type satisfies filter {type_filter:#b} with required properties \
                 {required_properties:?} -- this indicates a real driver/hardware limitation, \
                 not something v0.0.6's scope can recover from"
            )
        })
}

/// Allocates device memory satisfying `requirements` with
/// `required_properties`, sized exactly to `requirements.size`. A single
/// `vkAllocateMemory` call per resource — real engines sub-allocate from
/// larger blocks (via `gpu-allocator`/VMA-style allocators) to avoid the
/// driver's limited `maxMemoryAllocationCount`, but `v0.0.6`'s entire
/// scope is 2-3 allocations total (one vertex buffer, one color target,
/// one readback staging buffer), nowhere near that limit. Sub-allocation
/// is real, expected future work once a render graph is actually
/// creating enough resources for it to matter.
pub(crate) fn allocate_memory(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    requirements: vk::MemoryRequirements,
    required_properties: vk::MemoryPropertyFlags,
) -> Result<vk::DeviceMemory, vk::Result> {
    let memory_type = find_memory_type(
        memory_properties,
        requirements.memory_type_bits,
        required_properties,
    );
    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    unsafe { device.allocate_memory(&alloc_info, None) }
}

impl VulkanDevice {
    /// Creates the Vulkan backend: instance, physical/logical device,
    /// graphics queue, command pool, and the single shared render pass
    /// every [`VulkanColorTarget`]/[`VulkanPipeline`] this device creates
    /// will use (a fixed, crate-internal color format -- see COLOR_FORMAT).
    ///
    /// Picks the first enumerable physical device without preference —
    /// `v0.0.6`'s scope is proving the RHI trait and this backend work
    /// at all, on whatever device is available (in this sandbox,
    /// `llvmpipe`'s software rasterizer — see
    /// `docs/roadmap/v0.0.6-roadmap.md`'s "Verified, not assumed").
    /// Real device selection (preferring a discrete GPU, checking for
    /// required features/extensions) is real future work once there's
    /// more than one kind of device to choose between in practice.
    pub fn new() -> Result<Self, VulkanInitError> {
        let entry = unsafe { ash::Entry::load() }.map_err(VulkanInitError::EntryLoad)?;

        let app_name = CString::new("canary").expect("static string has no interior NUL");
        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .api_version(vk::API_VERSION_1_1);
        let instance_ci = vk::InstanceCreateInfo::default().application_info(&app_info);
        let instance = unsafe { entry.create_instance(&instance_ci, None) }?;

        let physical_device = unsafe { instance.enumerate_physical_devices() }?
            .into_iter()
            .next()
            .ok_or(VulkanInitError::NoPhysicalDevice)?;

        let queue_family_index =
            unsafe { instance.get_physical_device_queue_family_properties(physical_device) }
                .iter()
                .position(|props| props.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .ok_or(VulkanInitError::NoGraphicsQueueFamily)? as u32;

        let queue_priorities = [1.0f32];
        let queue_ci = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priorities);
        let queue_cis = [queue_ci];
        let device_ci = vk::DeviceCreateInfo::default().queue_create_infos(&queue_cis);
        let device = Rc::new(unsafe { instance.create_device(physical_device, &device_ci, None) }?);
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };

        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };

        let command_pool_ci = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = unsafe { device.create_command_pool(&command_pool_ci, None) }?;

        let render_pass = create_render_pass(&device)?;

        Ok(Self {
            _entry: entry,
            instance,
            device,
            queue,
            command_pool,
            render_pass,
            memory_properties,
        })
    }
}

fn create_render_pass(device: &ash::Device) -> Result<vk::RenderPass, vk::Result> {
    let attachment = vk::AttachmentDescription::default()
        .format(COLOR_FORMAT)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        // Ending directly in TRANSFER_SRC_OPTIMAL means
        // read_color_target_rgba8's copy needs no separate layout-
        // transition barrier -- the render pass itself performs it.
        .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
    let attachments = [attachment];

    let color_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let color_refs = [color_ref];
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs);
    let subpasses = [subpass];

    // Explicit dependency from "before the render pass" to the color
    // attachment output stage, since there's no prior pass in this
    // one-pass-per-frame scope to implicitly order against.
    let dependency = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
    let dependencies = [dependency];

    let render_pass_ci = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&dependencies);
    unsafe { device.create_render_pass(&render_pass_ci, None) }
}

impl RenderDevice for VulkanDevice {
    type Buffer = VulkanBuffer;
    type ColorTarget = VulkanColorTarget;
    type Pipeline = VulkanPipeline;
    type CommandEncoder<'a> = VulkanCommandEncoder<'a>;

    fn create_buffer(&self, desc: &BufferDescriptor<'_>) -> Self::Buffer {
        VulkanBuffer::new(self, desc)
    }

    fn create_color_target(&self, desc: &ColorTargetDescriptor) -> Self::ColorTarget {
        VulkanColorTarget::new(self, desc)
    }

    fn create_pipeline(&self, desc: &PipelineDescriptor<'_>) -> Self::Pipeline {
        VulkanPipeline::new(self, desc)
    }

    fn create_command_encoder(&self) -> Self::CommandEncoder<'_> {
        VulkanCommandEncoder::new(self)
    }

    fn submit_and_wait(&self, encoder: Self::CommandEncoder<'_>) {
        encoder.submit_and_wait();
    }

    fn read_color_target_rgba8(&self, target: &Self::ColorTarget) -> Vec<u8> {
        target.read_rgba8(self)
    }
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        unsafe {
            // Resources created via this device (buffers, color targets,
            // pipelines) must already be dropped by this point -- Rust's
            // ownership rules enforce this automatically, since they all
            // hold their own `Rc<ash::Device>` clone rather than a
            // borrow of `VulkanDevice` itself, so nothing here needs to
            // (or safely could) reach into them.
            self.device.destroy_render_pass(self.render_pass, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}
