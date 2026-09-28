//! Backend-neutral UI boundary for the Canary Engine.
//!
//! Target design: game and editor code depends on this crate's traits and
//! data types (`UiBackend`, `UiBuilder`, `CaptureResult`, `UiIntent`,
//! `UiPaint`) and never on a concrete UI backend. Concrete adapters live in
//! their own crates (first: `canary-ui-egui`) and convert to Canary-owned
//! render data submitted through `canary-render`.
//!
//! What is stub vs target: the event/capture/intent/paint model here is the
//! target contract for the v0.0.13 first-game slice (HUD text + button over
//! a scene, immutable game view in, intents out). Richer widgets (text
//! fields, sliders, panels), theming, and layout abstractions arrive when a
//! second consumer needs them — not before.
//!
//! Boundary rules (ADR 0011, `docs/architecture/ui-toolkit.md`):
//!
//! - No `egui`, `winit`, `ash`, Vulkan, or `raw-window-handle` types appear
//!   in any public signature of this crate.
//! - This crate does not depend on `canary-render`: paint output is plain
//!   data; adapters and consumers submit it through the selected backend.
//! - Gameplay input never reads backend capture flags; it reads
//!   [`CaptureResult`], judged per event at mapping time by `canary-input`.

pub mod backend;
pub mod capture;
pub mod intent;
pub mod paint;

pub use backend::{NullBackend, UiBackend, UiBuilder, UiFrameInput, UiFrameOutput};
pub use capture::CaptureResult;
pub use intent::{UiId, UiIntent, UiIntents};
pub use paint::{UiClipRect, UiDrawBatch, UiPaint, UiTextureId, UiTextureOp, UiTriangle, UiVertex};
