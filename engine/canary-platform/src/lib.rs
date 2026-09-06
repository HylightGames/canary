// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine platform abstraction.
//!
//! Layer 1 in `docs/architecture/engine-overview.md`: the only layer
//! allowed to know it's running on a specific OS. Everything above this
//! crate programs against the traits here, never against
//! `#[cfg(target_os = ...)]` directly.
//!
//! Ships a headless/null implementation (see [`HeadlessWindow`] and
//! [`HeadlessInput`]) unconditionally, plus a real `winit`-backed
//! implementation (see `winit_backend`, only present when that feature is
//! enabled) behind the `winit-backend` Cargo feature — off by default so
//! headless-only consumers (dedicated servers, most tests) never pull in
//! `winit`'s dependency tree. See
//! `docs/architecture/platform-abstraction.md` and
//! `docs/roadmap/v0.0.4-roadmap.md`.

mod headless;
mod input;
mod window;
#[cfg(feature = "winit-backend")]
pub mod winit_backend;

pub use headless::{HeadlessInput, HeadlessWindow};
pub use input::{InputEvent, InputSource, Key};
pub use window::{Window, WindowDescriptor};
