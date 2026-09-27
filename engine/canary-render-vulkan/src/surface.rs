// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The render-backend side of the `Window → surface` seam: rebuilding
//! native handles from Canary-owned carriers, creating window surfaces,
//! selecting a present-capable adapter, and mapping native
//! acquire/present results onto Canary statuses.
//!
//! This is the one module in this crate that names `raw-window-handle`
//! (rebuilding owned handles from [`WindowHandles`]) and `ash-window`
//! (required-extension query plus per-platform surface dispatch). The
//! interop version is 0.6 — the same line `winit` 0.30 enables by
//! default — locked to a single resolution in `Cargo.lock`. Nothing
//! here appears in a fully-public signature: every `ash`/`vk::*`/
//! handle-interop type stays `pub(crate)` at most, and callers above
//! this module see only `canary-render`'s presentation vocabulary.

use ash::vk;
use canary_platform::surface::{DisplayHandle, WindowHandle, WindowHandles};
use canary_render::presentation::{
    AcquireStatus, AdapterCapabilities, DeviceKind, PresentStatus, PresentationError, SurfaceFormat,
};
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle,
    WaylandDisplayHandle, WaylandWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
    XcbDisplayHandle, XcbWindowHandle, XlibDisplayHandle, XlibWindowHandle,
};

/// One-second acquire/fence timeout in nanoseconds: long enough that a
/// live swapchain never trips it, short enough that a wedged one fails
/// loudly instead of hanging the frame loop.
pub(crate) const ACQUIRE_TIMEOUT_NS: u64 = 1_000_000_000;

/// Rebuilds the owned display handle a surface call needs from its
/// Canary-owned carrier. Constructor plus field copies — the carrier
/// mirrors the interop structs, so no conversion (and no
/// pointer/integer cast) happens here. (`#[non_exhaustive]` on the
/// interop side forbids struct literals from here; the fields
/// themselves stay assignable.)
pub(crate) fn rebuild_display(
    handles: &WindowHandles,
) -> Result<RawDisplayHandle, PresentationError> {
    match handles.display() {
        DisplayHandle::Xlib { display, screen } => Ok(RawDisplayHandle::Xlib(
            XlibDisplayHandle::new(display, screen),
        )),
        DisplayHandle::Xcb { connection, screen } => Ok(RawDisplayHandle::Xcb(
            XcbDisplayHandle::new(connection, screen),
        )),
        DisplayHandle::Wayland { display } => Ok(RawDisplayHandle::Wayland(
            WaylandDisplayHandle::new(display),
        )),
        DisplayHandle::Windows => Ok(RawDisplayHandle::Windows(WindowsDisplayHandle::new())),
        DisplayHandle::AppKit => Ok(RawDisplayHandle::AppKit(AppKitDisplayHandle::new())),
    }
}

/// Rebuilds the owned window handle a surface call needs. Same
/// constructor-plus-assignment discipline as [`rebuild_display`].
pub(crate) fn rebuild_window(
    handles: &WindowHandles,
) -> Result<RawWindowHandle, PresentationError> {
    match handles.window() {
        WindowHandle::Xlib { window, visual_id } => {
            let mut rebuilt = XlibWindowHandle::new(window);
            rebuilt.visual_id = visual_id;
            Ok(RawWindowHandle::Xlib(rebuilt))
        }
        WindowHandle::Xcb { window, visual_id } => {
            let mut rebuilt = XcbWindowHandle::new(window);
            rebuilt.visual_id = visual_id;
            Ok(RawWindowHandle::Xcb(rebuilt))
        }
        WindowHandle::Wayland { surface } => {
            Ok(RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)))
        }
        WindowHandle::Win32 { hwnd, hinstance } => {
            let mut rebuilt = Win32WindowHandle::new(hwnd);
            rebuilt.hinstance = hinstance;
            Ok(RawWindowHandle::Win32(rebuilt))
        }
        WindowHandle::AppKit { view } => Ok(RawWindowHandle::AppKit(AppKitWindowHandle::new(view))),
    }
}

