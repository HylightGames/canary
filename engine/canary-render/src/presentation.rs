// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Windowed presentation vocabulary for the RHI: the `Window → surface →
//! swapchain → present` seam's Canary-owned types, negotiation policy, and
//! frame-outcome reporting.
//!
//! Everything here is plain data plus pure functions over it — no backend
//! types (`ash`/`vk::*`, `winit`, `raw-window-handle`) appear anywhere in
//! this module, so depending on it never pulls in a graphics API, a
//! windowing system, or a window-handle interop crate. Concrete backends
//! (today `canary-render-vulkan`'s presentation-gated presenter) convert
//! their native values into these types at the boundary, call the pure
//! negotiation functions below, and convert the answers back.
//!
//! This module is deliberately *not* a render graph, a materials system,
//! or multi-frame-flight bookkeeping: its scope is one cleared-frame
//! proof (acquire → clear-only pass → submit → present) plus the
//! lifecycle rules that proof needs (recreate triggers, minimized
//! suspend). See [ADR 0020](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0020-pre-v0-3-architectural-locks.md)
//! §10, which locks this seam (and nothing more of presentation) before
//! v0.3.
//!
//! [`RenderDevice`](crate::RenderDevice) itself is untouched by this
//! module: presentation lives in backend-owned presenter types built from
//! the vocabulary here, so every existing offscreen caller compiles
//! verbatim and the offscreen path stays byte-identical.

/// Which swapchain present mode Canary is allowed to request.
///
/// FIFO-only for this seam's scope: FIFO is the one mode the Vulkan
/// specification requires every swapchain to support, so requesting it
/// never needs a fallback path. Mailbox (lower latency, tearing-free)
/// is the intended next mode once a second consumer needs it — the enum
/// (rather than a bare constant) exists so adding it stays additive.
///
/// Non-exhaustive so that future mode arrives as a new variant, not as
/// a second parallel mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PresentModePreference {
    /// Vertical-sync FIFO: present in order, never tear, block the
    /// presenter when the queue is full. The only supported mode.
    Fifo,
}

/// What size swapchain to build, and which present mode to prefer.
///
/// Carries the *window's* current size in physical pixels (what
/// `canary-platform`'s surface-capable windows report); the backend
/// still clamps it against the surface capabilities via
/// [`resolve_extent`], since the driver — not the window — owns the
/// final say on legal extents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceDescriptor {
    /// Requested width in physical pixels. Zero in either axis means
    /// the window is minimized — see [`is_suspended_extent`].
    pub width: u32,
    /// Requested height in physical pixels.
    pub height: u32,
    /// Which present mode to request. FIFO-only today.
    pub present_mode: PresentModePreference,
}

/// Which class of GPU an adapter is, for presentation-adapter scoring.
///
/// Mirrors the Vulkan physical-device-type taxonomy without naming it,
/// so backends convert one enum match at the boundary (see
/// [`score_presentation_adapter`]) and everything above stays
/// backend-neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// A discrete GPU — preferred for presentation.
    DiscreteGpu,
    /// An integrated GPU.
    IntegratedGpu,
    /// A virtualized GPU (cloud/VM passthrough).
    VirtualGpu,
    /// A CPU software rasterizer (llvmpipe/lavapipe class).
    Cpu,
    /// Anything else, including kinds a future backend meets first.
    Other,
}

/// What a physical device offers toward windowed presentation.
///
/// All three capability flags must hold for the adapter to present at
/// all — see [`AdapterCapabilities::is_presentation_capable`]. Built by
/// the backend from one graphics-queue query, one
/// present-support query, and one device-extension enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCapabilities {
    /// Whether some queue family performs graphics operations.
    pub graphics_queue: bool,
    /// Whether that same family can present to the window surface.
    /// Same-family (not merely same-device) support is what this
    /// seam's scope requires: multi-family ownership transfer is real
    /// future work, not something a cleared-frame proof needs.
    pub present_support: bool,
    /// Whether the device exposes the swapchain device extension.
    pub swapchain_extension: bool,
    /// Which class of device this is, for scoring.
    pub device_kind: DeviceKind,
}

