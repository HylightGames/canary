// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The `swapchain → present` end of the seam: [`VulkanPresenter`], which
//! turns a surface-capable window into presented cleared frames.
//!
//! Scope is the cleared-frame proof only (acquire → clear-only pass →
//! submit → present), plus the lifecycle that proof needs: minimized
//! suspend, resize/recreate, suboptimal-proceed. No render graph, no
//! depth, no textures, no multi-frame flight, no mode switching, no
//! HDR — those arrive with their own milestones, not smuggled in here.
//!
//! The presenter owns its [`VulkanDevice`] and borrows the window for
//! its whole lifetime (`&'a dyn SurfaceHandlesProvider`): the surface
//! aliases the OS window, so the window must outlive everything here,
//! and the borrow makes that a compile-time rule rather than a
//! documented wish. `VulkanDevice`'s drop-order contract extends
//! naturally — swapchain objects and the surface die in this
//! presenter's `Drop`, before the owned device's own `Drop` runs.

use ash::vk;
use canary_platform::surface::SurfaceHandlesProvider;
use canary_render::presentation::{
    is_suspended_extent, should_recreate_after_acquire, should_recreate_after_present,
    should_skip_frame_after_acquire, AcquireStatus, AcquiredImage, FrameOutcome, PresentStatus,
    PresentationError, PresentedFrame,
};

use crate::device::VulkanDevice;
use crate::surface::{map_acquire_result, map_present_result, ACQUIRE_TIMEOUT_NS};
use crate::swapchain::VulkanSwapchain;

/// Presents cleared frames to a surface-capable window: the single
/// public entry point of the presentation seam.
///
/// Build with [`VulkanPresenter::new`], then call
/// [`VulkanPresenter::present_cleared_frame`] once per frame. The
/// presenter recreates the swapchain itself whenever the window or the
/// driver asks it to (resize, generation change, suboptimal/outdated),
/// and skips frames it must not draw (minimized window, acquire
/// timeout) — callers match on [`FrameOutcome`], never on native codes.
///
/// The window is passed to [`VulkanPresenter::new`] and to every
/// [`VulkanPresenter::present_cleared_frame`] call (not stored) so
/// event polling (`&mut` on the window) and presenting (`&` on the
/// window) interleave freely in a game loop.
///
/// Caller contract (same shape as `ash-window`'s own safety contract,
/// and like [`VulkanDevice`]'s drop-order rule, enforced by
/// documentation plus loud failure rather than by the borrow
/// checker): pass the same window the presenter was built for to every
/// frame, and drop the presenter before that window. The surface
/// aliases the OS window — presenting after the window died is
/// undefined behavior the type system cannot see, and the validation
/// layers (enabled whenever present) fail loudly on it in debug builds.
///
/// Like [`VulkanDevice`], this type is `!Send`: the device and every
/// resource stay on the thread that created them.
pub struct VulkanPresenter {
    device: VulkanDevice,
    swapchain: VulkanSwapchain,
    acquire_fence: vk::Fence,
    extent: (u32, u32),
    generation: u64,
    needs_recreate: bool,
}

