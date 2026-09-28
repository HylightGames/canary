//! `egui` texture deltas to Canary-owned texture ops.
//!
//! The adapter resolves partial updates against its own copy before
//! emitting: consumers only ever see full-texture `Set` ops plus `Free`
//! ops, never patches.

use std::collections::HashMap;

use canary_ui_core::{UiTextureId, UiTextureOp};
use egui::epaint::image::ImageData;

/// Converts one `egui` image to full RGBA bytes plus dimensions.
///
/// `egui` 0.36 removed the separate font-image type: the font atlas
/// arrives pre-rasterized as RGBA like every other image, so texels copy
/// straight across (gamma-encoded bytes stay gamma-encoded — sampling
/// behavior is the deferred gamma stance's problem, not the adapter's).
pub(crate) fn image_to_rgba(image: &ImageData) -> (usize, usize, Vec<u8>) {
    let ImageData::Color(color) = image;
    let rgba = color
        .pixels
        .iter()
        .flat_map(|texel| texel.to_array())
        .collect();
    (color.size[0], color.size[1], rgba)
}

/// Adapter-side texture store: `egui` speaks in deltas (full uploads and
/// partial patches, possibly several per texture per frame), Canary
/// consumers speak in full textures.
#[derive(Debug, Default)]
pub(crate) struct TextureCache {
    /// Live textures: id to `(width, height, full RGBA bytes)`.
    textures: HashMap<UiTextureId, (usize, usize, Vec<u8>)>,
}

impl TextureCache {
    /// Applies one frame's texture delta, returning Canary-owned ops: one
    /// full `Set` per touched texture (patches merged in order), then one
    /// `Free` per released texture. Takes the delta by value: the returned
    /// ops are the consumed form of the delta. A patch for an unknown
    /// texture falls back to uploading the patch image as-is (documents an
    /// `egui`-side ordering violation rather than dropping the upload).
    pub(crate) fn apply(&mut self, mut delta: egui::TexturesDelta) -> Vec<UiTextureOp> {
        let mut ops = Vec::with_capacity(delta.set.len() + delta.free.len());
        for (id, image_deltas) in &delta.set {
            let ui_id = map_texture_id(*id);
            for image_delta in image_deltas {
                let (width, height, rgba) = image_to_rgba(&image_delta.image);
                let (full_width, full_height, full_rgba) = match image_delta.pos {
                    None => (width, height, rgba),
                    Some([x, y]) => self.patch(ui_id, x, y, width, height, rgba),
                };
                self.textures
                    .insert(ui_id, (full_width, full_height, full_rgba));
            }
            let (width, height, rgba) = self.textures.get(&ui_id).cloned().unwrap_or_default();
            ops.push(UiTextureOp::Set {
                id: ui_id,
                width: saturating_u32(width, "texture width"),
                height: saturating_u32(height, "texture height"),
                pixels: rgba,
            });
        }
        for id in &delta.free {
            let ui_id = map_texture_id(*id);
            self.textures.remove(&ui_id);
            ops.push(UiTextureOp::Free { id: ui_id });
        }
        // Consumed: `egui` debug-asserts that painters take their deltas.
        delta.clear();
        ops
    }

    /// Merges a partial patch into the cached texture, returning the full
    /// texture. A patch that does not fit the cached texture (or arrives
    /// with no cached texture) falls back to the patch image as-is.
    fn patch(
        &self,
        id: UiTextureId,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    ) -> (usize, usize, Vec<u8>) {
        let Some((cached_width, cached_height, cached)) = self.textures.get(&id) else {
            return (width, height, rgba);
        };
        if x + width > *cached_width || y + height > *cached_height {
            return (width, height, rgba);
        }
        let mut full = cached.clone();
        let row_bytes = width * 4;
        for row in 0..height {
            let from = row * row_bytes;
            let to = ((y + row) * cached_width + x) * 4;
            full[to..to + row_bytes].copy_from_slice(&rgba[from..from + row_bytes]);
        }
        (*cached_width, *cached_height, full)
    }
}

