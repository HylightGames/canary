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
use canary_render::{
    BufferDescriptor, ColorTargetDescriptor, PipelineDescriptor, RenderDevice, TextureDescriptor,
};

use crate::buffer::VulkanBuffer;
use crate::color_target::VulkanColorTarget;
use crate::encoder::VulkanCommandEncoder;
use crate::pipeline::VulkanPipeline;
use crate::texture::VulkanTexture;

#[cfg(feature = "presentation")]
use canary_platform::surface::WindowHandles;
#[cfg(feature = "presentation")]
use canary_render::presentation::PresentationError;

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
/// resource this creates ([`VulkanBuffer`], [`VulkanTexture`],
/// [`VulkanColorTarget`], [`VulkanPipeline`]) can clean itself up in
/// its own `Drop` implementation without needing an explicit lifetime
/// tying it back to `VulkanDevice`. The same `Rc` makes this type
/// `!Send`: the device and its resources stay on the thread that
/// created them and never cross into `Schedule` workers or
/// `Subsystem::tick` threads.
///
/// # Drop order contract
///
/// `VulkanDevice` must be dropped **after** every resource created
/// from it: this `Drop` impl destroys the render pass, command pool,
/// device, and instance unconditionally, and the `Rc` only keeps the
/// host-side `ash::Device` handle alive -- it cannot defer the
/// `destroy_device` call itself. Dropping the device while a buffer,
/// texture, target, or pipeline still exists destroys the very `VkDevice`
/// those resources' own `Drop` impls then call into (use-after-
/// destroy). Debug builds fail loudly on this via the
/// `debug_assert!` below; release builds cannot detect it, so treat
/// "resources first, device last" as a hard ordering rule at every
/// call site.
pub struct VulkanDevice {
    _entry: ash::Entry,
    pub(crate) instance: ash::Instance,
    /// The selected physical device, kept for surface queries (surface
    /// capabilities, formats, present modes) that presentation performs
    /// long after construction. Never destroyed explicitly — physical
    /// devices are owned by the instance, not by us. Present only with
    /// the presentation feature (its only reader).
    #[cfg(feature = "presentation")]
    pub(crate) physical_device: vk::PhysicalDevice,
    pub(crate) device: Rc<ash::Device>,
    pub(crate) queue: vk::Queue,
    pub(crate) command_pool: vk::CommandPool,
    pub(crate) render_pass: vk::RenderPass,
    pub(crate) memory_properties: vk::PhysicalDeviceMemoryProperties,
    /// The one descriptor-set layout every [`VulkanTexture`] allocates
    /// its set from and every textured pipeline includes.
    ///
    /// Shared (created once here, like the single shared render pass)
    /// so that texture sets and textured pipelines always agree: a
    /// per-pipeline layout would need cross-object compatibility
    /// bookkeeping this release's single-texture scope has no use for.
    /// Binding 0 is the 2D sampled image, binding 1 is its sampler —
    /// see [`canary_render::RenderDevice::create_textured_pipeline`]'s
    /// contract docs for why two bindings carry one texture.
    pub(crate) texture_set_layout: vk::DescriptorSetLayout,
    /// Debug messenger torn down with the device. `None` in release
    /// builds and whenever `VK_LAYER_KHRONOS_validation` is absent
    /// (mesa/llvmpipe CI has no validation layers) — validation is
    /// opportunistic diagnostics, never a behavior gate.
    debug_messenger: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
}

