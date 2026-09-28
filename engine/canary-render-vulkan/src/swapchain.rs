// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The `surface → swapchain` half of the presentation seam: swapchain
//! creation, destruction, and recreation with old-swapchain chaining,
//! plus the clear-only render pass and per-image framebuffers the
//! cleared-frame proof draws into.
//!
//! Negotiation policy lives in `canary-render`'s presentation module as
//! pure functions; this module converts native query results into that
//! vocabulary, calls it, and converts the answers back. Every native
//! result code is erased into [`PresentationError`] at the boundary.

use ash::vk;
use canary_render::presentation::{
    choose_surface_format_index, is_suspended_extent, negotiate_image_count, resolve_extent,
    PresentationError, SurfaceFormat,
};

use crate::device::VulkanDevice;
use crate::surface::surface_format_of;

/// A window swapchain with its images, views, present render pass, and
/// per-image framebuffers.
///
/// Owned by the presenter, which destroys it (via
/// [`VulkanSwapchain::destroy_surface_and_objects`]) before the device:
/// every handle here dies with `VkDevice`/`VkInstance`, so dropping the
/// device first would leave use-after-destroy behind. Recreation keeps
/// the surface and chains the old swapchain into the new creation (the
/// driver then retires the old images only once nothing references
/// them).
pub(crate) struct VulkanSwapchain {
    surface: vk::SurfaceKHR,
    loader: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    render_pass: vk::RenderPass,
    /// The negotiated surface format in Canary-owned codes (the native
    /// values are converted back at each use site via `from_raw`, so no
    /// backend type crosses this struct's boundary outward).
    format: SurfaceFormat,
    extent: vk::Extent2D,
}

impl VulkanSwapchain {
    /// Number of presentable images. Returned as `usize` (a `Vec` len)
    /// so no narrowing conversion ever touches this path.
    pub(crate) fn image_count(&self) -> usize {
        self.images.len()
    }

    /// Current swapchain extent in pixels.
    pub(crate) fn extent(&self) -> (u32, u32) {
        (self.extent.width, self.extent.height)
    }

    /// The swapchain handle, for acquire/present calls.
    pub(crate) fn handle(&self) -> vk::SwapchainKHR {
        self.swapchain
    }

    /// The device extension loader bound to this swapchain's device.
    pub(crate) fn loader(&self) -> &ash::khr::swapchain::Device {
        &self.loader
    }

    /// The present render pass, for clear-only recording.
    pub(crate) fn render_pass(&self) -> vk::RenderPass {
        self.render_pass
    }

    /// The framebuffer drawing into swapchain image `index`, or `None`
    /// when the driver handed back an index outside the swapchain.
    pub(crate) fn framebuffer_at(&self, index: usize) -> Option<vk::Framebuffer> {
        self.framebuffers.get(index).copied()
    }

    /// The swapchain image at `index`, or `None` when the driver handed
    /// back an index outside the swapchain. For content blits (see the
    /// presenter's content-frame path).
    pub(crate) fn image_at(&self, index: usize) -> Option<vk::Image> {
        self.images.get(index).copied()
    }

    /// The negotiated surface format, in Canary-owned codes.
    pub(crate) fn surface_format(&self) -> SurfaceFormat {
        self.format
    }

    /// Builds the swapchain for `surface` at the window's requested
    /// extent, negotiating format, image count, and extent against the
    /// surface capabilities per Canary policy.
    pub(crate) fn create(
        device: &VulkanDevice,
        surface: vk::SurfaceKHR,
        window_extent: (u32, u32),
    ) -> Result<Self, PresentationError> {
        Self::build(device, surface, window_extent, vk::SwapchainKHR::null())
    }

