//! Canary Engine platform abstraction.
//!
//! Layer 1 in `docs/architecture/engine-overview.md`: the only layer
//! allowed to know it's running on a specific OS. Everything above this
//! crate programs against the traits here, never against
//! `#[cfg(target_os = ...)]` directly.
//!
//! v0.0.1-pre1 ships only trait definitions plus a headless/null
//! implementation (see [`HeadlessWindow`] and [`HeadlessInput`]) — no real
//! windowing backend yet. See
//! `docs/architecture/platform-abstraction.md` for why, and
//! `docs/roadmap/v0.0.1-roadmap.md` for when a `winit`-backed
//! implementation is planned.

mod headless;
mod input;
mod window;

pub use headless::{HeadlessInput, HeadlessWindow};
pub use input::{InputEvent, InputSource, Key};
pub use window::{Window, WindowDescriptor};