impl VulkanPresenter {
    /// Creates the presentation stack for `window`: device (with the
    /// window's required surface extensions and a present-capable
    /// adapter), window surface, swapchain, and acquire fence.
    ///
    /// Requires a currently visible window: a minimized (zero-extent)
    /// window at construction time is an error — retry once visible.
    /// (Minimization *during* presentation suspends per-frame instead;
    /// see [`VulkanPresenter::present_cleared_frame`].) A window with
    /// no live OS window yet is [`PresentationError::NoWindowHandles`].
    pub fn new(window: &dyn SurfaceHandlesProvider) -> Result<Self, PresentationError> {
        let handles = window
            .window_handles()
            .ok_or(PresentationError::NoWindowHandles)?;
        let extent = window
            .surface_extent()
            .ok_or(PresentationError::NoWindowHandles)?;
        if is_suspended_extent(extent) {
            return Err(PresentationError::SwapchainCreation {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: String::from(
                    "window is minimized (zero extent) at presenter creation: \
                     retry once the window is visible; minimization during \
                     presentation suspends per-frame instead",
                ),
            });
        }
        let generation = window.resize_generation();
        let (device, surface) = VulkanDevice::new_for_presentation(&handles)?;
        let mut swapchain = match VulkanSwapchain::create(&device, surface, extent) {
            Ok(swapchain) => swapchain,
            Err(error) => {
                let surface_loader =
                    ash::khr::surface::Instance::new(device.entry(), &device.instance);
                // SAFETY: surface owned here (never moved into a
                // swapchain on this path), loader live, destroyed once.
                unsafe {
                    surface_loader.destroy_surface(surface, None);
                }
                return Err(error);
            }
        };
        let fence_create_info = vk::FenceCreateInfo::default();
        // SAFETY: fully specified create-info against a live device;
        // error-or-handle return.
        let acquire_fence = match unsafe { device.device.create_fence(&fence_create_info, None) } {
            Ok(fence) => fence,
            Err(result) => {
                swapchain.destroy_surface_and_objects(&device);
                return Err(PresentationError::AcquireFailed {
                    code: result.as_raw(),
                    message: std::format!("creating the acquire fence: {result}"),
                });
            }
        };
        Ok(Self {
            device,
            swapchain,
            acquire_fence,
            extent,
            generation,
            needs_recreate: false,
        })
    }

    /// The device this presenter was built on, for future passes that
    /// draw real content through the same queue.
    pub fn device(&self) -> &VulkanDevice {
        &self.device
    }

    /// The swapchain extent frames are currently drawn at.
    pub fn extent(&self) -> (u32, u32) {
        self.extent
    }

    /// How many images the swapchain holds.
    pub fn image_count(&self) -> usize {
        self.swapchain.image_count()
    }

    /// The negotiated surface format, in Canary-owned codes — the proof
    /// test asserts these match the preferred format policy.
    pub fn surface_format(&self) -> canary_render::presentation::SurfaceFormat {
        self.swapchain.surface_format()
    }

    /// Acquires an image, clears it to `clear_color` (linear RGBA in
    /// `[0.0, 1.0]`), and presents it — one full cleared frame.
    ///
    /// `window` is the same window this presenter was built for (see
    /// the struct-level caller contract): each call re-reads its extent
    /// and resize generation for the recreate check below.
    ///
    /// Lifecycle, in order: a minimized window suspends (returns
    /// `Skipped`, touching nothing); an extent/generation change or a
    /// pending mismatch flag recreates the swapchain first (waiting
    /// idle, chaining the old swapchain); an `Outdated` acquire
    /// recreates and skips; a `Suboptimal` acquire proceeds and flags
    /// the next recreate; a `Suboptimal`/`Outdated` present flags the
    /// next recreate while still reporting this frame as presented.
    pub fn present_cleared_frame(
        &mut self,
        window: &dyn SurfaceHandlesProvider,
        clear_color: [f32; 4],
    ) -> Result<FrameOutcome, PresentationError> {
        match self.prepare_frame(window)? {
            ReadyOrSkipped::Skipped(outcome) => Ok(outcome),
            ReadyOrSkipped::Ready(ready) => {
                self.record_clear_and_submit(ready.image_index, clear_color)?;
                self.finish_present(ready)
            }
        }
    }