/// Saturates a `usize` image dimension to `u32`: atlas dimensions never
/// approach the boundary in practice, and saturating beats refusing to
/// upload the font atlas over a dimension conversion.
fn saturating_u32(value: usize, what: &str) -> u32 {
    u32::try_from(value).unwrap_or_else(|_| {
        debug_assert!(false, "{what} {value} exceeds u32; saturating");
        u32::MAX
    })
}

/// Maps an `egui` texture id to the Canary-owned counterpart.
pub(crate) fn map_texture_id(id: egui::TextureId) -> UiTextureId {
    match id {
        egui::TextureId::Managed(native) => UiTextureId::Managed(native),
        egui::TextureId::User(native) => UiTextureId::User(native),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use egui::epaint::image::ImageDelta;

    fn color_image() -> ImageData {
        ImageData::Color(Arc::new(egui::ColorImage::new(
            [2, 1],
            vec![
                egui::Color32::RED,
                egui::Color32::from_rgba_unmultiplied(0, 0, 255, 255),
            ],
        )))
    }

    fn full_delta(image: ImageData) -> egui::TexturesDelta {
        // Built via `default` + `insert`: `TexturesDelta` uses an `ahash`
        // map, whose hasher type this crate must not name.
        let mut delta = egui::TexturesDelta::default();
        delta.set.insert(
            egui::TextureId::Managed(0),
            [ImageDelta {
                image,
                pos: None,
                options: egui::TextureOptions::default(),
            }]
            .into_iter()
            .collect(),
        );
        delta
    }

    #[test]
    fn color_image_converts_texel_for_texel() {
        let (width, height, rgba) = image_to_rgba(&color_image());
        assert_eq!((width, height), (2, 1));
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn full_upload_emits_set_and_caches() {
        let mut cache = TextureCache::default();
        let ops = cache.apply(full_delta(color_image()));
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            &ops[0],
            UiTextureOp::Set {
                width: 2,
                height: 1,
                ..
            }
        ));
    }

    #[test]
    fn partial_patch_updates_cache_and_emits_full_set() {
        let mut cache = TextureCache::default();
        cache.apply(full_delta(color_image()));

        let patch = ImageData::Color(Arc::new(egui::ColorImage::new(
            [1, 1],
            vec![egui::Color32::GREEN],
        )));
        let mut delta = egui::TexturesDelta::default();
        delta.set.insert(
            egui::TextureId::Managed(0),
            [ImageDelta {
                image: patch,
                pos: Some([1, 0]),
                options: egui::TextureOptions::default(),
            }]
            .into_iter()
            .collect(),
        );
        let ops = cache.apply(delta);
        assert_eq!(ops.len(), 1);
        if let UiTextureOp::Set { pixels, .. } = &ops[0] {
            // First texel untouched, second texel now green.
            assert_eq!(&pixels[0..4], &[255, 0, 0, 255]);
            assert_eq!(&pixels[4..8], &[0, 255, 0, 255]);
        } else {
            panic!("expected a Set op");
        }
    }

    #[test]
    fn free_emits_free_and_evicts() {
        let mut cache = TextureCache::default();
        cache.apply(full_delta(color_image()));
        let mut delta = egui::TexturesDelta::default();
        delta.free.insert(egui::TextureId::Managed(0));
        let ops = cache.apply(delta);
        assert_eq!(
            ops,
            vec![UiTextureOp::Free {
                id: UiTextureId::Managed(0)
            }]
        );
    }

    #[test]
    fn managed_and_user_ids_map_to_matching_variants() {
        assert_eq!(
            map_texture_id(egui::TextureId::Managed(7)),
            UiTextureId::Managed(7)
        );
        assert_eq!(
            map_texture_id(egui::TextureId::User(3)),
            UiTextureId::User(3)
        );
    }
}