impl AdapterCapabilities {
    /// Whether this adapter can present at all: graphics-capable,
    /// present-capable, *and* swapchain-extension-bearing. A single
    /// missing flag disqualifies — there is no partial presentation.
    pub const fn is_presentation_capable(&self) -> bool {
        self.graphics_queue && self.present_support && self.swapchain_extension
    }
}

/// Ranks a presentation-capable adapter for device selection.
///
/// Returns `None` when [`AdapterCapabilities::is_presentation_capable`]
/// fails (missing graphics, present support, or the swapchain
/// extension); otherwise a higher number means a more preferred
/// device (discrete above integrated/virtual above CPU above other).
/// Backends pick the highest score, first-enumerated winning ties —
/// deterministic given a stable enumeration order.
pub const fn score_presentation_adapter(caps: &AdapterCapabilities) -> Option<u32> {
    if !caps.is_presentation_capable() {
        return None;
    }
    match caps.device_kind {
        DeviceKind::DiscreteGpu => Some(3),
        DeviceKind::IntegratedGpu | DeviceKind::VirtualGpu => Some(2),
        DeviceKind::Cpu => Some(1),
        DeviceKind::Other => Some(0),
    }
}

/// One surface format the driver offers, as raw codes.
///
/// Backend-neutral by construction: the backend converts each native
/// format/color-space pair to its two integer codes at the boundary,
/// calls [`choose_surface_format_index`], and converts the winner back.
/// The preferred codes name `B8G8R8A8_SRGB` with `SRGB_NONLINEAR`
/// (Canary's fixed presentation policy), but this struct itself carries
/// no backend enum — just the codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceFormat {
    /// The image format code (Vulkan: `vk::Format::as_raw()`).
    pub format_code: i32,
    /// The color-space code (Vulkan: `vk::ColorSpaceKHR::as_raw()`).
    pub color_space_code: i32,
}

impl SurfaceFormat {
    /// The preferred format code: `B8G8R8A8_SRGB` (`vk::Format` 50).
    pub const PREFERRED_FORMAT_CODE: i32 = 50;
    /// The preferred color-space code: `SRGB_NONLINEAR` (0).
    pub const PREFERRED_COLOR_SPACE_CODE: i32 = 0;
}

/// Picks which offered surface format to build the swapchain with.
///
/// Canary policy, fixed: the preferred format plus the preferred color
/// space when offered, otherwise the first offered format. Returns the
/// *index* into `formats` (not a copy) so the backend converts the
/// original native value back without a second lookup. `None` when no
/// format was offered at all — a driver reporting zero formats cannot
/// present, and the caller turns this into a fatal error rather than
/// guessing.
pub fn choose_surface_format_index(formats: &[SurfaceFormat]) -> Option<usize> {
    let preferred = formats.iter().position(|format| {
        format.format_code == SurfaceFormat::PREFERRED_FORMAT_CODE
            && format.color_space_code == SurfaceFormat::PREFERRED_COLOR_SPACE_CODE
    });
    match preferred {
        Some(index) => Some(index),
        None => {
            if formats.is_empty() {
                None
            } else {
                Some(0)
            }
        }
    }
}

/// How many swapchain images to request, from the surface capabilities.
///
/// Canary policy, fixed: one more than the minimum (one slot being
/// acquired/rendered while another is presented), clamped down to the
/// maximum when the driver bounds it (`max == 0` means unbounded per
/// the Vulkan contract), preferring at least triple-buffering whenever
/// the maximum allows it.
pub const fn negotiate_image_count(min_image_count: u32, max_image_count: u32) -> u32 {
    let plus_one = min_image_count.saturating_add(1);
    let desired = if plus_one > 3 { plus_one } else { 3 };
    if max_image_count == 0 {
        desired
    } else if desired > max_image_count {
        max_image_count
    } else {
        desired
    }
}

/// Which swapchain extent to build, from the surface capabilities.
///
/// When the driver dictates an extent (`current` is `Some` — i.e. its
/// `currentExtent` was not `UINT32_MAX`), that extent wins verbatim:
/// the driver owns the final say. Otherwise the window's requested size
/// is clamped per axis into the driver's `[min, max]` range.
pub const fn resolve_extent(
    current: Option<(u32, u32)>,
    min: (u32, u32),
    max: (u32, u32),
    window: (u32, u32),
) -> (u32, u32) {
    match current {
        Some(extent) => extent,
        None => (
            clamp_axis(window.0, min.0, max.0),
            clamp_axis(window.1, min.1, max.1),
        ),
    }
}