    /// Blits an offscreen RHI color target into the acquired swapchain
    /// image and presents it — one full content frame. The scene and UI
    /// draw through the RHI into `target` first (same device — see
    /// [`VulkanPresenter::device`]); this method only moves the finished
    /// pixels to the screen and presents.
    ///
    /// Requires a drawn target at exactly the swapchain's current extent
    /// (see [`VulkanPresenter::extent`]): a never-drawn target is a
    /// caller bug (refused loudly in debug — blitting from an
    /// `UNDEFINED`-layout image is invalid), and an extent mismatch is
    /// [`PresentationError::ContentExtentMismatch`] (recreate the target
    /// and retry — the presenter never scales or crops silently). The
    /// blit converts formats semantically (offscreen RGBA to swapchain
    /// BGRA, UNORM to sRGB); swapchain creation already verified both
    /// directions' feature support, so an unsupported surface fails at
    /// construction, not per frame. Lifecycle (suspend, recreate, skip,
    /// mismatch flags) is identical to
    /// [`VulkanPresenter::present_cleared_frame`].
    pub fn present_color_target(
        &mut self,
        window: &dyn SurfaceHandlesProvider,
        target: &crate::VulkanColorTarget,
    ) -> Result<FrameOutcome, PresentationError> {
        // Lifecycle order matters: a minimized window suspends even with
        // a stale target (Suspended, not Mismatch). Otherwise the target
        // is checked against the WINDOW's extent — what the swapchain
        // will be after prepare — so a resize reports Mismatch with no
        // side effects: no acquire, no recreate, and the recreate flag
        // stays intact for the retry that actually presents. (A post-
        // prepare re-check below nets the race where the window moves
        // between the check and the blit.)
        let extent = window
            .surface_extent()
            .ok_or(PresentationError::NoWindowHandles)?;
        if is_suspended_extent(extent) {
            return Ok(FrameOutcome::Skipped {
                status: AcquireStatus::Suspended,
            });
        }
        if (target.width, target.height) != extent {
            return Err(PresentationError::ContentExtentMismatch {
                swapchain: extent,
                content: (target.width, target.height),
            });
        }
        debug_assert!(
            target.was_drawn(),
            "present_color_target with a never-drawn target: submit at least one RHI render pass first"
        );
        match self.prepare_frame(window)? {
            ReadyOrSkipped::Skipped(outcome) => Ok(outcome),
            ReadyOrSkipped::Ready(ready) => {
                // Race net: the window may have moved between the check
                // above and this blit (prepare recreates first). A
                // mismatch here consumed the recreate already, so the
                // retry reports cleanly.
                if (target.width, target.height) != self.extent {
                    return Err(PresentationError::ContentExtentMismatch {
                        swapchain: self.extent,
                        content: (target.width, target.height),
                    });
                }
                self.record_blit_and_submit(ready.image_index, target)?;
                self.finish_present(ready)
            }
        }
    }

    /// The shared acquire-head of both frame paths: suspend, recreate,
    /// acquire, skip handling. Returns either a skip outcome or a ready
    /// image with the recreate flag the report carries.
    fn prepare_frame(
        &mut self,
        window: &dyn SurfaceHandlesProvider,
    ) -> Result<ReadyOrSkipped, PresentationError> {
        let mut swapchain_recreated = false;
        let extent = window
            .surface_extent()
            .ok_or(PresentationError::NoWindowHandles)?;
        if is_suspended_extent(extent) {
            return Ok(ReadyOrSkipped::Skipped(FrameOutcome::Skipped {
                status: AcquireStatus::Suspended,
            }));
        }
        let generation = window.resize_generation();
        if extent != self.extent || generation != self.generation || self.needs_recreate {
            self.swapchain.recreate(&self.device, extent)?;
            self.extent = extent;
            self.generation = generation;
            self.needs_recreate = false;
            swapchain_recreated = true;
        }
        let acquired = self.acquire_image()?;
        if should_skip_frame_after_acquire(acquired.status) {
            if acquired.status == AcquireStatus::Outdated {
                let extent_now = window
                    .surface_extent()
                    .ok_or(PresentationError::NoWindowHandles)?;
                if is_suspended_extent(extent_now) {
                    return Ok(ReadyOrSkipped::Skipped(FrameOutcome::Skipped {
                        status: AcquireStatus::Suspended,
                    }));
                }
                self.swapchain.recreate(&self.device, extent_now)?;
                self.extent = extent_now;
                self.generation = window.resize_generation();
                self.needs_recreate = false;
            }
            return Ok(ReadyOrSkipped::Skipped(FrameOutcome::Skipped {
                status: acquired.status,
            }));
        }
        let image_index = acquired.index.ok_or(PresentationError::AcquireFailed {
            code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
            message: String::from("acquire reported success without an image index"),
        })?;
        if should_recreate_after_acquire(acquired.status) {
            self.needs_recreate = true;
        }
        Ok(ReadyOrSkipped::Ready(ReadyImage {
            image_index,
            acquire_status: acquired.status,
            swapchain_recreated,
        }))
    }