/// An error creating the Vulkan backend itself (instance, device,
/// queue, ...). Resource creation *after* this succeeds is currently
/// infallible at the trait level — see [`RenderDevice`]'s own docs for
/// why that's a deliberate `v0.0.6` simplification, not an oversight.
///
/// Backend-facing boundary rule (see `crate` docs): no `ash`/`vk::*`
/// types appear here. Both fallible variants below erase the
/// third-party error into an owned code/message pair at construction,
/// so downstream crates match on and display this error without ever
/// naming `ash` types.
#[derive(Debug, thiserror::Error)]
pub enum VulkanInitError {
    /// The Vulkan loader itself couldn't be found/loaded (e.g. no
    /// `libvulkan.so`/`vulkan-1.dll` present).
    #[error("failed to load the Vulkan entry point: {0}")]
    EntryLoad(String),
    /// A Vulkan API call failed, with its raw result code preserved
    /// for diagnosis.
    #[error("a Vulkan API call failed: {message} ({code})")]
    Vulkan {
        /// Raw `vk::Result` code (`as_raw`), kept so diagnostics can
        /// name the exact failure without depending on `ash` types.
        code: i32,
        /// Human-readable rendering of the result at construction time.
        message: String,
    },
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

impl VulkanInitError {
    /// Erases a `vk::Result` at the backend boundary: the only
    /// constructor for the [`VulkanInitError::Vulkan`] variant, so no
    /// call site names `ash` types in this error's construction either.
    /// (`From<vk::Result>` is deliberately *not* implemented — a
    /// blanket `From` would re-admit third-party types into every `?`
    /// site's inferred bounds. Call `.map_err(...)` explicitly.)
    fn from_vk(result: vk::Result) -> Self {
        Self::Vulkan {
            code: result.as_raw(),
            message: result.to_string(),
        }
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
    // SAFETY: size/type validated by the caller-provided requirements;
    // error-or-handle return, no null path.
    unsafe { device.allocate_memory(&alloc_info, None) }
}

/// Owned strings backing the debug-only validation request, if any.
/// Module scope: Rust forbids item definitions inside `impl` blocks,
/// and these must outlive the `instance_ci` built from them in `new()`.
struct ValidationStrings {
    layer_names: Vec<CString>,
    extension_names: Vec<CString>,
}

/// The shared tail both constructors assemble: everything after the
/// logical device exists. Split out so [`VulkanDevice::new`] and the
/// presentation constructor build it identically.
struct DeviceTail {
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    render_pass: vk::RenderPass,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    texture_set_layout: vk::DescriptorSetLayout,
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
        // SAFETY for the initialization sequence below as a whole: every
        // call operates on handles obtained lines above from the same
        // loader/instance/device chain, with fully specified
        // create-infos and no chained extension structs; each fallible
        // call returns `Err` (propagated with `?` into `VulkanInitError`)
        // rather than a null handle on failure. Per-call notes below
        // cover only what differs per call.
        let entry = Self::load_entry()?;
        let (instance, validation_enabled) = Self::create_instance(&entry, &[])?;
        let debug_messenger =
            Self::create_messenger_if_enabled(&entry, &instance, validation_enabled);
        let (physical_device, queue_family_index) = Self::pick_first_graphics_device(&instance)?;
        let device =
            Self::create_logical_device(&instance, physical_device, queue_family_index, &[])?;
        let tail = Self::assemble_tail(&instance, &device, physical_device, queue_family_index)?;
        Ok(Self {
            _entry: entry,
            instance,
            #[cfg(feature = "presentation")]
            physical_device,
            device,
            queue: tail.queue,
            command_pool: tail.command_pool,
            render_pass: tail.render_pass,
            memory_properties: tail.memory_properties,
            texture_set_layout: tail.texture_set_layout,
            debug_messenger,
        })
    }

    /// The crate-visible loader, for building extension loaders (surface,
    /// swapchain, debug) against this device's instance without
    /// re-loading. Extension loaders are free to construct — this just
    /// avoids a second `Entry::load`.
    #[cfg(feature = "presentation")]
    pub(crate) fn entry(&self) -> &ash::Entry {
        &self._entry
    }

    /// Loads the Vulkan loader. Split out so the presentation
    /// constructor shares the exact same entry point (and error) as
    /// [`VulkanDevice::new`].
    fn load_entry() -> Result<ash::Entry, VulkanInitError> {
        // SAFETY: loads the Vulkan loader library; no handles involved,
        // error-or-entry return.
        unsafe { ash::Entry::load() }.map_err(|e| VulkanInitError::EntryLoad(e.to_string()))
    }

