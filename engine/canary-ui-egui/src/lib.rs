//! `egui` implementation of the [`canary-ui-core`](canary_ui_core) backend.
//!
//! Target design: this crate is the only place that names `egui` types.
//! Game code programs against `canary-ui-core` traits; the adapter converts
//! platform events to `egui::RawInput`, runs one immediate-mode build pass
//! inside a `canary-hud` window, and converts tessellated output to
//! renderer-independent [`UiPaint`](canary_ui_core::UiPaint).
//!
//! What is stub vs target: label + button widgets, pointer/keyboard input,
//! and font-atlas texture management are target. Text fields, sliders,
//! panels, theming, and native-`pixels_per_point` tessellation arrive when
//! a second consumer needs them (the adapter tessellates at 1.0 and the
//! painter scales clip rects to the real target at submit).

mod adapter;
mod paint;
mod textures;
mod translate;

pub use adapter::EguiBackend;
pub use paint::{
    clip_to_scissor, encode_batch, ui_vertex_attributes, ui_vertex_stride, UiPaintDraw,
    UiPaintStats, UiPainter, UiViewport, UI_PAINT_WGSL,
};