    /// Presents a submitted image and folds the present status into the
    /// recreate flag, reporting the frame as presented. Shared tail of
    /// both frame paths.
    fn finish_present(&mut self, ready: ReadyImage) -> Result<FrameOutcome, PresentationError> {
        let present_status = self.present_image(ready.image_index)?;
        if should_recreate_after_present(present_status) {
            self.needs_recreate = true;
        }
        Ok(FrameOutcome::Presented(PresentedFrame {
            image_index: ready.image_index,
            acquire_status: ready.acquire_status,
            present_status,
            swapchain_recreated: ready.swapchain_recreated,
        }))
    }

    /// Acquires the next presentable image, waiting up to the acquire
    /// timeout on a fence (no semaphores: the single-frame
    /// submit-and-wait discipline never overlaps frames, so a fence is
    /// the whole synchronization story at this scope).
    fn acquire_image(&self) -> Result<AcquiredImage, PresentationError> {
        // SAFETY: fence owned here (created in `new`, destroyed in
        // `Drop`); resetting an unsignaled fence is a no-op, so every
        // frame starts from a known state.
        unsafe { self.device.device.reset_fences(&[self.acquire_fence]) }
            .map_err(|result| fence_error("resetting the acquire fence", result))?;
        // SAFETY: live swapchain of this device; finite timeout with a
        // real fence (never both-null); returns index-or-error, no null
        // path.
        let (image_index, suboptimal) = match unsafe {
            self.swapchain.loader().acquire_next_image(
                self.swapchain.handle(),
                ACQUIRE_TIMEOUT_NS,
                vk::Semaphore::null(),
                self.acquire_fence,
            )
        } {
            Ok(acquired) => acquired,
            Err(result) => {
                return map_acquire_result(result).map(|status| AcquiredImage {
                    index: None,
                    status,
                });
            }
        };
        // SAFETY: the fence was just handed to the acquire above on this
        // device; waiting on it is sound, and the finite timeout keeps a
        // wedged acquire from hanging the frame loop.
        match unsafe {
            self.device
                .device
                .wait_for_fences(&[self.acquire_fence], true, ACQUIRE_TIMEOUT_NS)
        } {
            Ok(()) => Ok(AcquiredImage {
                index: Some(image_index),
                status: if suboptimal {
                    AcquireStatus::Suboptimal
                } else {
                    AcquireStatus::Success
                },
            }),
            Err(vk::Result::TIMEOUT) => Ok(AcquiredImage {
                index: None,
                status: AcquireStatus::Timeout,
            }),
            Err(vk::Result::ERROR_DEVICE_LOST) => Err(PresentationError::DeviceLost),
            Err(result) => Err(PresentationError::AcquireFailed {
                code: result.as_raw(),
                message: std::format!("waiting on the acquire fence: {result}"),
            }),
        }
    }