    /// Rebuilds the swapchain in place (resize, mismatch flag, ...):
    /// waits for in-flight work, builds the replacement chained to the
    /// old swapchain, then destroys the old objects. The surface is
    /// kept. A failed rebuild leaves the old swapchain untouched and
    /// valid, so the next frame simply retries.
    pub(crate) fn recreate(
        &mut self,
        device: &VulkanDevice,
        window_extent: (u32, u32),
    ) -> Result<(), PresentationError> {
        // SAFETY: drains the single-frame submit-and-wait discipline
        // (nothing is ever in flight past a frame boundary here), so no
        // command buffer references the objects destroyed below.
        unsafe { device.device.queue_wait_idle(device.queue) }.map_err(|result| {
            if result == vk::Result::ERROR_DEVICE_LOST {
                PresentationError::DeviceLost
            } else {
                PresentationError::SwapchainCreation {
                    code: result.as_raw(),
                    message: std::format!("waiting for idle before swapchain recreation: {result}"),
                }
            }
        })?;
        let fresh = Self::build(device, self.surface, window_extent, self.swapchain)?;
        self.destroy_swapchain_objects(device);
        *self = fresh;
        Ok(())
    }

    /// Destroys the swapchain and everything built from it, then the
    /// surface itself. Assumes the device is idle (the caller waits —
    /// see `recreate` and the presenter's `Drop`).
    pub(crate) fn destroy_surface_and_objects(&mut self, device: &VulkanDevice) {
        self.destroy_swapchain_objects(device);
        let surface_loader = ash::khr::surface::Instance::new(device.entry(), &device.instance);
        // SAFETY: surface owned here (created by the device constructor,
        // moved through this struct ever since), loader live, destroyed
        // exactly once; the instance outlives this call by drop order.
        unsafe {
            surface_loader.destroy_surface(self.surface, None);
        }
        self.surface = vk::SurfaceKHR::null();
    }