    /// Creates the instance, enabling the debug-only validation layers
    /// (see [`VulkanDevice::maybe_install_validation`] for the contract)
    /// plus `extra_extensions` — empty for [`VulkanDevice::new`], the
    /// surface extensions for presentation. Returns whether validation
    /// was actually enabled, so the caller can decide about the debug
    /// messenger. The owned `CString`s live only for this call (Vulkan
    /// copies what it needs during `create_instance`).
    fn create_instance(
        entry: &ash::Entry,
        extra_extensions: &[*const std::os::raw::c_char],
    ) -> Result<(ash::Instance, bool), VulkanInitError> {
        let app_name = CString::new("canary").expect("static string has no interior NUL");
        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .api_version(vk::API_VERSION_1_1);
        let validation = Self::maybe_install_validation(entry);
        let validation_layer_ptrs: Vec<*const std::os::raw::c_char> = validation
            .as_ref()
            .map(|owned| owned.layer_names.iter().map(|name| name.as_ptr()).collect())
            .unwrap_or_default();
        let mut extension_ptrs: Vec<*const std::os::raw::c_char> = validation
            .as_ref()
            .map(|owned| {
                owned
                    .extension_names
                    .iter()
                    .map(|name| name.as_ptr())
                    .collect()
            })
            .unwrap_or_default();
        extension_ptrs.extend_from_slice(extra_extensions);
        let validation_enabled = validation.is_some();
        let instance_ci = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&validation_layer_ptrs)
            .enabled_extension_names(&extension_ptrs);
        // SAFETY: fully specified create-info with live string backing;
        // error-or-handle return.
        let instance = unsafe { entry.create_instance(&instance_ci, None) }
            .map_err(VulkanInitError::from_vk)?;
        Ok((instance, validation_enabled))
    }