    /// Records a clear-only pass over `image_index` and submits it,
    /// blocking until done. No pipeline, no draw — the proof is
    /// acquire → clear → present, nothing more.
    fn record_clear_and_submit(
        &self,
        image_index: u32,
        clear_color: [f32; 4],
    ) -> Result<(), PresentationError> {
        let position =
            usize::try_from(image_index).map_err(|_| PresentationError::PresentFailed {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: std::format!(
                    "swapchain image index {image_index} exceeds the address space"
                ),
            })?;
        let framebuffer =
            self.swapchain
                .framebuffer_at(position)
                .ok_or(PresentationError::PresentFailed {
                    code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                    message: std::format!("no framebuffer for swapchain image {image_index}"),
                })?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.device.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: fully specified allocate-info against this device's
        // own pool; error-or-vec return.
        let command_buffer = unsafe { self.device.device.allocate_command_buffers(&allocate_info) }
            .map_err(|result| submit_error("allocating the clear command buffer", result))?
            .first()
            .copied()
            .ok_or(PresentationError::PresentFailed {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: String::from("the driver allocated no command buffer for count 1"),
            })?;
        let outcome = self.record_submit_inner(command_buffer, framebuffer, clear_color);
        // SAFETY: buffer owned here (allocated lines above, never
        // submitted elsewhere); freeing is infallible, so it runs on
        // both paths — a failed submit never leaks its buffer.
        unsafe {
            self.device
                .device
                .free_command_buffers(self.device.command_pool, &[command_buffer]);
        }
        outcome
    }