    /// Destroys exactly the swapchain-derived objects (images are owned
    /// by the swapchain handle itself and need no call): framebuffers,
    /// views, present render pass, then the swapchain handle.
    fn destroy_swapchain_objects(&mut self, device: &VulkanDevice) {
        // SAFETY: every handle below is owned here (created in `build`,
        // destroyed exactly once here), and the caller guarantees an
        // idle device, so nothing references them anymore.
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                device.device.destroy_framebuffer(framebuffer, None);
            }
            for view in self.views.drain(..) {
                device.device.destroy_image_view(view, None);
            }
            // A null render pass/swapchain means "already destroyed"
            // (see `destroy_surface_and_objects` nulling the surface;
            // the same guard style here keeps double-destroy impossible
            // even if destroy order ever changes).
            if self.render_pass != vk::RenderPass::null() {
                device.device.destroy_render_pass(self.render_pass, None);
                self.render_pass = vk::RenderPass::null();
            }
            if self.swapchain != vk::SwapchainKHR::null() {
                self.loader.destroy_swapchain(self.swapchain, None);
                self.swapchain = vk::SwapchainKHR::null();
            }
            self.images.clear();
        }
    }

    /// Negotiates and builds everything, chaining `old` (possibly null)
    /// into the creation. On error every object created so far is
    /// destroyed before returning — including `old`'s replacement
    /// chain position being left alone (a failed build never touches
    /// `old` itself, which stays valid for a retry).
    fn build(
        device: &VulkanDevice,
        surface: vk::SurfaceKHR,
        window_extent: (u32, u32),
        old: vk::SwapchainKHR,
    ) -> Result<Self, PresentationError> {
        let surface_loader = ash::khr::surface::Instance::new(device.entry(), &device.instance);
        // SAFETY for the query block below: read-only surface queries
        // against the selected physical device and a live surface; each
        // fallible call maps into `PresentationError`, never a null.
        let capabilities = unsafe {
            surface_loader.get_physical_device_surface_capabilities(device.physical_device, surface)
        }
        .map_err(|result| PresentationError::SwapchainCreation {
            code: result.as_raw(),
            message: std::format!("querying surface capabilities: {result}"),
        })?;
        let native_formats = unsafe {
            surface_loader.get_physical_device_surface_formats(device.physical_device, surface)
        }
        .map_err(|result| PresentationError::SwapchainCreation {
            code: result.as_raw(),
            message: std::format!("querying surface formats: {result}"),
        })?;
        let present_modes = unsafe {
            surface_loader
                .get_physical_device_surface_present_modes(device.physical_device, surface)
        }
        .map_err(|result| PresentationError::SwapchainCreation {
            code: result.as_raw(),
            message: std::format!("querying surface present modes: {result}"),
        })?;

        if !present_modes.contains(&vk::PresentModeKHR::FIFO) {
            return Err(PresentationError::SwapchainCreation {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: String::from(
                    "the surface offers no FIFO present mode (required by the Vulkan specification)",
                ),
            });
        }
        let offered: Vec<SurfaceFormat> = native_formats
            .iter()
            .map(|format| surface_format_of(*format))
            .collect();
        let format_index =
            choose_surface_format_index(&offered).ok_or(PresentationError::SwapchainCreation {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: String::from("the surface offers no formats at all"),
            })?;
        // `choose_surface_format_index` returns an index into the slice
        // it was given, which parallels `native_formats` exactly; the
        // Canary-owned copy is what this struct keeps (see `format`).
        let format = offered[format_index];
        let native_format = native_formats[format_index];

        // Content frames blit an offscreen target into the swapchain
        // image (see the presenter's content-frame path), so creation
        // demands both halves of that transfer up front: the surface
        // must allow `TRANSFER_DST` usage on its images, and the
        // negotiated format must support blit-destination. Loud errors,
        // never a silent clear-only fallback — a caller asking for
        // content frames on a surface that cannot receive them is a
        // configuration bug, not a runtime condition.
        if !capabilities
            .supported_usage_flags
            .contains(vk::ImageUsageFlags::TRANSFER_DST)
        {
            return Err(PresentationError::SwapchainCreation {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: String::from(
                    "the surface does not support TRANSFER_DST image usage: \
                     content-frame blits cannot land here",
                ),
            });
        }
        // SAFETY: read-only format-feature query against the selected
        // physical device and a format the surface just offered; returns
        // properties by value, no null path.
        let dst_features = unsafe {
            device
                .instance
                .get_physical_device_format_properties(device.physical_device, native_format.format)
        }
        .optimal_tiling_features;
        if !dst_features.contains(vk::FormatFeatureFlags::BLIT_DST) {
            return Err(PresentationError::SwapchainCreation {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: std::format!(
                    "the negotiated surface format {:?} supports no blit-destination: \
                     content-frame blits cannot land here",
                    native_format.format,
                ),
            });
        }

        let current = if capabilities.current_extent.width == u32::MAX {
            None
        } else {
            Some((
                capabilities.current_extent.width,
                capabilities.current_extent.height,
            ))
        };
        let (width, height) = resolve_extent(
            current,
            (
                capabilities.min_image_extent.width,
                capabilities.min_image_extent.height,
            ),
            (
                capabilities.max_image_extent.width,
                capabilities.max_image_extent.height,
            ),
            window_extent,
        );
        if is_suspended_extent((width, height)) {
            return Err(PresentationError::SwapchainCreation {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: String::from(
                    "resolved swapchain extent is zero (minimized): suspend instead of building",
                ),
            });
        }
        let extent = vk::Extent2D { width, height };
        let image_count =
            negotiate_image_count(capabilities.min_image_count, capabilities.max_image_count);

        let loader = ash::khr::swapchain::Device::new(&device.instance, &device.device);
        let create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(native_format.format)
            .image_color_space(native_format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(capabilities.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true)
            .old_swapchain(old);
        // SAFETY: fully specified create-info (negotiated format,
        // clamped extent, same-family exclusive sharing) against a live
        // device; error-or-handle return. `old` is either null or a
        // live swapchain of this same device.
        let swapchain =
            unsafe { loader.create_swapchain(&create_info, None) }.map_err(|result| {
                PresentationError::SwapchainCreation {
                    code: result.as_raw(),
                    message: std::format!("creating the swapchain: {result}"),
                }
            })?;

        // From here on, every error path destroys what was built so
        // far (bottom-up: views, then the swapchain handle above).
        // SAFETY: read-only image query on a live swapchain.
        let images = unsafe { loader.get_swapchain_images(swapchain) }.map_err(|result| {
            // SAFETY: swapchain owned here, loader live, destroyed
            // exactly once on this error path.
            unsafe {
                loader.destroy_swapchain(swapchain, None);
            }
            PresentationError::SwapchainCreation {
                code: result.as_raw(),
                message: std::format!("querying swapchain images: {result}"),
            }
        })?;

        let mut views = Vec::with_capacity(images.len());
        for image in &images {
            let view_ci = vk::ImageViewCreateInfo::default()
                .image(*image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(native_format.format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            // SAFETY: live swapchain image, matching negotiated format,
            // single mip/layer; error-or-handle return.
            match unsafe { device.device.create_image_view(&view_ci, None) } {
                Ok(view) => views.push(view),
                Err(result) => {
                    // SAFETY: destroys exactly the views pushed so far
                    // plus the swapchain; nothing else references them.
                    unsafe {
                        for view in views {
                            device.device.destroy_image_view(view, None);
                        }
                        loader.destroy_swapchain(swapchain, None);
                    }
                    return Err(PresentationError::SwapchainCreation {
                        code: result.as_raw(),
                        message: std::format!("creating a swapchain image view: {result}"),
                    });
                }
            }
        }

        let render_pass = match create_present_render_pass(&device.device, native_format.format) {
            Ok(pass) => pass,
            Err(result) => {
                // SAFETY: destroys exactly what `build` created so far.
                unsafe {
                    for view in views {
                        device.device.destroy_image_view(view, None);
                    }
                    loader.destroy_swapchain(swapchain, None);
                }
                return Err(PresentationError::SwapchainCreation {
                    code: result.as_raw(),
                    message: std::format!("creating the present render pass: {result}"),
                });
            }
        };

        let mut framebuffers = Vec::with_capacity(views.len());
        for view in &views {
            let attachments = [*view];
            let framebuffer_ci = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(width)
                .height(height)
                .layers(1);
            // SAFETY: view, pass, and extent are mutually compatible
            // (built lines above for exactly this pairing).
            match unsafe { device.device.create_framebuffer(&framebuffer_ci, None) } {
                Ok(framebuffer) => framebuffers.push(framebuffer),
                Err(result) => {
                    // SAFETY: destroys exactly what `build` created so far.
                    unsafe {
                        for framebuffer in framebuffers {
                            device.device.destroy_framebuffer(framebuffer, None);
                        }
                        for view in views {
                            device.device.destroy_image_view(view, None);
                        }
                        device.device.destroy_render_pass(render_pass, None);
                        loader.destroy_swapchain(swapchain, None);
                    }
                    return Err(PresentationError::SwapchainCreation {
                        code: result.as_raw(),
                        message: std::format!("creating a swapchain framebuffer: {result}"),
                    });
                }
            }
        }

        Ok(Self {
            surface,
            loader,
            swapchain,
            images,
            views,
            framebuffers,
            render_pass,
            format,
            extent,
        })
    }
}

/// The clear-only render pass for swapchain images: clears to the
/// frame's color, stores, and transitions directly to
/// `PRESENT_SRC_KHR` so no extra barrier stands between the pass and
/// the present. Separate from the device's shared offscreen pass,
/// whose fixed format never matches a negotiated swapchain format.
fn create_present_render_pass(
    device: &ash::Device,
    format: vk::Format,
) -> Result<vk::RenderPass, vk::Result> {
    let attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
    let attachments = [attachment];

    let color_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let color_refs = [color_ref];
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs);
    let subpasses = [subpass];

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
    // SAFETY: fully specified single-subpass description against a live
    // device; error-or-handle return. Destroyed with the swapchain.
    unsafe { device.create_render_pass(&render_pass_ci, None) }
}