/// Rejects display/window pairs that agree on nothing (an Xlib display
/// with a Wayland surface, ...). The carrier's constructors cannot
/// express platform pairing rules, so this runs before any native call:
/// handing a mismatched pair to the driver is driver-undefined, and
/// host-side refusal is soundness, not polish.
pub(crate) fn validate_pair(
    display: RawDisplayHandle,
    window: RawWindowHandle,
) -> Result<(), PresentationError> {
    let matched = matches!(
        (display, window),
        (RawDisplayHandle::Xlib(_), RawWindowHandle::Xlib(_))
            | (RawDisplayHandle::Xcb(_), RawWindowHandle::Xcb(_))
            | (RawDisplayHandle::Wayland(_), RawWindowHandle::Wayland(_))
            | (RawDisplayHandle::Windows(_), RawWindowHandle::Win32(_))
            | (RawDisplayHandle::AppKit(_), RawWindowHandle::AppKit(_))
    );
    if matched {
        Ok(())
    } else {
        Err(PresentationError::UnsupportedPlatform)
    }
}

/// Queries the instance extensions surface creation needs for this
/// display connection (`VK_KHR_surface` plus the one platform
/// extension), via `ash-window`. Private per the lifecycle contract:
/// instance creation calls this before building its extension list.
pub(crate) fn required_extensions(
    display: RawDisplayHandle,
) -> Result<&'static [*const std::os::raw::c_char], PresentationError> {
    ash_window::enumerate_required_extensions(display).map_err(|result| {
        PresentationError::RequiredExtensions {
            code: result.as_raw(),
            message: result.to_string(),
        }
    })
}

/// Creates the window surface for `handles` against `instance`.
///
/// The caller keeps `handles`' window alive for the surface's whole
/// lifetime (the presenter borrows the provider for exactly this
/// reason): the native call aliases the OS window/display objects, and
/// destroying them first is undefined behavior the type system cannot
/// see.
pub(crate) fn create_surface(
    entry: &ash::Entry,
    instance: &ash::Instance,
    handles: &WindowHandles,
) -> Result<vk::SurfaceKHR, PresentationError> {
    let display = rebuild_display(handles)?;
    let window = rebuild_window(handles)?;
    validate_pair(display, window)?;
    // SAFETY: `display`/`window` alias a live OS window (caller-upheld
    // lifetime); parent (`instance`) outlives the child by the
    // presenter's drop order; error-or-handle return, no null path.
    unsafe { ash_window::create_surface(entry, instance, display, window, None) }.map_err(
        |result| PresentationError::SurfaceCreation {
            code: result.as_raw(),
            message: result.to_string(),
        },
    )
}

/// Converts a backend device type without naming backend enums above
/// this module.
pub(crate) fn device_kind_of(kind: vk::PhysicalDeviceType) -> DeviceKind {
    match kind {
        vk::PhysicalDeviceType::DISCRETE_GPU => DeviceKind::DiscreteGpu,
        vk::PhysicalDeviceType::INTEGRATED_GPU => DeviceKind::IntegratedGpu,
        vk::PhysicalDeviceType::VIRTUAL_GPU => DeviceKind::VirtualGpu,
        vk::PhysicalDeviceType::CPU => DeviceKind::Cpu,
        _ => DeviceKind::Other,
    }
}

/// Whether `wanted` (a `NAME`-style extension identifier) is among the
/// enumerated device extensions. Byte-compare through the driver's
/// NUL-terminated name, mirroring this backend's validation-layer
/// presence check.
pub(crate) fn extension_present(
    enumerated: &[vk::ExtensionProperties],
    wanted: &std::ffi::CStr,
) -> bool {
    enumerated.iter().any(|properties| {
        // SAFETY: Vulkan guarantees `extension_name` is NUL-terminated
        // within its fixed bounds for enumerated properties.
        let name = unsafe { std::ffi::CStr::from_ptr(properties.extension_name.as_ptr()) };
        name == wanted
    })
}