    /// The record/submit/wait body of [`VulkanPresenter::record_clear_and_submit`]:
    /// factored out so the command buffer is freed exactly once on
    /// every path.
    fn record_submit_inner(
        &self,
        command_buffer: vk::CommandBuffer,
        framebuffer: vk::Framebuffer,
        clear_color: [f32; 4],
    ) -> Result<(), PresentationError> {
        let device = &self.device.device;
        let begin_info = vk::CommandBufferBeginInfo::default();
        // SAFETY: freshly allocated primary buffer, never begun.
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .map_err(|result| submit_error("beginning the clear command buffer", result))?;
        let (width, height) = self.swapchain.extent();
        let render_area = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D { width, height },
        };
        let clear_value = vk::ClearValue {
            color: vk::ClearColorValue {
                float32: clear_color,
            },
        };
        let clear_values = [clear_value];
        let pass_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.swapchain.render_pass())
            .framebuffer(framebuffer)
            .render_area(render_area)
            .clear_values(&clear_values);
        // SAFETY: pass, framebuffer, area, and clear value are mutually
        // compatible (built together for exactly this pairing); the
        // viewport/scissor below cover the same target rect.
        unsafe {
            device.cmd_begin_render_pass(command_buffer, &pass_begin, vk::SubpassContents::INLINE);
            // Dynamic viewport/scissor at the drawn extent (kept current
            // across recreates because it is recorded per frame, never
            // baked).
            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: axis_to_float(width)?,
                height: axis_to_float(height)?,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            device.cmd_set_viewport(command_buffer, 0, &[viewport]);
            device.cmd_set_scissor(command_buffer, 0, &[render_area]);
            device.cmd_end_render_pass(command_buffer);
        }
        // SAFETY: begun above with only valid record calls since; ending
        // a well-formed buffer is sound.
        unsafe { device.end_command_buffer(command_buffer) }
            .map_err(|result| submit_error("ending the clear command buffer", result))?;
        // SAFETY: single owned buffer, submitted to this device's own
        // graphics-cum-present queue; waited on below before return.
        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        unsafe { device.queue_submit(self.device.queue, &[submit_info], vk::Fence::null()) }
            .map_err(|result| submit_error("submitting the clear command buffer", result))?;
        unsafe { device.queue_wait_idle(self.device.queue) }
            .map_err(|result| submit_error("waiting for the clear submission", result))?;
        Ok(())
    }

    /// Blits `target`'s current contents into swapchain image
    /// `image_index` and submits the transfer, blocking until done. The
    /// blit converts formats semantically (offscreen RGBA to the
    /// negotiated swapchain format); extents are equal by the time this
    /// runs (checked in
    /// [`VulkanPresenter::present_color_target`]), so no scaling occurs.
    fn record_blit_and_submit(
        &self,
        image_index: u32,
        target: &crate::VulkanColorTarget,
    ) -> Result<(), PresentationError> {
        let position =
            usize::try_from(image_index).map_err(|_| PresentationError::PresentFailed {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: std::format!(
                    "swapchain image index {image_index} exceeds the address space"
                ),
            })?;
        let dst_image =
            self.swapchain
                .image_at(position)
                .ok_or(PresentationError::PresentFailed {
                    code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                    message: std::format!("no swapchain image for index {image_index}"),
                })?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.device.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: fully specified allocate-info against this device's
        // own pool; error-or-vec return.
        let command_buffer = unsafe { self.device.device.allocate_command_buffers(&allocate_info) }
            .map_err(|result| submit_error("allocating the blit command buffer", result))?
            .first()
            .copied()
            .ok_or(PresentationError::PresentFailed {
                code: vk::Result::ERROR_INITIALIZATION_FAILED.as_raw(),
                message: String::from("the driver allocated no command buffer for count 1"),
            })?;
        let outcome = self.record_blit_inner(command_buffer, dst_image, target);
        // SAFETY: buffer owned here (allocated lines above, never
        // submitted elsewhere); freeing is infallible, so it runs on
        // both paths — a failed submit never leaks its buffer.
        unsafe {
            self.device
                .device
                .free_command_buffers(self.device.command_pool, &[command_buffer]);
        }
        outcome
    }

    /// The record/submit/wait body of
    /// [`VulkanPresenter::record_blit_and_submit`]: transitions the
    /// acquired image into transfer-destination, blits the offscreen
    /// target across it, transitions to present-source, and submits.
    ///
    /// Layout notes: the destination's old layout is `UNDEFINED` (always
    /// a legal old layout, and correct here — the image was just
    /// acquired and the blit overwrites it fully). The source needs no
    /// barrier: the caller submits the RHI scene first via
    /// `submit_and_wait`, so the queue is idle and the transfer-source
    /// layout the RHI pass leaves is current and visible.
    fn record_blit_inner(
        &self,
        command_buffer: vk::CommandBuffer,
        dst_image: vk::Image,
        target: &crate::VulkanColorTarget,
    ) -> Result<(), PresentationError> {
        let device = &self.device.device;
        let begin_info = vk::CommandBufferBeginInfo::default();
        // SAFETY: freshly allocated primary buffer, never begun.
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .map_err(|result| submit_error("beginning the blit command buffer", result))?;
        let (width, height) = self.swapchain.extent();
        let subresource = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let to_dst_barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(dst_image)
            .subresource_range(subresource);
        let blit = vk::ImageBlit::default()
            .src_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_offsets([
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: width_int(width)?,
                    y: height_int(height)?,
                    z: 1,
                },
            ])
            .dst_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .dst_offsets([
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: width_int(width)?,
                    y: height_int(height)?,
                    z: 1,
                },
            ]);
        let blits = [blit];
        let to_present_barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(dst_image)
            .subresource_range(subresource);
        // SAFETY: both barriers reference the just-acquired image (owned
        // by this swapchain, no other writer — the queue is idle), the
        // blit reads the caller-submitted offscreen image in its
        // transfer-source layout over the full equal extent, and the
        // filter is `NEAREST` (no linear-filtering feature requirement)
        // over a same-size region (no scaling).
        unsafe {
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_dst_barrier],
            );
            device.cmd_blit_image(
                command_buffer,
                target.image(),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                dst_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &blits,
                vk::Filter::NEAREST,
            );
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_present_barrier],
            );
        }
        // SAFETY: begun above with only valid record calls since; ending
        // a well-formed buffer is sound.
        unsafe { device.end_command_buffer(command_buffer) }
            .map_err(|result| submit_error("ending the blit command buffer", result))?;
        // SAFETY: single owned buffer, submitted to this device's own
        // graphics-cum-present queue; waited on below before return.
        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
        unsafe { device.queue_submit(self.device.queue, &[submit_info], vk::Fence::null()) }
            .map_err(|result| submit_error("submitting the blit command buffer", result))?;
        unsafe { device.queue_wait_idle(self.device.queue) }
            .map_err(|result| submit_error("waiting for the blit submission", result))?;
        Ok(())
    }

    /// Presents a submitted image to the window.
    fn present_image(&self, image_index: u32) -> Result<PresentStatus, PresentationError> {
        let swapchains = [self.swapchain.handle()];
        let indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .swapchains(&swapchains)
            .image_indices(&indices);
        // SAFETY: swapchain and index were just acquired-and-submitted
        // on this device/queue; no wait semaphores at this scope (the
        // submit above already completed via queue_wait_idle).
        match unsafe {
            self.swapchain
                .loader()
                .queue_present(self.device.queue, &present_info)
        } {
            Ok(suboptimal) => Ok(if suboptimal {
                PresentStatus::Suboptimal
            } else {
                PresentStatus::Success
            }),
            Err(result) => map_present_result(result),
        }
    }
}