    /// Installs the validation debug messenger when `enabled` (debug
    /// builds with layers present), `None` otherwise. Shared by both
    /// constructors so the presentation path gets the same fail-loud
    /// validation as the offscreen one.
    fn create_messenger_if_enabled(
        entry: &ash::Entry,
        instance: &ash::Instance,
        enabled: bool,
    ) -> Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)> {
        #[cfg(debug_assertions)]
        {
            if enabled {
                Self::create_debug_messenger(entry, instance)
            } else {
                None
            }
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = (entry, instance, enabled);
            None
        }
    }

    /// Destroys a messenger created by
    /// [`VulkanDevice::create_messenger_if_enabled`]. Used on error
    /// paths where the owning device was never assembled (the assembled
    /// device's `Drop` handles the success path instead).
    #[cfg(feature = "presentation")]
    fn destroy_messenger(
        messenger: &Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    ) {
        if let Some((ref debug_utils, handle)) = messenger {
            // SAFETY: live loader/messenger pair created together above,
            // destroyed exactly once here.
            unsafe {
                debug_utils.destroy_debug_utils_messenger(*handle, None);
            }
        }
    }

    /// Picks the first enumerable physical device and its first
    /// graphics-capable queue family — exactly [`VulkanDevice::new`]'s
    /// long-standing behavior, split out so the presentation
    /// constructor (which selects differently) shares the tail below.
    fn pick_first_graphics_device(
        instance: &ash::Instance,
    ) -> Result<(vk::PhysicalDevice, u32), VulkanInitError> {
        // SAFETY: read-only enumeration against a live instance; the
        // family index is proven below before use.
        let physical_device = unsafe { instance.enumerate_physical_devices() }
            .map_err(VulkanInitError::from_vk)?
            .into_iter()
            .next()
            .ok_or(VulkanInitError::NoPhysicalDevice)?;

        let queue_family_position =
            unsafe { instance.get_physical_device_queue_family_properties(physical_device) }
                .iter()
                .position(|props| props.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .ok_or(VulkanInitError::NoGraphicsQueueFamily)?;
        // `position` is an index into a just-enumerated `Vec` — far below
        // `u32::MAX` on any real device; the error below is unreachable
        // in practice but keeps a (lossy) `as` cast out of the codebase.
        let queue_family_index = u32::try_from(queue_family_position)
            .map_err(|_| VulkanInitError::NoGraphicsQueueFamily)?;

        Ok((physical_device, queue_family_index))
    }

    /// Creates the logical device with one queue from `queue_family_index`,
    /// enabling `device_extensions` (empty for [`VulkanDevice::new`],
    /// the swapchain extension for presentation).
    fn create_logical_device(
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        queue_family_index: u32,
        device_extensions: &[*const std::os::raw::c_char],
    ) -> Result<Rc<ash::Device>, VulkanInitError> {
        let queue_priorities = [1.0f32];
        let queue_ci = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&queue_priorities);
        let queue_cis = [queue_ci];
        let device_ci = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_cis)
            .enabled_extension_names(device_extensions);
        // SAFETY: `queue_family_index` was proven by the caller (either
        // picker above errors otherwise) and queue 0 of any family
        // always exists; fully specified create-info otherwise.
        let device = unsafe { instance.create_device(physical_device, &device_ci, None) }
            .map_err(VulkanInitError::from_vk)?;
        Ok(Rc::new(device))
    }

    /// The shared tail of both constructors: graphics queue, memory
    /// properties, command pool, shared offscreen render pass, and
    /// shared texture-set layout.
    fn assemble_tail(
        instance: &ash::Instance,
        device: &Rc<ash::Device>,
        physical_device: vk::PhysicalDevice,
        queue_family_index: u32,
    ) -> Result<DeviceTail, VulkanInitError> {
        // SAFETY: `queue_family_index` was proven by the caller and
        // queue 0 always exists; the device is live.
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };

        // SAFETY: read-only query against a live instance/device pair.
        let memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };

        let command_pool_ci = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: fully specified create-info against a live device;
        // error-or-handle return.
        let command_pool = unsafe { device.create_command_pool(&command_pool_ci, None) }
            .map_err(VulkanInitError::from_vk)?;

        let render_pass = create_render_pass(device).map_err(VulkanInitError::from_vk)?;

        let texture_set_layout =
            create_texture_set_layout(device).map_err(VulkanInitError::from_vk)?;

        Ok(DeviceTail {
            queue,
            command_pool,
            render_pass,
            memory_properties,
            texture_set_layout,
        })
    }

    /// Creates the Vulkan backend for windowed presentation: instance
    /// with the window's required surface extensions, a window surface,
    /// and the best graphics-*and*-present-capable adapter with the
    /// swapchain device extension enabled.
    ///
    /// Returns the device plus the created surface. The surface is
    /// *not* owned by the device (its `Drop` would not know when to
    /// destroy it relative to the presenter's swapchain): the caller —
    /// the presenter — takes ownership and destroys it before the
    /// device. On any error, everything created so far (surface,
    /// messenger, instance) is destroyed before returning, so no
    /// half-built chain leaks.
    ///
    /// Window-handle extraction happened before this call (in
    /// `canary-platform`): `handles` is Canary-owned data, so no
    /// third-party type appears here.
    #[cfg(feature = "presentation")]
    pub(crate) fn new_for_presentation(
        handles: &WindowHandles,
    ) -> Result<(Self, vk::SurfaceKHR), PresentationError> {
        let entry =
            Self::load_entry().map_err(|e| PresentationError::Initialization(e.to_string()))?;
        let display = crate::surface::rebuild_display(handles)?;
        let required = crate::surface::required_extensions(display)?;
        let (instance, validation_enabled) = Self::create_instance(&entry, required)
            .map_err(|e| PresentationError::Initialization(e.to_string()))?;
        Self::new_for_presentation_inner(entry, instance, validation_enabled, handles)
    }

    /// The fallible middle of [`VulkanDevice::new_for_presentation`]:
    /// messenger, surface, adapter selection, logical device, tail.
    /// Owns `entry`/`instance` while running (moving them into the
    /// returned `Self` on success); every error path destroys what it
    /// created — messenger, surface where built, then the instance —
    /// before returning.
    #[cfg(feature = "presentation")]
    fn new_for_presentation_inner(
        entry: ash::Entry,
        instance: ash::Instance,
        validation_enabled: bool,
        handles: &WindowHandles,
    ) -> Result<(Self, vk::SurfaceKHR), PresentationError> {
        let debug_messenger =
            Self::create_messenger_if_enabled(&entry, &instance, validation_enabled);
        let surface = match crate::surface::create_surface(&entry, &instance, handles) {
            Ok(surface) => surface,
            Err(error) => {
                Self::destroy_messenger(&debug_messenger);
                Self::destroy_instance(&instance);
                return Err(error);
            }
        };
        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
        let (physical_device, queue_family_index) =
            match crate::surface::pick_presentable_device(&instance, &surface_loader, surface) {
                Ok(selected) => selected,
                Err(error) => {
                    Self::destroy_surface(&surface_loader, surface);
                    Self::destroy_messenger(&debug_messenger);
                    Self::destroy_instance(&instance);
                    return Err(error);
                }
            };
        let swapchain_extension = [ash::khr::swapchain::NAME.as_ptr()];
        let device = match Self::create_logical_device(
            &instance,
            physical_device,
            queue_family_index,
            &swapchain_extension,
        ) {
            Ok(device) => device,
            Err(error) => {
                Self::destroy_surface(&surface_loader, surface);
                Self::destroy_messenger(&debug_messenger);
                Self::destroy_instance(&instance);
                return Err(PresentationError::Initialization(error.to_string()));
            }
        };
        let tail =
            match Self::assemble_tail(&instance, &device, physical_device, queue_family_index) {
                Ok(tail) => tail,
                Err(error) => {
                    Self::destroy_surface(&surface_loader, surface);
                    Self::destroy_messenger(&debug_messenger);
                    // SAFETY: `device` is the sole `Rc` owner here (no
                    // resource holds a clone yet); destroying the handle
                    // directly is exact-once, and the `Rc` itself drops
                    // unreferenced after.
                    unsafe {
                        device.destroy_device(None);
                    }
                    Self::destroy_instance(&instance);
                    return Err(PresentationError::Initialization(error.to_string()));
                }
            };
        Ok((
            Self {
                _entry: entry,
                instance,
                physical_device,
                device,
                queue: tail.queue,
                command_pool: tail.command_pool,
                render_pass: tail.render_pass,
                memory_properties: tail.memory_properties,
                texture_set_layout: tail.texture_set_layout,
                debug_messenger,
            },
            surface,
        ))
    }

    /// Destroys an owned instance on a constructor error path (the
    /// assembled device's `Drop` handles the success path instead).
    #[cfg(feature = "presentation")]
    fn destroy_instance(instance: &ash::Instance) {
        // SAFETY: instance owned here, never moved into a `Self` on this
        // path, destroyed exactly once.
        unsafe {
            instance.destroy_instance(None);
        }
    }

    /// Destroys an owned surface on a constructor error path.
    #[cfg(feature = "presentation")]
    fn destroy_surface(loader: &ash::khr::surface::Instance, surface: vk::SurfaceKHR) {
        // SAFETY: surface owned here, loader live, destroyed exactly
        // once; the instance outlives this call (destroyed after, above).
        unsafe {
            loader.destroy_surface(surface, None);
        }
    }

    /// Attempts `VK_LAYER_KHRONOS_validation` in debug builds; always
    /// `None` in release and whenever the layer is absent. Validation
    /// is opportunistic diagnostics, never a behavior gate: absence
    /// changes nothing except that misuses stay silent on drivers that
    /// tolerate them (llvmpipe does — which is exactly why the
    /// host-side guards elsewhere in this backend exist regardless).
    #[cfg(debug_assertions)]
    fn maybe_install_validation(entry: &ash::Entry) -> Option<ValidationStrings> {
        // SAFETY: read-only enumeration against a live loader; no
        // handles created, nothing to free.
        let available = unsafe { entry.enumerate_instance_layer_properties() }.ok()?;
        let wanted: &[u8] = b"VK_LAYER_KHRONOS_validation";
        let present = available.iter().any(|properties| {
            // `layer_name` is a fixed-size null-terminated C array.
            let raw = &properties.layer_name as *const _ as *const std::os::raw::c_char;
            // SAFETY: Vulkan guarantees NUL-termination within the
            // array bounds for enumerated properties.
            let name = unsafe { std::ffi::CStr::from_ptr(raw).to_bytes() };
            name == wanted
        });
        if !present {
            return None;
        }
        Some(ValidationStrings {
            layer_names: vec![CString::new("VK_LAYER_KHRONOS_validation")
                .expect("static string has no interior NUL")],
            extension_names: vec![
                CString::new("VK_EXT_debug_utils").expect("static string has no interior NUL")
            ],
        })
    }

    /// Release counterpart: validation layers are never installed in
    /// release builds (no behavior gate, no diagnostics cost), so there
    /// is nothing to own and no `CString`s to keep alive. Present so
    /// `new()` compiles identically in both profiles.
    #[cfg(not(debug_assertions))]
    fn maybe_install_validation(_entry: &ash::Entry) -> Option<ValidationStrings> {
        None
    }

    /// Creates the debug messenger reporting validation errors. Only
    /// called when [`VulkanDevice::maybe_install_validation`] returned
    /// `Some` (debug builds with layers present).
    #[cfg(debug_assertions)]
    fn create_debug_messenger(
        entry: &ash::Entry,
        instance: &ash::Instance,
    ) -> Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)> {
        let debug_utils = ash::ext::debug_utils::Instance::new(entry, instance);
        let create_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(
                vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                    | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
            )
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                    | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                    | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
            )
            .pfn_user_callback(Some(validation_callback));
        // SAFETY: `create_info` is fully specified with a valid
        // function-pointer callback; the loader/instance are live.
        // A failure here degrades to no-messenger (the host-side
        // guards remain the enforcement), never to a broken device.
        let messenger = unsafe {
            debug_utils
                .create_debug_utils_messenger(&create_info, None)
                .ok()?
        };
        Some((debug_utils, messenger))
    }
}