/// Picks the best graphics-*and*-present-capable adapter with swapchain
/// support: one queue family must do both (same-family presentation is
/// this seam's scope — multi-family ownership transfer is deferred),
/// and the swapchain device extension must be enumerated. Scores via
/// `canary-render`'s pure scorer so the policy stays backend-neutral;
/// ties go to the first-enumerated device (deterministic given a stable
/// enumeration order). No capable adapter is
/// [`PresentationError::NoPresentCapableAdapter`], not a panic.
pub(crate) fn pick_presentable_device(
    instance: &ash::Instance,
    surface_loader: &ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
) -> Result<(vk::PhysicalDevice, u32), PresentationError> {
    // SAFETY for the enumeration below as a whole: read-only queries
    // against a live instance and a live surface; every fallible call
    // maps into `PresentationError::Initialization` (these queries have
    // no defined per-frame recovery — they run once at startup, not in
    // the frame loop).
    let physical_devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|result| PresentationError::Initialization(result.to_string()))?;
    let mut best: Option<(u32, vk::PhysicalDevice, u32)> = None;
    for physical in physical_devices {
        // SAFETY: read-only property query on an enumerated device.
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        // SAFETY: read-only family query on an enumerated device.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        for (index, family) in families.iter().enumerate() {
            if !family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                continue;
            }
            // `index` addresses a just-enumerated `Vec` — unreachable to
            // overflow `u32`, but a lossy `as` stays out regardless.
            let family_index = u32::try_from(index).map_err(|_| {
                PresentationError::Initialization(String::from(
                    "queue family index exceeds the addressable range",
                ))
            })?;
            // SAFETY: enumerated device, in-range family, live surface.
            let supported = unsafe {
                surface_loader.get_physical_device_surface_support(physical, family_index, surface)
            }
            .map_err(|result| PresentationError::Initialization(result.to_string()))?;
            if !supported {
                continue;
            }
            // SAFETY: read-only extension query on an enumerated device.
            let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
                .map_err(|result| PresentationError::Initialization(result.to_string()))?;
            let capabilities = AdapterCapabilities {
                graphics_queue: true,
                present_support: true,
                swapchain_extension: extension_present(&extensions, ash::khr::swapchain::NAME),
                device_kind: device_kind_of(properties.device_type),
            };
            if let Some(score) =
                canary_render::presentation::score_presentation_adapter(&capabilities)
            {
                let displaces = best.is_none_or(|(best_score, _, _)| score > best_score);
                if displaces {
                    best = Some((score, physical, family_index));
                }
            }
        }
    }
    best.map(|(_, physical, family)| (physical, family))
        .ok_or(PresentationError::NoPresentCapableAdapter)
}

/// Maps a failed swapchain-image acquire onto a status or a fatal
/// error. Success travels separately (the `Ok` arm of
/// `acquire_next_image`, handled by the caller): only the `Err` payload
/// reaches this function.
pub(crate) fn map_acquire_result(result: vk::Result) -> Result<AcquireStatus, PresentationError> {
    match result {
        vk::Result::TIMEOUT | vk::Result::NOT_READY => Ok(AcquireStatus::Timeout),
        vk::Result::SUBOPTIMAL_KHR => Ok(AcquireStatus::Suboptimal),
        vk::Result::ERROR_OUT_OF_DATE_KHR => Ok(AcquireStatus::Outdated),
        vk::Result::ERROR_SURFACE_LOST_KHR => Err(PresentationError::SurfaceLost),
        vk::Result::ERROR_DEVICE_LOST => Err(PresentationError::DeviceLost),
        other => Err(PresentationError::AcquireFailed {
            code: other.as_raw(),
            message: other.to_string(),
        }),
    }
}