/// Clamps one extent axis into `[lo, hi]`, written out because
/// `Ord::clamp` is not yet callable from `const fn` on stable.
const fn clamp_axis(value: u32, lo: u32, hi: u32) -> u32 {
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
}

/// Whether an extent means "minimized — suspend, don't error".
///
/// A zero in either axis is what a minimized window reports; building
/// (or keeping) a swapchain for it is invalid, so the presenter skips
/// the frame and tries again next frame rather than failing.
pub const fn is_suspended_extent(extent: (u32, u32)) -> bool {
    extent.0 == 0 || extent.1 == 0
}

/// The non-error outcome of acquiring a swapchain image.
///
/// These are statuses, not errors, quite deliberately: acquiring an
/// image while reporting `Suboptimal` still hands back a usable image,
/// and stuffing that into `Err` would force every caller to treat a
/// successful acquire as a failure. Only failures that yield *no*
/// defined caller action (device lost, surface lost, a Vulkan call
/// failing outright) become [`PresentationError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireStatus {
    /// An image was acquired; the swapchain still matches the surface.
    Success,
    /// An image was acquired but the swapchain no longer matches the
    /// surface exactly: proceed with this frame, flag a recreate.
    Suboptimal,
    /// No image became available within the acquire timeout: skip the
    /// frame, retry next frame.
    Timeout,
    /// The window is occluded: skip the frame, retry next frame.
    Occluded,
    /// The swapchain no longer matches the surface at all: recreate,
    /// skip this frame.
    Outdated,
    /// The surface was lost. Returned for synthetic/completeness paths;
    /// real backend calls surface this as [`PresentationError::SurfaceLost`]
    /// instead, since no per-frame recovery exists.
    Lost,
    /// The window is minimized (zero extent): suspend — skip the frame
    /// without touching the swapchain, retry next frame.
    Suspended,
}

/// The non-error outcome of presenting a rendered image.
///
/// Same statuses-not-errors contract as [`AcquireStatus`]: a present
/// reporting `Suboptimal` still presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentStatus {
    /// The image presented; the swapchain still matches the surface.
    Success,
    /// The image presented but the swapchain no longer matches the
    /// surface exactly: flag a recreate.
    Suboptimal,
    /// The swapchain no longer matches the surface at all (a resize
    /// raced the present): flag a recreate.
    Outdated,
    /// The window is occluded: the present may not have become
    /// visible; flag nothing, retry next frame.
    Occluded,
    /// The surface was lost. Returned for synthetic/completeness paths;
    /// real backend calls surface this as [`PresentationError::SurfaceLost`].
    Lost,
}

/// Whether the swapchain must be recreated after an acquire.
///
/// `Outdated` always recreates (the swapchain no longer matches);
/// `Suboptimal` recreates too (it still matches, but only just — the
/// next resize would tip it over). Everything else proceeds on the
/// current swapchain.
pub const fn should_recreate_after_acquire(status: AcquireStatus) -> bool {
    match status {
        AcquireStatus::Success
        | AcquireStatus::Timeout
        | AcquireStatus::Occluded
        | AcquireStatus::Suspended => false,
        AcquireStatus::Suboptimal | AcquireStatus::Outdated | AcquireStatus::Lost => true,
    }
}

/// Whether the current frame must be skipped after an acquire.
///
/// `Outdated` skips (recreate first, draw nothing into a stale
/// swapchain); `Timeout`/`Occluded`/`Suspended` skip (no usable image
/// to draw into); `Lost` skips (nothing to draw into at all).
/// `Success` and `Suboptimal` both carry a live image — draw.
pub const fn should_skip_frame_after_acquire(status: AcquireStatus) -> bool {
    match status {
        AcquireStatus::Success | AcquireStatus::Suboptimal => false,
        AcquireStatus::Timeout
        | AcquireStatus::Occluded
        | AcquireStatus::Outdated
        | AcquireStatus::Lost
        | AcquireStatus::Suspended => true,
    }
}

