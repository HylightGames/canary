// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Benchmarks for the asset layer: content-hash identity, the
//! generational [`AssetStore`], and PNG decoding.
//!
//! Fixtures are synthesized in-memory (and, for the decode benchmark,
//! written to a temporary file) rather than read from
//! `tests/fixtures/`: those are Git-LFS objects, and a benchmark that
//! silently measures 130 bytes of LFS pointer text when LFS is missing
//! would be worse than no benchmark at all.
//!
//! Run locally with `cargo bench -p canary-assets`; in CI these are
//! measured by CodSpeed (see `.github/workflows/codspeed.yml`).

use std::io::BufWriter;
use std::path::PathBuf;

use canary_assets::{load_texture, AssetHandle, AssetId, AssetStore};
use divan::{black_box, Bencher};

fn main() {
    divan::main();
}

/// Payload sizes (bytes) the content-hash benchmarks run at: a small
/// asset (a shader or a tiny texture) and a megabyte-scale one (a mesh).
const PAYLOAD_SIZES: &[usize] = &[4 * 1024, 1024 * 1024];

/// Asset counts the store benchmarks run at.
const ASSET_COUNTS: &[usize] = &[1_000, 10_000];

/// Square texture sizes (pixels per side) the PNG decode benchmark runs
/// at.
const TEXTURE_SIDES: &[u32] = &[64, 256];

/// A deterministic, non-uniform byte payload: uniform bytes would let
/// both the hash and (for textures) the PNG filters see an unrealistically
/// friendly input.
fn payload(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| (index.wrapping_mul(31).wrapping_add(7) % 251) as u8)
        .collect()
}

/// One stand-in asset: a small owned buffer, so the store benchmarks
/// measure slot bookkeeping rather than payload copying.
#[derive(Debug, Clone)]
struct StoredAsset {
    id: AssetId,
    bytes: Vec<u8>,
}

/// A store holding `count` assets, plus every handle it issued.
fn populated_store(count: usize) -> (AssetStore<StoredAsset>, Vec<AssetHandle<StoredAsset>>) {
    let mut store = AssetStore::new();
    let mut handles = Vec::with_capacity(count);
    for index in 0..count {
        let bytes = index.to_le_bytes().to_vec();
        handles.push(store.insert(StoredAsset {
            id: AssetId::new(&bytes),
            bytes,
        }));
    }
    (store, handles)
}

/// Writes a synthetic RGBA8 PNG of `side`x`side` pixels into `dir` and
/// returns its path.
fn write_png(dir: &std::path::Path, side: u32) -> PathBuf {
    let path = dir.join(format!("synthetic-{side}.png"));
    let file = std::fs::File::create(&path).expect("temporary directory is writable");
    let mut encoder = png::Encoder::new(BufWriter::new(file), side, side);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("PNG header is well-formed");
    let pixels = payload((side as usize) * (side as usize) * 4);
    writer
        .write_image_data(&pixels)
        .expect("pixel buffer matches the declared dimensions");
    writer.finish().expect("PNG stream finishes cleanly");
    path
}

/// Content hashing: the cost of identifying an asset by its bytes, which
/// every load pays once.
#[divan::bench(args = PAYLOAD_SIZES)]
fn asset_id_from_bytes(bencher: Bencher, size: usize) {
    let bytes = payload(size);
    bencher.bench_local(|| black_box(AssetId::new(black_box(&bytes))));
}

/// Rendering an ID as hex: what logs, cache filenames, and manifests use.
#[divan::bench]
fn asset_id_to_hex() {
    let id = AssetId::new(b"canary-asset-id-benchmark");
    black_box(black_box(&id).to_hex());
}

/// Parsing an ID back from hex — the manifest-read side of the round trip.
#[divan::bench]
fn asset_id_from_hex() {
    let hex = AssetId::new(b"canary-asset-id-benchmark").to_hex();
    black_box(AssetId::from_hex(black_box(&hex))).expect("round trip is valid hex");
}

/// Filling a store from empty: slot allocation with no free list to reuse.
#[divan::bench(args = ASSET_COUNTS)]
fn store_insert(bencher: Bencher, count: usize) {
    bencher.bench_local(|| black_box(populated_store(count)));
}

/// Handle resolution: the generational liveness check plus the lookup,
/// which extract systems run once per renderable per frame.
#[divan::bench(args = ASSET_COUNTS)]
fn store_get(bencher: Bencher, count: usize) {
    let (store, handles) = populated_store(count);
    bencher.bench_local(|| {
        let mut live = 0usize;
        for handle in &handles {
            if let Some(asset) = store.get(*handle) {
                black_box(asset.id);
                live += asset.bytes.len();
            }
        }
        black_box(live)
    });
}

/// Resolving handles that have all gone stale: the reject path, which a
/// hot loop hits whenever content is being swapped out underneath it.
#[divan::bench(args = ASSET_COUNTS)]
fn store_get_stale_handles(bencher: Bencher, count: usize) {
    let (mut store, handles) = populated_store(count);
    for handle in &handles {
        store.remove(*handle);
    }
    bencher.bench_local(|| {
        let mut live = 0usize;
        for handle in &handles {
            if store.get(*handle).is_some() {
                live += 1;
            }
        }
        black_box(live)
    });
}

/// A full unload/reload cycle: removals bump generations and feed the
/// free list, and the reinserts have to recycle from it.
#[divan::bench(args = ASSET_COUNTS)]
fn store_remove_and_reinsert(bencher: Bencher, count: usize) {
    bencher
        .with_inputs(|| populated_store(count))
        .bench_local_values(|(mut store, handles)| {
            for handle in &handles {
                black_box(store.remove(*handle));
            }
            for index in 0..handles.len() {
                let bytes = index.to_le_bytes().to_vec();
                black_box(store.insert(StoredAsset {
                    id: AssetId::new(&bytes),
                    bytes,
                }));
            }
            store
        });
}

/// PNG decode through the real loader: read, decode, normalize to RGBA8,
/// and budget-check — the whole texture-load path a level load repeats
/// once per image.
#[divan::bench(args = TEXTURE_SIDES)]
fn load_png_texture(bencher: Bencher, side: u32) {
    let dir = tempfile::tempdir().expect("a temporary directory is available");
    let path = write_png(dir.path(), side);
    bencher.bench_local(|| {
        let texture = load_texture(black_box(&path)).expect("synthetic PNG decodes");
        black_box(texture.rgba8().len())
    });
}