/// Validation-layer callback: validation *errors* fail loudly (a test
/// or run that triggers one has recorded invalid API use), warnings
/// and below are ignored — the layers are noisier than this backend's
/// scope warrants, and everything actionable arrives as an error.
// SAFETY: `p_callback_data` is valid for the call duration by Vulkan
// contract when non-null; the callback touches no shared state and
// returns `FALSE` (never vetoes the call). Loudness is a hard abort,
// never a panic: this callback runs on the driver's side of an FFI
// boundary, and unwinding (what `panic!` does) across that boundary
// into C code is undefined behavior. `abort` keeps the fail-loud
// contract without an unwind crossing the boundary.
#[cfg(debug_assertions)]
unsafe extern "system" fn validation_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _user_data: *mut std::ffi::c_void,
) -> vk::Bool32 {
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        let message = if p_callback_data.is_null() {
            "<null callback data>".to_string()
        } else {
            // SAFETY: guarded by the null check directly above.
            unsafe {
                std::ffi::CStr::from_ptr((*p_callback_data).p_message)
                    .to_string_lossy()
                    .into_owned()
            }
        };
        eprintln!("Vulkan validation error: {message}");
        std::process::abort();
    }
    vk::FALSE
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
    // SAFETY: fully specified single-subpass description against a
    // live device; error-or-handle return.
    unsafe { device.create_render_pass(&render_pass_ci, None) }
}