/// Whether the swapchain must be recreated after a present.
///
/// Either mismatch signal recreates before the next acquire;
/// `Occluded` and `Lost` carry no recreate (occlusion resolves on its
/// own; loss is fatal, not recoverable by recreating).
pub const fn should_recreate_after_present(status: PresentStatus) -> bool {
    match status {
        PresentStatus::Success | PresentStatus::Occluded | PresentStatus::Lost => false,
        PresentStatus::Suboptimal | PresentStatus::Outdated => true,
    }
}

/// One acquired swapchain image: its index plus how the acquire went.
///
/// `index` is `None` exactly when [`should_skip_frame_after_acquire`]
/// holds for `status` — no image to draw into, skip the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquiredImage {
    /// The swapchain image index, or `None` when the frame is skipped.
    pub index: Option<u32>,
    /// How the acquire went.
    pub status: AcquireStatus,
}

/// One presented frame's report: which image, how each step went, and
/// whether the swapchain was rebuilt underneath this call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentedFrame {
    /// The swapchain image index that was drawn and presented.
    pub image_index: u32,
    /// How the acquire went (`Success` or `Suboptimal` here — anything
    /// else skips the frame instead; see [`FrameOutcome`]).
    pub acquire_status: AcquireStatus,
    /// How the present went.
    pub present_status: PresentStatus,
    /// Whether the swapchain was recreated during this call (resize,
    /// generation change, or a pending mismatch flag) before drawing.
    pub swapchain_recreated: bool,
}

/// What one `present_cleared_frame` call did.
///
/// Either a frame went out the door ([`FrameOutcome::Presented`]) or
/// the frame was deliberately skipped ([`FrameOutcome::Skipped`]) —
/// skipping is normal operation (minimized window, acquire timeout,
/// a recreate consuming the frame), never an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// A cleared frame was drawn and presented. See [`PresentedFrame`].
    Presented(PresentedFrame),
    /// The frame was skipped; `status` says why.
    Skipped {
        /// The acquire status that caused the skip.
        status: AcquireStatus,
    },
}

