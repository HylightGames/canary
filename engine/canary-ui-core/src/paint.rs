//! Renderer-independent paint output: textured triangle soup plus clip rects.
//!
//! The adapter tessellates its widget tree into this plain data; the
//! consumer submits it through the selected Canary rendering backend
//! (`canary-render` RHI traits). Meshes arrive de-indexed: the soup render
//! path documents no index-buffer support, so adapters expand indices on
//! the CPU instead of asking the backend for an index format it lacks.

/// One UI vertex: 2D position in logical pixels, texture UV, RGBA color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiVertex {
    /// Position in logical pixels, viewport origin at the top-left.
    pub position: [f32; 2],
    /// Texture coordinates, 0.0-1.0.
    pub uv: [f32; 2],
    /// Straight (non-premultiplied) linear RGBA, 0.0-1.0 per channel.
    pub color: [f32; 4],
}

/// One de-indexed triangle: three vertices in front-to-back paint order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiTriangle {
    /// Triangle corners in paint order.
    pub vertices: [UiVertex; 3],
}

/// Clip rectangle in logical pixels: minimum edges inclusive, maximum edges
/// exclusive. The consumer converts to pixel units with `clamp_scissor`;
/// an empty or fully-offscreen rect draws nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiClipRect {
    /// Left edge, logical pixels.
    pub min_x: f32,
    /// Top edge, logical pixels.
    pub min_y: f32,
    /// Right edge, logical pixels.
    pub max_x: f32,
    /// Bottom edge, logical pixels.
    pub max_y: f32,
}

/// Backend-neutral texture identity.
///
/// Mirrors what paint consumers need (stable ids for atlas textures vs
/// user-provided images) without naming any backend or UI-library type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UiTextureId {
    /// A texture the UI backend manages (font atlas, icons it owns).
    Managed(u64),
    /// A texture the game provided (portrait, map tile).
    /// Unused by the first-game slice; reserved so ids stay unambiguous.
    User(u64),
}

/// A texture upload or release. `Set` always carries the full texture:
///
/// the adapter resolves partial updates against its own copy before
/// emitting, so the consumer never patches.
#[derive(Clone, Debug, PartialEq)]
pub enum UiTextureOp {
    /// Upload (or replace) a texture: `pixels` is `width * height` RGBA
    /// bytes, row-major from the top-left.
    Set {
        /// Which texture to write.
        id: UiTextureId,
        /// Texture width in texels.
        width: u32,
        /// Texture height in texels.
        height: u32,
        /// Full RGBA texel data.
        pixels: Vec<u8>,
    },
    /// Release a texture previously uploaded with `Set`.
    Free {
        /// Which texture to release.
        id: UiTextureId,
    },
}

/// One clipped, single-texture batch: every triangle draws with `texture`
/// under `clip`.
#[derive(Clone, Debug, PartialEq)]
pub struct UiDrawBatch {
    /// Clip rectangle for the whole batch, logical pixels.
    pub clip: UiClipRect,
    /// Texture sampled by every triangle in this batch.
    pub texture: UiTextureId,
    /// De-indexed triangles in paint order.
    pub triangles: Vec<UiTriangle>,
}

/// Whole-frame paint output: draw batches plus the texture uploads and
/// releases the frame requires. Texture ops apply before the batches that
/// reference them; releases apply after the frame presents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiPaint {
    /// Draw batches in paint order.
    pub batches: Vec<UiDrawBatch>,
    /// Texture uploads and releases for this frame.
    pub textures: Vec<UiTextureOp>,
}

impl UiPaint {
    /// `true` when the frame drew nothing and touched no textures: the
    /// consumer may skip UI submission entirely.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.batches.is_empty() && self.textures.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex() -> UiVertex {
        UiVertex {
            position: [1.0, 2.0],
            uv: [0.0, 1.0],
            color: [1.0, 1.0, 1.0, 1.0],
        }
    }

    #[test]
    fn empty_paint_reports_empty() {
        assert!(UiPaint::default().is_empty());
    }

    #[test]
    fn batch_or_texture_op_makes_paint_nonempty() {
        let clip = UiClipRect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 10.0,
            max_y: 10.0,
        };
        let with_batch = UiPaint {
            batches: vec![UiDrawBatch {
                clip,
                texture: UiTextureId::Managed(0),
                triangles: vec![UiTriangle {
                    vertices: [vertex(), vertex(), vertex()],
                }],
            }],
            textures: Vec::new(),
        };
        assert!(!with_batch.is_empty());

        let with_texture = UiPaint {
            batches: Vec::new(),
            textures: vec![UiTextureOp::Free {
                id: UiTextureId::Managed(0),
            }],
        };
        assert!(!with_texture.is_empty());
    }

    #[test]
    fn set_op_carries_full_rgba_payload() {
        let op = UiTextureOp::Set {
            id: UiTextureId::Managed(1),
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        if let UiTextureOp::Set {
            width,
            height,
            pixels,
            ..
        } = op
        {
            assert_eq!((width, height), (2, 1));
            assert_eq!(pixels.len(), 8);
        } else {
            panic!("expected a Set op");
        }
    }
}