/// What [`VulkanPresenter::prepare_frame`] resolved: skip the frame or
/// present into the acquired image.
enum ReadyOrSkipped {
    Skipped(FrameOutcome),
    Ready(ReadyImage),
}

/// An acquired, drawable swapchain image plus the report flags its frame
/// carries.
struct ReadyImage {
    image_index: u32,
    acquire_status: AcquireStatus,
    swapchain_recreated: bool,
}

/// Maps an acquire-fence failure: device loss is fatal, everything else
/// is a failed acquire with context.
fn fence_error(phase: &str, result: vk::Result) -> PresentationError {
    if result == vk::Result::ERROR_DEVICE_LOST {
        PresentationError::DeviceLost
    } else {
        PresentationError::AcquireFailed {
            code: result.as_raw(),
            message: std::format!("{phase}: {result}"),
        }
    }
}

/// Maps a clear-submit failure with its phase attached.
fn submit_error(phase: &str, result: vk::Result) -> PresentationError {
    if result == vk::Result::ERROR_DEVICE_LOST {
        PresentationError::DeviceLost
    } else {
        PresentationError::PresentFailed {
            code: result.as_raw(),
            message: std::format!("{phase}: {result}"),
        }
    }
}

/// Expresses a swapchain-extent axis as a blit offset without a lossy
/// `as` cast: axes beyond `i32`'s range (no real display reaches it)
/// are an explicit error, not a silent truncation.
fn width_int(value: u32) -> Result<i32, PresentationError> {
    i32::try_from(value).map_err(|_| PresentationError::ExtentTooLarge { value })
}

/// Expresses a swapchain-extent axis as a blit offset without a lossy
/// `as` cast: axes beyond `i32`'s range (no real display reaches it)
/// are an explicit error, not a silent truncation.
fn height_int(value: u32) -> Result<i32, PresentationError> {
    i32::try_from(value).map_err(|_| PresentationError::ExtentTooLarge { value })
}

/// Expresses a swapchain-extent axis as a viewport float without a
/// lossy `as` cast: axes beyond `u16`'s range (no real display reaches
/// it — the largest swapchain extents stay under 17k) are an explicit
/// error, not a silent truncation.
fn axis_to_float(value: u32) -> Result<f32, PresentationError> {
    let narrowed = u16::try_from(value).map_err(|_| PresentationError::ExtentTooLarge { value })?;
    Ok(f32::from(narrowed))
}

impl Drop for VulkanPresenter {
    /// Tears down swapchain objects, the surface, and the acquire fence
    /// before the owned device's own `Drop` runs (field drops run after
    /// this): the reverse of the creation order in [`VulkanPresenter::new`].
    /// The idle wait is best-effort — `Drop` cannot fail, and proceeding
    /// to destroy is strictly safer than leaking.
    fn drop(&mut self) {
        // SAFETY: every handle below is owned here (created in `new`/
        // `build`, destroyed exactly once here); the device outlives
        // this call because its field `Drop` runs after.
        unsafe {
            let _ = self.device.device.device_wait_idle();
            self.device.device.destroy_fence(self.acquire_fence, None);
        }
        self.swapchain.destroy_surface_and_objects(&self.device);
    }
}
