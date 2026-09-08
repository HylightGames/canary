// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine's Vulkan RHI backend, via [`ash`] -- the first native
//! per-graphics-API backend, per
//! [ADR 0016](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0016-native-rendering-backends.md).
//! Implements [`canary_render::RenderDevice`]; nothing here leaks past
//! that trait boundary into a public type `canary-render` itself doesn't
//! already define -- see [`VulkanDevice`]'s own docs, and this crate's
//! module structure, for how that's enforced (every `ash`/`vk::*` type
//! stays `pub(crate)` at most).

mod buffer;
mod color_target;
mod device;
mod encoder;
mod pipeline;

pub use buffer::VulkanBuffer;
pub use color_target::VulkanColorTarget;
pub use device::{VulkanDevice, VulkanInitError};
pub use pipeline::VulkanPipeline;
