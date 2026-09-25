// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Hostile-lifecycle probes: the two loud-failure contracts around resource
//! lifetime.
//!
//! `#[ignore]`d by default like `hello_triangle`: needs a real Vulkan ICD.
//! Run explicitly via
//! `cargo test -p canary-render-vulkan --test chaos -- --ignored`.

use canary_render::{ColorTargetDescriptor, RenderDevice, TextureDescriptor};
use canary_render_vulkan::VulkanDevice;

/// Creates a real Vulkan device, panicking with the ICD hint when none exists.
fn real_device() -> VulkanDevice {
    VulkanDevice::new().unwrap_or_else(|e| {
        panic!(
            "failed to create a real Vulkan device -- is a Vulkan ICD installed? \
             (this sandbox needs mesa-vulkan-drivers; see \
             docs/architecture/platform-abstraction.md): {e}"
        )
    })
}

#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
#[should_panic(expected = "dimensions must be nonzero")]
fn zero_size_color_target_panics_loudly() {
    // Given: a zero-size target descriptor.
    let device = real_device();

    // When: creation is attempted. Then: a loud host-side panic — the RHI
    // is infallible by design, so a degenerate target is refused at the
    // boundary rather than recorded as undefined GPU work.
    let _target = device.create_color_target(&ColorTargetDescriptor {
        width: 0,
        height: 0,
    });
}

#[test]
#[ignore = "needs a real Vulkan ICD (e.g. mesa-vulkan-drivers' llvmpipe); see this file's module docs"]
#[cfg(debug_assertions)]
#[should_panic(expected = "dropped while resources")]
fn device_drop_while_texture_alive_panics_in_debug() {
    // Given: a live texture holding an `Rc` clone of the device handle.
    let device = real_device();
    let _texture = device.create_texture(&TextureDescriptor {
        label: "chaos drop-order probe",
        width: 2,
        height: 2,
        rgba8: &[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    });

    // When: the device is dropped while the texture is still alive.
    // Then: the `Rc::strong_count` check fires loudly in every profile
    // (unconditional panic, not `debug_assert` — a device dropped under
    // live resources would otherwise destroy the `VkDevice` those
    // resources' own `Drop` impls still call into). This test stays
    // debug-gated only because no release job runs the `#[ignore]`
    // suites yet, not because release is silent.
    drop(device);
}