/// A fatal presentation failure: something with no defined per-frame
/// recovery. Everything recoverable (suboptimal, outdated, timeout,
/// occlusion, minimization) is a status on [`AcquireStatus`] /
/// [`PresentStatus`] instead — see those enums' docs for why the split
/// exists.
///
/// Backend-facing boundary rule (see the crate docs): no third-party
/// types appear here. Fallible variants erase the native error into an
/// owned code/message pair at construction, so callers match on and
/// display this error without ever naming backend types.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PresentationError {
    /// The window offered no handles for surface creation (no live OS
    /// window, or a platform this seam does not cover).
    #[error("no window handles available for surface creation")]
    NoWindowHandles,
    /// The window/display native pair is not one this seam builds
    /// surfaces for.
    #[error("unsupported native window/display pair for presentation")]
    UnsupportedPlatform,
    /// Querying the surface extensions the instance needs failed.
    #[error("failed to query required surface extensions: {message} ({code})")]
    RequiredExtensions {
        /// Raw native result code, kept for diagnosis.
        code: i32,
        /// Human-readable rendering of the failure at construction time.
        message: String,
    },
    /// Window-surface creation failed.
    #[error("failed to create window surface: {message} ({code})")]
    SurfaceCreation {
        /// Raw native result code, kept for diagnosis.
        code: i32,
        /// Human-readable rendering of the failure at construction time.
        message: String,
    },
    /// No enumerated adapter is graphics-capable, present-capable, and
    /// swapchain-bearing all at once.
    #[error("no presentation-capable adapter found (need graphics + present + swapchain support)")]
    NoPresentCapableAdapter,
    /// Backend initialization around presentation failed (loader,
    /// instance, logical device, ...), with the backend's own message
    /// preserved as text.
    #[error("backend initialization for presentation failed: {0}")]
    Initialization(String),
    /// Swapchain creation (or recreation) failed.
    #[error("failed to create swapchain: {message} ({code})")]
    SwapchainCreation {
        /// Raw native result code, kept for diagnosis.
        code: i32,
        /// Human-readable rendering of the failure at construction time.
        message: String,
    },
    /// Swapchain-image acquisition failed fatally (not timeout — that
    /// is [`AcquireStatus::Timeout`]).
    #[error("failed to acquire swapchain image: {message} ({code})")]
    AcquireFailed {
        /// Raw native result code, kept for diagnosis.
        code: i32,
        /// Human-readable rendering of the failure at construction time.
        message: String,
    },
    /// Queue presentation failed.
    #[error("failed to present swapchain image: {message} ({code})")]
    PresentFailed {
        /// Raw native result code, kept for diagnosis.
        code: i32,
        /// Human-readable rendering of the failure at construction time.
        message: String,
    },
    /// The window surface was lost: no per-frame recovery exists.
    #[error("the window surface was lost")]
    SurfaceLost,
    /// The logical device was lost: no per-frame recovery exists.
    #[error("the device was lost")]
    DeviceLost,
    /// A swapchain extent axis exceeds what the presenter can express
    /// (see the backend's conversion docs); carries the offending axis
    /// length, not a native error.
    #[error("swapchain extent axis {value} exceeds the presentable range")]
    ExtentTooLarge {
        /// The offending axis length in pixels.
        value: u32,
    },
    /// A content frame's offscreen target does not match the swapchain
    /// extent. The caller recreates its target from the presenter's
    /// current extent (reported by `PresentedFrame.swapchain_recreated`)
    /// and retries — presenting a mismatched target would scale or crop
    /// silently, so the presenter refuses loudly instead.
    #[error(
        "content target extent {content:?} does not match swapchain extent {swapchain:?}: \
         recreate the target and retry"
    )]
    ContentExtentMismatch {
        /// The swapchain's current extent in pixels.
        swapchain: (u32, u32),
        /// The content target's extent in pixels.
        content: (u32, u32),
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_canary_format_and_color_space_when_offered() {
        let formats = [
            SurfaceFormat {
                format_code: 37,
                color_space_code: 0,
            },
            SurfaceFormat {
                format_code: SurfaceFormat::PREFERRED_FORMAT_CODE,
                color_space_code: SurfaceFormat::PREFERRED_COLOR_SPACE_CODE,
            },
            SurfaceFormat {
                format_code: SurfaceFormat::PREFERRED_FORMAT_CODE,
                color_space_code: 1,
            },
        ];
        assert_eq!(choose_surface_format_index(&formats), Some(1));
    }

    #[test]
    fn falls_back_to_the_first_offered_format_without_the_preferred_pair() {
        let formats = [
            SurfaceFormat {
                format_code: 37,
                color_space_code: 0,
            },
            SurfaceFormat {
                format_code: SurfaceFormat::PREFERRED_FORMAT_CODE,
                color_space_code: 1,
            },
        ];
        assert_eq!(choose_surface_format_index(&formats), Some(0));
    }

    #[test]
    fn reports_no_format_when_the_driver_offers_none() {
        assert_eq!(choose_surface_format_index(&[]), None);
    }

    #[test]
    fn requests_one_more_than_the_minimum_unbounded() {
        assert_eq!(negotiate_image_count(2, 0), 3);
    }

    #[test]
    fn prefers_triple_buffering_when_the_maximum_allows_it() {
        assert_eq!(negotiate_image_count(1, 8), 3);
        assert_eq!(negotiate_image_count(2, 8), 3);
        assert_eq!(negotiate_image_count(4, 8), 5);
    }

    #[test]
    fn clamps_down_to_a_bounding_maximum() {
        assert_eq!(negotiate_image_count(2, 2), 2);
        assert_eq!(negotiate_image_count(8, 8), 8);
    }

    #[test]
    fn uses_the_driver_dictated_extent_verbatim() {
        assert_eq!(
            resolve_extent(Some((800, 600)), (1, 1), (4096, 4096), (1280, 720)),
            (800, 600)
        );
    }

    #[test]
    fn clamps_the_window_size_into_the_driver_range() {
        assert_eq!(
            resolve_extent(None, (64, 64), (4096, 4096), (1280, 720)),
            (1280, 720)
        );
        assert_eq!(
            resolve_extent(None, (64, 64), (4096, 4096), (8, 9000)),
            (64, 4096)
        );
    }

    #[test]
    fn treats_any_zero_axis_as_minimized_suspend() {
        assert!(is_suspended_extent((0, 0)));
        assert!(is_suspended_extent((0, 600)));
        assert!(is_suspended_extent((800, 0)));
        assert!(!is_suspended_extent((1, 1)));
        assert!(!is_suspended_extent((800, 600)));
    }

    #[test]
    fn recreates_on_mismatch_signals_only_after_acquire() {
        assert!(!should_recreate_after_acquire(AcquireStatus::Success));
        assert!(should_recreate_after_acquire(AcquireStatus::Suboptimal));
        assert!(should_recreate_after_acquire(AcquireStatus::Outdated));
        assert!(!should_recreate_after_acquire(AcquireStatus::Timeout));
        assert!(!should_recreate_after_acquire(AcquireStatus::Occluded));
        assert!(!should_recreate_after_acquire(AcquireStatus::Suspended));
    }

    #[test]
    fn skips_the_frame_whenever_no_live_image_was_acquired() {
        assert!(!should_skip_frame_after_acquire(AcquireStatus::Success));
        assert!(!should_skip_frame_after_acquire(AcquireStatus::Suboptimal));
        assert!(should_skip_frame_after_acquire(AcquireStatus::Outdated));
        assert!(should_skip_frame_after_acquire(AcquireStatus::Timeout));
        assert!(should_skip_frame_after_acquire(AcquireStatus::Occluded));
        assert!(should_skip_frame_after_acquire(AcquireStatus::Suspended));
        assert!(should_skip_frame_after_acquire(AcquireStatus::Lost));
    }

    #[test]
    fn recreates_on_mismatch_signals_only_after_present() {
        assert!(!should_recreate_after_present(PresentStatus::Success));
        assert!(should_recreate_after_present(PresentStatus::Suboptimal));
        assert!(should_recreate_after_present(PresentStatus::Outdated));
        assert!(!should_recreate_after_present(PresentStatus::Occluded));
    }

    #[test]
    fn rejects_adapters_missing_any_presentation_requirement() {
        let capable = AdapterCapabilities {
            graphics_queue: true,
            present_support: true,
            swapchain_extension: true,
            device_kind: DeviceKind::DiscreteGpu,
        };
        assert!(capable.is_presentation_capable());
        for missing in [
            AdapterCapabilities {
                graphics_queue: false,
                ..capable
            },
            AdapterCapabilities {
                present_support: false,
                ..capable
            },
            AdapterCapabilities {
                swapchain_extension: false,
                ..capable
            },
        ] {
            assert!(!missing.is_presentation_capable());
            assert_eq!(score_presentation_adapter(&missing), None);
        }
    }

    #[test]
    fn scores_discrete_above_integrated_above_cpu_above_other() {
        let capable = |device_kind| AdapterCapabilities {
            graphics_queue: true,
            present_support: true,
            swapchain_extension: true,
            device_kind,
        };
        let discrete = score_presentation_adapter(&capable(DeviceKind::DiscreteGpu));
        let integrated = score_presentation_adapter(&capable(DeviceKind::IntegratedGpu));
        let congestion = score_presentation_adapter(&capable(DeviceKind::VirtualGpu));
        let cpu = score_presentation_adapter(&capable(DeviceKind::Cpu));
        let other = score_presentation_adapter(&capable(DeviceKind::Other));
        assert!(discrete > integrated);
        assert_eq!(integrated, congestion);
        assert!(integrated > cpu);
        assert!(cpu > other);
    }

    #[test]
    fn every_presentation_error_displays_a_non_empty_message() {
        let errors = [
            PresentationError::NoWindowHandles,
            PresentationError::UnsupportedPlatform,
            PresentationError::RequiredExtensions {
                code: -1,
                message: String::from("test"),
            },
            PresentationError::SurfaceCreation {
                code: -1,
                message: String::from("test"),
            },
            PresentationError::NoPresentCapableAdapter,
            PresentationError::Initialization(String::from("test")),
            PresentationError::SwapchainCreation {
                code: -1,
                message: String::from("test"),
            },
            PresentationError::AcquireFailed {
                code: -1,
                message: String::from("test"),
            },
            PresentationError::PresentFailed {
                code: -1,
                message: String::from("test"),
            },
            PresentationError::SurfaceLost,
            PresentationError::DeviceLost,
            PresentationError::ExtentTooLarge { value: 100_000 },
        ];
        for error in errors {
            assert!(!error.to_string().is_empty(), "{error:?}");
        }
    }
}