fn create_texture_set_layout(device: &ash::Device) -> Result<vk::DescriptorSetLayout, vk::Result> {
    // The single-texture layout: binding 0 is the sampled image,
    // binding 1 is its sampler, both visible to the fragment stage.
    // Split (not combined) because WGSL `texture_2d` + `sampler`
    // compile to separate descriptors — see the RHI trait docs.
    let bindings = [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT),
    ];
    let layout_ci = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    // SAFETY: two fully specified bindings; destroyed in `Drop`.
    unsafe { device.create_descriptor_set_layout(&layout_ci, None) }
}

impl RenderDevice for VulkanDevice {
    type Buffer = VulkanBuffer;
    type ColorTarget = VulkanColorTarget;
    type Pipeline = VulkanPipeline;
    type Texture = VulkanTexture;
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

    fn create_texture(&self, desc: &TextureDescriptor<'_>) -> Self::Texture {
        VulkanTexture::new(self, desc)
    }

    fn create_textured_pipeline(&self, desc: &PipelineDescriptor<'_>) -> Self::Pipeline {
        VulkanPipeline::new_textured(self, desc)
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
        // See the drop-order contract on `VulkanDevice`'s own docs:
        // every live resource holds one `Rc` clone of `device`, so a
        // count above 1 here means a buffer, texture, target, or
        // pipeline still exists and is about to be left pointing at a
        // destroyed `VkDevice`. Loud in debug; callers must uphold the
        // order in release.
        //
        // Unconditional (not `debug_assert`): dropping the device while
        // resources created from it still exist destroys the `VkDevice`
        // those resources' own `Drop` impls then call into — use after
        // destroy from 100% safe caller code. A loud panic in every
        // profile beats silent Vulkan UB that only some drivers punish.
        if Rc::strong_count(&self.device) != 1 {
            panic!(
                "VulkanDevice dropped while resources created from it still exist; \
                 drop all buffers, textures, color targets, and pipelines first"
            );
        }
        // SAFETY: every handle below is owned here (created in `new`,
        // destroyed exactly once here) in reverse-creation order, and
        // the unconditional count check above verified no live resource
        // still holds the device; callers uphold the documented
        // resources-first/device-last order in every profile.
        unsafe {
            // Debug messenger first: it reports on everything below
            // while those objects still exist to be reported about.
            if let Some((ref debug_utils, messenger)) = self.debug_messenger {
                debug_utils.destroy_debug_utils_messenger(messenger, None);
            }
            self.device
                .destroy_descriptor_set_layout(self.texture_set_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}