/// Maps a failed queue present onto a status or a fatal error. Success
/// (with its suboptimal flag) travels separately, as with
/// [`map_acquire_result`].
pub(crate) fn map_present_result(result: vk::Result) -> Result<PresentStatus, PresentationError> {
    match result {
        vk::Result::SUBOPTIMAL_KHR => Ok(PresentStatus::Suboptimal),
        vk::Result::ERROR_OUT_OF_DATE_KHR => Ok(PresentStatus::Outdated),
        vk::Result::ERROR_SURFACE_LOST_KHR => Err(PresentationError::SurfaceLost),
        vk::Result::ERROR_DEVICE_LOST => Err(PresentationError::DeviceLost),
        other => Err(PresentationError::PresentFailed {
            code: other.as_raw(),
            message: other.to_string(),
        }),
    }
}

/// Converts one offered native format pair into the Canary-owned code
/// pair the pure negotiation runs on.
pub(crate) fn surface_format_of(format: vk::SurfaceFormatKHR) -> SurfaceFormat {
    SurfaceFormat {
        format_code: format.format.as_raw(),
        color_space_code: format.color_space.as_raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::{NonZeroIsize, NonZeroU32};
    use std::ptr::NonNull;

    fn xlib_handles() -> WindowHandles {
        WindowHandles::new(
            DisplayHandle::Xlib {
                display: Some(NonNull::dangling()),
                screen: 0,
            },
            WindowHandle::Xlib {
                window: 42,
                visual_id: 0,
            },
        )
    }

    fn wayland_handles() -> WindowHandles {
        WindowHandles::new(
            DisplayHandle::Wayland {
                display: NonNull::dangling(),
            },
            WindowHandle::Wayland {
                surface: NonNull::dangling(),
            },
        )
    }

    #[test]
    fn rebuilds_xlib_handles_field_for_field() {
        let rebuilt_display = rebuild_display(&xlib_handles()).expect("xlib display rebuilds");
        let rebuilt_window = rebuild_window(&xlib_handles()).expect("xlib window rebuilds");
        match rebuilt_display {
            RawDisplayHandle::Xlib(handle) => {
                assert_eq!(handle.display, Some(NonNull::dangling()));
                assert_eq!(handle.screen, 0);
            }
            other => panic!("expected an Xlib display, got {other:?}"),
        }
        match rebuilt_window {
            RawWindowHandle::Xlib(handle) => {
                assert_eq!(handle.window, 42);
                assert_eq!(handle.visual_id, 0);
            }
            other => panic!("expected an Xlib window, got {other:?}"),
        }
    }

    #[test]
    fn rebuilds_win32_and_appkit_handles() {
        let hwnd = NonZeroIsize::new(7).expect("nonzero literal");
        let handles = WindowHandles::new(
            DisplayHandle::Windows,
            WindowHandle::Win32 {
                hwnd,
                hinstance: None,
            },
        );
        match rebuild_window(&handles).expect("win32 rebuilds") {
            RawWindowHandle::Win32(handle) => {
                assert_eq!(handle.hwnd, hwnd);
                assert_eq!(handle.hinstance, None);
            }
            other => panic!("expected a Win32 window, got {other:?}"),
        }
        let view = NonNull::dangling();
        let appkit = WindowHandles::new(DisplayHandle::AppKit, WindowHandle::AppKit { view });
        match rebuild_display(&appkit).expect("appkit display rebuilds") {
            RawDisplayHandle::AppKit(_) => {}
            other => panic!("expected an AppKit display, got {other:?}"),
        }
        match rebuild_window(&appkit).expect("appkit window rebuilds") {
            RawWindowHandle::AppKit(handle) => assert_eq!(handle.ns_view, view),
            other => panic!("expected an AppKit view, got {other:?}"),
        }
    }

    #[test]
    fn rebuilds_xcb_handles_preserving_visual_ids() {
        let window = NonZeroU32::new(9).expect("nonzero literal");
        let handles = WindowHandles::new(
            DisplayHandle::Xcb {
                connection: None,
                screen: 1,
            },
            WindowHandle::Xcb {
                window,
                visual_id: None,
            },
        );
        match rebuild_display(&handles).expect("xcb display rebuilds") {
            RawDisplayHandle::Xcb(handle) => {
                assert_eq!(handle.connection, None);
                assert_eq!(handle.screen, 1);
            }
            other => panic!("expected an Xcb display, got {other:?}"),
        }
        match rebuild_window(&handles).expect("xcb window rebuilds") {
            RawWindowHandle::Xcb(handle) => {
                assert_eq!(handle.window, window);
                assert_eq!(handle.visual_id, None);
            }
            other => panic!("expected an Xcb window, got {other:?}"),
        }
    }

    #[test]
    fn accepts_matched_pairs_and_rejects_mismatches() {
        let xlib_display = rebuild_display(&xlib_handles()).expect("xlib display rebuilds");
        let xlib_window = rebuild_window(&xlib_handles()).expect("xlib window rebuilds");
        let wayland_display =
            rebuild_display(&wayland_handles()).expect("wayland display rebuilds");
        let wayland_window = rebuild_window(&wayland_handles()).expect("wayland window rebuilds");
        assert!(validate_pair(xlib_display, xlib_window).is_ok());
        assert!(validate_pair(wayland_display, wayland_window).is_ok());
        assert!(matches!(
            validate_pair(xlib_display, wayland_window),
            Err(PresentationError::UnsupportedPlatform)
        ));
        assert!(matches!(
            validate_pair(wayland_display, xlib_window),
            Err(PresentationError::UnsupportedPlatform)
        ));
    }

    #[test]
    fn maps_acquire_failures_to_statuses_or_fatal_errors() {
        assert_eq!(
            map_acquire_result(vk::Result::TIMEOUT),
            Ok(AcquireStatus::Timeout)
        );
        assert_eq!(
            map_acquire_result(vk::Result::SUBOPTIMAL_KHR),
            Ok(AcquireStatus::Suboptimal)
        );
        assert_eq!(
            map_acquire_result(vk::Result::ERROR_OUT_OF_DATE_KHR),
            Ok(AcquireStatus::Outdated)
        );
        assert!(matches!(
            map_acquire_result(vk::Result::ERROR_SURFACE_LOST_KHR),
            Err(PresentationError::SurfaceLost)
        ));
        assert!(matches!(
            map_acquire_result(vk::Result::ERROR_DEVICE_LOST),
            Err(PresentationError::DeviceLost)
        ));
        assert!(matches!(
            map_acquire_result(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
            Err(PresentationError::AcquireFailed { .. })
        ));
    }

    #[test]
    fn maps_present_failures_to_statuses_or_fatal_errors() {
        assert_eq!(
            map_present_result(vk::Result::SUBOPTIMAL_KHR),
            Ok(PresentStatus::Suboptimal)
        );
        assert_eq!(
            map_present_result(vk::Result::ERROR_OUT_OF_DATE_KHR),
            Ok(PresentStatus::Outdated)
        );
        assert!(matches!(
            map_present_result(vk::Result::ERROR_SURFACE_LOST_KHR),
            Err(PresentationError::SurfaceLost)
        ));
        assert!(matches!(
            map_present_result(vk::Result::ERROR_DEVICE_LOST),
            Err(PresentationError::DeviceLost)
        ));
        assert!(matches!(
            map_present_result(vk::Result::ERROR_OUT_OF_HOST_MEMORY),
            Err(PresentationError::PresentFailed { .. })
        ));
    }

    #[test]
    fn converts_backend_device_kinds_without_loss() {
        assert_eq!(
            device_kind_of(vk::PhysicalDeviceType::DISCRETE_GPU),
            DeviceKind::DiscreteGpu
        );
        assert_eq!(
            device_kind_of(vk::PhysicalDeviceType::INTEGRATED_GPU),
            DeviceKind::IntegratedGpu
        );
        assert_eq!(
            device_kind_of(vk::PhysicalDeviceType::VIRTUAL_GPU),
            DeviceKind::VirtualGpu
        );
        assert_eq!(device_kind_of(vk::PhysicalDeviceType::CPU), DeviceKind::Cpu);
        assert_eq!(
            device_kind_of(vk::PhysicalDeviceType::OTHER),
            DeviceKind::Other
        );
    }
}
