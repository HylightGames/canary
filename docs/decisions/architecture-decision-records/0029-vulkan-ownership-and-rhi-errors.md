# ADR 0029: Vulkan Ownership and RHI Error Semantics for v0.1.0

Status: Accepted
Date: 2026-10-23

## Context

Audit findings 2 and 5: `VulkanPresenter` accepted any window per call,
resources held `Rc<ash::Device>` while `VulkanDevice::drop` destroyed the
native device unconditionally, no device identity prevented cross-device
use, and RHI creation was infallible with panic/assert paths.

## Decision

- Runtime identity: `VulkanDevice` mints a `DeviceId` (process counter);
  every buffer, texture, color target, and pipeline copies it at creation.
  Encoder entry points assert ownership; presenter content path returns
  `PresentationError::DeviceLost` on mismatch with a warn log.
- Window provenance: `VulkanPresenter` stores the `WindowHandles` it was
  built for and returns `PresentationError::WrongWindow` when a later frame
  names a different window. Surface recreation across winit resume still
  works because the check compares handles, not extents/generations.
- Drop order: keep resources-first/device-last with an unconditional panic
  that now names the device id and live clone count. Full shared-ownership
  (`Rc`-shared native teardown deferred until last resource drops) is
  deferred as future work; the panic stays loud in every profile.
- RHI creation stays infallible for v0.1.0 (documented fatal semantics).
  Typed `Result` creation is the preferred long-term direction but would
  churn every caller and the render-ECS bridge; not in scope for closure.

## Consequences

- Wrong-window and cross-device bugs fail fast with typed errors or loud
  panics, never silent UB.
- Callers uphold drop order; CI adds negative tests for wrong window,
  cross-device use, and device-drop-while-live.
- Future work: shared native teardown, typed creation results, per-device
  render-pass compatibility when a second format arrives.
