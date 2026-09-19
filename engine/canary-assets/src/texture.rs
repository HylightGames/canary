// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! PNG texture loading: file bytes become a validated [`Texture`] value.
//!
//! The one format this release reads is PNG, decoded by the pure-Rust
//! `png` crate (ADR 0018) — not the full `image` crate, whose dozen
//! formats are attack surface, pins, and compile time this release's
//! "at least one texture format" bar does not need. As with the mesh
//! loader, third-party types never escape this module: the public surface
//! is [`Texture`] (plain integers plus one byte vector) and
//! [`load_texture`] / [`load_texture_with_budget`] (a path in, validated
//! values out).
//!
//! Fixture provenance: `tests/fixtures/rgba2x2.png` (four known RGBA
//! pixels) and `tests/fixtures/rgba16-2x1.png` (16-bit samples proving the
//! documented downsampling) are checked in and hash-stable; see
//! `tests/fixtures/README.md` for how they were generated. Tests assert
//! loader *output bytes* against checked-in expectations — never hashes of
//! toolchain behavior.

use std::io::Cursor;
use std::path::Path;

use crate::AssetError;

/// Default ceiling for the decoded size of one texture, in bytes.
///
/// Sixty-four mebibytes matches the `png` crate's own default
/// (`Limits::default()`), so the explicit pre-check in
/// [`load_texture_with_budget`] and the decoder-internal limit agree with
/// each other out of the box rather than encoding two different opinions
/// about "too big". The value is **provisional** — a DoS bound chosen
/// before any real content exists to calibrate against, exactly like the
/// mesh budgets in the `mesh` module — and callers with measured needs pass
/// their own ceiling to [`load_texture_with_budget`] instead.
pub const DEFAULT_MAX_TEXTURE_BYTES: u64 = 64 * 1024 * 1024;

/// Texture loaded from a PNG file: dimensions plus RGBA8 pixel data.
///
/// `rgba8` always holds `width * height * 4` bytes in top-to-bottom,
/// left-to-right row-major order — the same layout the Phase 3b renderer
/// slice uploads to the GPU, so upload stays a `memcpy` with no
/// reshuffling at the graphics boundary.
///
/// Why RGBA8-normalized: every PNG color type (grayscale, palette, RGB,
/// with or without alpha or transparency chunks) converts to one layout
/// at load time, so no consumer ever branches on "which PNG flavor was
/// this?". The conversion is total and documented (see
/// [`load_texture_with_budget`]), never a silent reinterpretation of the
/// source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
}

impl Texture {
    /// Pixel width of the decoded image.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Pixel height of the decoded image.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// RGBA8 pixel data, `width * height * 4` bytes, row-major from the top
    /// row.
    ///
    /// The length invariant is established at load time (see
    /// [`load_texture_with_budget`]), so consumers may chunk this into
    /// fours without re-validating.
    pub fn rgba8(&self) -> &[u8] {
        &self.rgba8
    }

    /// The RGBA pixel at column `x`, row `y` (origin at the top-left), or
    /// `None` when the coordinates fall outside the image.
    ///
    /// Exists so tests — and later the renderer bridge — can name one
    /// pixel without hand-rolling row-major index arithmetic (and its
    /// attendant width/height transposition bugs) at every call site.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let x = usize::try_from(x).ok()?;
        let y = usize::try_from(y).ok()?;
        let width = usize::try_from(self.width).ok()?;
        if x >= width || y >= usize::try_from(self.height).ok()? {
            return None;
        }
        let offset = (y * width + x) * 4;
        let bytes = self.rgba8.get(offset..offset + 4)?;
        Some([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}

/// Loads the PNG file at `path` as a [`Texture`], enforcing
/// [`DEFAULT_MAX_TEXTURE_BYTES`].
///
/// This is the entry point most callers want: one path in, one validated
/// texture out, with the workspace-standard budget. Callers that have
/// measured their content (or tests proving the over-budget path) use
/// [`load_texture_with_budget`] with an explicit ceiling instead.
///
/// See [`load_texture_with_budget`] for the conversion contract and the
/// full failure taxonomy.
pub fn load_texture(path: &Path) -> Result<Texture, AssetError> {
    load_texture_with_budget(path, DEFAULT_MAX_TEXTURE_BYTES)
}

/// Loads the PNG file at `path` as a [`Texture`], refusing decodes whose
/// output would exceed `max_decode_bytes`.
///
/// Conversion contract — every PNG color type normalizes to RGBA8:
/// - RGBA8 passes through byte-identical.
/// - RGB8 gains an opaque alpha channel (every pixel's fourth byte is
///   255): the file said "no transparency", and opaque alpha is what "no
///   transparency" means in RGBA space, not a guess.
/// - Grayscale (any bit depth at or below 8) replicates its sample across
///   R, G, and B with opaque alpha; grayscale-with-alpha replicates
///   across RGB and keeps its alpha.
/// - Palette images resolve through their PLTE (and tRNS, when present)
///   chunks to the same RGB(A) values the palette names.
/// - Samples deeper than 8 bits keep only their most significant byte
///   (documented downsampling, not silent truncation: this matches the
///   `STRIP_16` convention the `png` crate itself applies, where the high
///   byte is the value an 8-bit consumer would have seen).
///
/// All of the above is performed by the decoder's `normalize_to_color8`
/// transformation plus one small RGB-to-RGBA expansion in this module, so
/// the eight PNG color-type/bit-depth combinations collapse to four
/// decoder outputs and then to one stored layout.
///
/// Budget contract — refused *before* allocating: the header's claimed
/// dimensions are checked against `max_decode_bytes` first (a 1x1 file
/// claiming gigapixel dimensions fails here, with both numbers attached),
/// and the same ceiling is handed to the decoder as its internal
/// allocation limit, so intermediate buffers are bounded too. Units are
/// output RGBA bytes throughout, which is what [`AssetError::OverBudget`]
/// reports.
///
/// Other behavior worth knowing rather than discovering:
/// - Interlaced (Adam7) PNGs decode transparently — the decoder
///   de-interlaces, so there is nothing for this loader to reject.
/// - Animated PNGs yield their first frame only; animation control chunks
///   are ignored. Frame sequencing belongs to a later asset milestone,
///   not to a loader whose contract is "one file, one texture".
///
/// Failure taxonomy (all variants carry `path`):
/// - Missing/unreadable file → [`AssetError::Io`].
/// - Bad signature, corrupt chunks, truncated data, decoder output that
///   disagrees with the header → [`AssetError::InvalidFormat`].
/// - Claimed or actual size past `max_decode_bytes` →
///   [`AssetError::OverBudget`].
/// - Decoder output in a layout this loader does not expand (reachable
///   only if the decoder's own transformation contract changes under a
///   future `png` bump) → [`AssetError::UnsupportedFeature`].
///
/// Synchronous and panic-free over file inputs, like `crate::mesh`:
/// malformed files produce `Err`, and there is no `async` API (ADR 0018).
pub fn load_texture_with_budget(path: &Path, max_decode_bytes: u64) -> Result<Texture, AssetError> {
    let invalid = |reason: String| AssetError::invalid_format(path, reason);

    let bytes = std::fs::read(path).map_err(|source| AssetError::io(path, source))?;

    let mut decoder = png::Decoder::new(Cursor::new(bytes.as_slice()));
    decoder.set_limits(png::Limits {
        bytes: usize::try_from(max_decode_bytes).unwrap_or(usize::MAX),
    });
    decoder.set_transformations(png::Transformations::normalize_to_color8());

    // The header arrives before any pixel allocation, so its claimed
    // dimensions are the earliest place a lying file can be refused.
    // `read_header_info` borrows the decoder mutably and returns header
    // metadata; the decoder itself is consumed by `read_info` below.
    let (header_width, header_height) = {
        let info = decoder
            .read_header_info()
            .map_err(|source| invalid(format!("not a parseable PNG file: {source}")))?;
        (info.width, info.height)
    };
    check_rgba8_budget(path, max_decode_bytes, header_width, header_height)?;

    let mut reader = decoder
        .read_info()
        .map_err(|source| map_decode_error(path, max_decode_bytes, source))?;
    let (width, height) = (reader.info().width, reader.info().height);
    let decoded_size = check_rgba8_budget(path, max_decode_bytes, width, height)?;

    // `output_buffer_size` is `None` exactly when the frame does not fit
    // the address space the decoder was told about — another face of
    // over-budget, reported as such rather than as a corrupt file.
    let buffer_size = reader
        .output_buffer_size()
        .ok_or_else(|| AssetError::over_budget(path, max_decode_bytes, u64::MAX))?;
    // `try_reserve`, not `vec![0; n]`: a huge (caller-approved) budget
    // must surface as `Err` on allocation failure, never as an allocator
    // abort, keeping the loader's total-over-inputs promise.
    let mut raw = Vec::new();
    raw.try_reserve(buffer_size)
        .map_err(|_| AssetError::over_budget(path, max_decode_bytes, buffer_size as u64))?;
    raw.resize(buffer_size, 0);
    // `next_frame` decodes the first frame and reports the layout it
    // actually wrote — the authoritative input to the expansion below,
    // not the header's pre-decode description of it.
    let output = reader
        .next_frame(&mut raw)
        .map_err(|source| map_decode_error(path, max_decode_bytes, source))?;

    let rgba8 = expand_to_rgba8(path, &raw, &output)?;
    let expected = usize::try_from(decoded_size).unwrap_or(usize::MAX);
    if rgba8.len() != expected {
        return Err(invalid(format!(
            "decoded {} bytes but {width}x{height} RGBA8 needs {expected}",
            rgba8.len()
        )));
    }

    Ok(Texture {
        width,
        height,
        rgba8,
    })
}

/// RGBA8 byte count for a `width` by `height` image, or `None` on
/// arithmetic overflow.
///
/// Returns `None` instead of wrapping because the inputs are `u32`s from
/// an untrusted header: `u32::MAX` by `u32::MAX` pixels times 4 overflows
/// even `u64`. Callers treat `None` as "larger than any budget".
fn rgba8_byte_count(width: u32, height: u32) -> Option<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
}

/// Refuses an image whose RGBA8 byte count exceeds `max_decode_bytes`.
///
/// Shared by the header pre-check (claimed dimensions, refused before any
/// pixel allocation) and the post-decode check (actual dimensions): both
/// gates report [`AssetError::OverBudget`] with the same numbers, and an
/// overflowing dimension product counts as larger than any budget. One
/// function so the two gates cannot drift into different taxonomies for
/// the same condition.
fn check_rgba8_budget(
    path: &Path,
    max_decode_bytes: u64,
    width: u32,
    height: u32,
) -> Result<u64, AssetError> {
    match rgba8_byte_count(width, height) {
        None => Err(AssetError::over_budget(path, max_decode_bytes, u64::MAX)),
        Some(size) if size > max_decode_bytes => {
            Err(AssetError::over_budget(path, max_decode_bytes, size))
        }
        Some(size) => Ok(size),
    }
}

/// Maps a `png` crate decode failure onto the [`AssetError`] taxonomy.
///
/// `LimitsExceeded` is the decoder's own name for "the stated budget
/// refused this allocation", so it becomes [`AssetError::OverBudget`] with
/// the numbers attached; every other decode failure means the bytes are
/// damaged or truncated ([`AssetError::InvalidFormat`]). The `png` crate's
/// error type stays inside this function: it never appears in a public
/// signature.
fn map_decode_error(path: &Path, max_decode_bytes: u64, source: png::DecodingError) -> AssetError {
    if matches!(source, png::DecodingError::LimitsExceeded) {
        // The decoder refused to say how big the image claimed to be, so
        // `u64::MAX` records honestly that the size was never established.
        return AssetError::over_budget(path, max_decode_bytes, u64::MAX);
    }
    AssetError::invalid_format(path, format!("PNG decode failed: {source}"))
}

/// Expands one decoded PNG frame into RGBA8 bytes.
///
/// After `normalize_to_color8`, the decoder emits exactly four layouts —
/// 8-bit grayscale, grayscale-with-alpha, RGB, or RGBA — and this function
/// is the single place that knows that. Anything else means the decoder's
/// transformation contract changed under a future `png` bump, which is a
/// loader-narrowness failure ([`AssetError::UnsupportedFeature`]), not a
/// corrupt file.
fn expand_to_rgba8(
    path: &Path,
    raw: &[u8],
    output: &png::OutputInfo,
) -> Result<Vec<u8>, AssetError> {
    use png::{BitDepth, ColorType};
    // All arithmetic in `u64`: `u32` dimensions from an untrusted header
    // multiply past `usize` on 32-bit targets, and this module refuses to
    // depend on pointer width for its safety reasoning.
    let pixel_count = u64::from(output.width).checked_mul(u64::from(output.height));
    let Some(pixel_count) = pixel_count else {
        return Err(AssetError::over_budget(path, u64::MAX, u64::MAX));
    };

    // Bytes per pixel of the decoder's layout per arm below; the RGBA8
    // expansion always writes 4.
    let bytes_per_pixel: u64 = match (output.color_type, output.bit_depth) {
        (ColorType::Rgba, BitDepth::Eight) => 4,
        (ColorType::Rgb, BitDepth::Eight) => 3,
        (ColorType::Grayscale, BitDepth::Eight) => 1,
        (ColorType::GrayscaleAlpha, BitDepth::Eight) => 2,
        (color_type, bit_depth) => {
            return Err(AssetError::unsupported_feature(
                path,
                format!("PNG decoder output {color_type:?} at {bit_depth:?}"),
            ));
        }
    };
    let needed = pixel_count
        .checked_mul(bytes_per_pixel)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| AssetError::over_budget(path, u64::MAX, u64::MAX))?;
    if raw.len() < needed {
        return Err(AssetError::invalid_format(
            path,
            format!(
                "{:?} frame holds {} bytes but needs {needed}",
                output.color_type,
                raw.len()
            ),
        ));
    }
    let frame = &raw[..needed];

    match (output.color_type, output.bit_depth) {
        (ColorType::Rgba, BitDepth::Eight) => Ok(frame.to_vec()),
        (ColorType::Rgb, BitDepth::Eight) => {
            let mut rgba = Vec::with_capacity(pixel_count_as_usize(path, pixel_count)? * 4);
            for pixel in frame.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 0xFF]);
            }
            Ok(rgba)
        }
        (ColorType::Grayscale, BitDepth::Eight) => {
            let mut rgba = Vec::with_capacity(pixel_count_as_usize(path, pixel_count)? * 4);
            for sample in frame {
                rgba.extend_from_slice(&[*sample, *sample, *sample, 0xFF]);
            }
            Ok(rgba)
        }
        (ColorType::GrayscaleAlpha, BitDepth::Eight) => {
            let mut rgba = Vec::with_capacity(pixel_count_as_usize(path, pixel_count)? * 4);
            for pixel in frame.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            Ok(rgba)
        }
        // Exhaustiveness re-check: the `bytes_per_pixel` match above
        // already rejected every other layout, so this arm is unreachable —
        // but the compiler cannot see that, and an explicit unreachable
        // message beats silently mis-decoding a future layout.
        (color_type, bit_depth) => Err(AssetError::unsupported_feature(
            path,
            format!("PNG decoder output {color_type:?} at {bit_depth:?}"),
        )),
    }
}

/// Converts a `u64` pixel count into a `usize` for allocation, refusing
/// with [`AssetError::OverBudget`] when the count does not fit the
/// address space.
///
/// `u64` arithmetic above is deliberately pointer-width independent, so
/// the narrowing back to `usize` happens here, once, with a typed error
/// instead of a wrap or a platform-dependent `as` cast.
fn pixel_count_as_usize(path: &Path, pixel_count: u64) -> Result<usize, AssetError> {
    usize::try_from(pixel_count).map_err(|_| AssetError::over_budget(path, u64::MAX, pixel_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Checked-in fixture path (see `mesh.rs` tests for why
    /// `CARGO_MANIFEST_DIR` instead of a relative path).
    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// Scratch file for runtime-built inputs, unique per test name so
    /// parallel tests never share a path.
    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("canary-assets-texture-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("scratch fixture must be writable");
        path
    }

    /// Encodes a tiny PNG in-memory with the `png` crate's own encoder so
    /// grayscale/RGB/palette expansion arms are exercised without checking
    /// in one fixture per color type. Deterministic: fixed encoder
    /// settings, no timestamps in the format.
    fn encode_png(
        width: u32,
        height: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        let mut writer = encoder
            .write_header()
            .expect("in-memory encode must succeed");
        writer
            .write_image_data(pixels)
            .expect("in-memory encode must succeed");
        drop(writer);
        out
    }

    #[test]
    fn rgba_fixture_decodes_to_known_bytes() {
        let texture = load_texture(&fixture("rgba2x2.png")).expect("RGBA fixture must load");
        assert_eq!(texture.width(), 2);
        assert_eq!(texture.height(), 2);
        assert_eq!(
            texture.rgba8(),
            &[
                255, 0, 0, 255, // top-left: red
                0, 255, 0, 255, // top-right: green
                0, 0, 255, 255, // bottom-left: blue
                255, 255, 255, 255, // bottom-right: white
            ],
            "row-major top-to-bottom bytes per the fixtures README"
        );
    }

    #[test]
    fn pixel_accessor_names_corners_and_rejects_outside() {
        let texture = load_texture(&fixture("rgba2x2.png")).expect("RGBA fixture must load");
        assert_eq!(texture.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(texture.pixel(1, 0), Some([0, 255, 0, 255]));
        assert_eq!(texture.pixel(0, 1), Some([0, 0, 255, 255]));
        assert_eq!(texture.pixel(1, 1), Some([255, 255, 255, 255]));
        assert_eq!(texture.pixel(2, 0), None, "past the right edge");
        assert_eq!(texture.pixel(0, 2), None, "past the bottom edge");
    }

    #[test]
    fn sixteen_bit_samples_downsample_to_most_significant_byte() {
        let texture = load_texture(&fixture("rgba16-2x1.png")).expect("16-bit fixture must load");
        assert_eq!((texture.width(), texture.height()), (2, 1));
        assert_eq!(
            texture.rgba8(),
            &[255, 0, 128, 255, 0x12, 0x56, 0x9A, 0xDE],
            "STRIP_16 keeps the high byte: 0x8000 -> 0x80, 0x1234 -> 0x12, ..."
        );
    }

    #[test]
    fn grayscale_expands_across_rgb_with_opaque_alpha() {
        let bytes = encode_png(
            2,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Eight,
            &[0x00, 0x7F],
        );
        let path = write_temp("gray.png", &bytes);
        let texture = load_texture(&path).expect("grayscale must load");
        assert_eq!(texture.rgba8(), &[0, 0, 0, 255, 127, 127, 127, 255]);
    }

    #[test]
    fn rgb_gains_opaque_alpha() {
        let bytes = encode_png(
            1,
            1,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &[10, 20, 30],
        );
        let path = write_temp("rgb.png", &bytes);
        let texture = load_texture(&path).expect("RGB must load");
        assert_eq!(texture.rgba8(), &[10, 20, 30, 255]);
    }

    #[test]
    fn grayscale_alpha_keeps_its_alpha() {
        let bytes = encode_png(
            1,
            1,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            &[0x40, 0x80],
        );
        let path = write_temp("gray-alpha.png", &bytes);
        let texture = load_texture(&path).expect("grayscale-alpha must load");
        assert_eq!(texture.rgba8(), &[0x40, 0x40, 0x40, 0x80]);
    }

    #[test]
    fn missing_file_is_io_not_a_panic() {
        let err = load_texture(Path::new("definitely-not-a-texture.png"))
            .expect_err("missing file must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "missing file must be Io, got: {err:?}"
        );
    }

    #[test]
    fn garbage_bytes_are_invalid_format() {
        let path = write_temp("garbage.png", b"this is not a png file");
        let err = load_texture(&path).expect_err("garbage must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "garbage must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn truncated_fixture_is_invalid_format() {
        let full = std::fs::read(fixture("rgba2x2.png")).expect("RGBA fixture must exist");
        let path = write_temp("truncated.png", &full[..30]);
        let err = load_texture(&path).expect_err("truncated file must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "truncated file must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn tiny_budget_refuses_before_decoding() {
        // The 2x2 RGBA fixture needs exactly 16 output bytes; a ceiling of
        // 15 must fail with both numbers attached, not decode anyway.
        let err = load_texture_with_budget(&fixture("rgba2x2.png"), 15)
            .expect_err("15-byte budget must fail a 16-byte image");
        match err {
            AssetError::OverBudget { limit, actual, .. } => {
                assert_eq!(limit, 15);
                assert_eq!(actual, 16);
            }
            other => panic!("must be OverBudget, got: {other:?}"),
        }
    }

    #[test]
    fn exact_budget_is_enough() {
        let texture = load_texture_with_budget(&fixture("rgba2x2.png"), 16)
            .expect("a budget of exactly the output size must succeed");
        assert_eq!(texture.rgba8().len(), 16);
    }

    #[test]
    fn zero_budget_refuses_even_the_smallest_fixture() {
        let err = load_texture_with_budget(&fixture("rgba2x2.png"), 0)
            .expect_err("a zero budget must refuse everything");
        match err {
            AssetError::OverBudget { limit, actual, .. } => {
                assert_eq!(limit, 0);
                assert_eq!(actual, 16, "the 2x2 RGBA fixture needs exactly 16 bytes");
            }
            other => panic!("zero budget must be OverBudget, got: {other:?}"),
        }
    }

    #[test]
    fn lying_header_claiming_gigapixels_is_over_budget() {
        // A 1x1 file whose IHDR claims u32::MAX dimensions: the explicit
        // pre-check must refuse it without attempting the allocation.
        // Built by encoding a valid 1x1 PNG, then patching IHDR width and
        // height in place (bytes 16..24) and fixing the chunk CRC.
        let mut bytes = encode_png(
            1,
            1,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[1, 2, 3, 4],
        );
        bytes[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        bytes[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
        let path = write_temp("lying-header.png", &bytes);
        let err = load_texture(&path).expect_err("gigapixel claim must fail");
        assert!(
            matches!(err, AssetError::OverBudget { .. }),
            "lying header must be OverBudget, got: {err:?}"
        );
    }

    /// Minimal CRC32 (IEEE) for patching a PNG chunk CRC in tests.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    #[test]
    fn loaded_texture_flows_through_asset_store_with_stable_identity() {
        use crate::{AssetId, AssetStore, LOADER_VERSION};

        let path = fixture("rgba2x2.png");
        let bytes = std::fs::read(&path).expect("RGBA fixture must exist");
        assert_eq!(
            AssetId::for_file(&path).expect("readable fixture must hash"),
            AssetId::new(&bytes),
            "file identity must equal the in-memory hash of the same bytes"
        );
        assert_eq!(
            AssetId::new(&bytes),
            AssetId::with_version(&bytes, LOADER_VERSION),
            "`new` must hash under LOADER_VERSION"
        );

        let texture = load_texture(&path).expect("RGBA fixture must load");
        let mut store = AssetStore::new();
        let handle = store.insert(texture);
        let stored = store.get(handle).expect("live handle must resolve");
        assert_eq!(stored.pixel(0, 0), Some([255, 0, 0, 255]));
    }

    #[test]
    fn git_lfs_pointer_bytes_are_invalid_format_not_a_panic() {
        // Given: what a fixture resolves to when Git LFS objects were not
        // fetched (CI without `lfs: true`): pointer text, not a PNG.
        let pointer =
            b"version https://git-lfs.github.com/spec/v1\noid sha256:0000000000000000000000000000000000000000000000000000000000000000\nsize 12345\n";
        let path = write_temp("lfs-pointer.png", pointer);

        // When: loaded. Then: Err (InvalidFormat), never a panic.
        let err = load_texture(&path).expect_err("LFS pointer text must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "LFS pointer bytes must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn missing_parent_directory_is_io_not_a_panic() {
        // Given: a path under a directory that does not exist at all.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/does-not-exist-dir/rgba2x2.png");

        // When: loaded. Then: Io (the read fails before any decode).
        let err = load_texture(&path).expect_err("missing directory must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "missing parent directory must be Io, got: {err:?}"
        );
    }

    #[test]
    fn unreadable_file_reports_io_even_under_zero_budget() {
        // Given: a path that cannot be read at all, with a budget of zero.
        // Then: Io, not OverBudget — the read runs before every budget
        // guard, so a missing file can never report a budget failure.
        let err = load_texture_with_budget(Path::new("definitely-not-a-texture.png"), 0)
            .expect_err("missing file must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "read-before-budget: missing file must be Io even with budget 0, got: {err:?}"
        );
    }

    #[test]
    fn png_truncated_mid_idat_is_invalid_format() {
        // Given: the RGBA fixture cut off three-quarters through — past the
        // header and into the image data.
        let full = std::fs::read(fixture("rgba2x2.png")).expect("RGBA fixture must exist");
        let path = write_temp("mid-idat-truncated.png", &full[..full.len() * 3 / 4]);

        // When: loaded. Then: InvalidFormat, never a panic.
        let err = load_texture(&path).expect_err("mid-IDAT truncation must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "truncated IDAT must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn png_with_flipped_idat_byte_is_invalid_format() {
        // Given: the RGBA fixture with one byte flipped inside the IDAT
        // payload (past the 8-byte signature + 25-byte IHDR chunk, into
        // the IDAT length/type region) — the header parses, but the
        // chunk CRC no longer matches.
        let mut bytes = std::fs::read(fixture("rgba2x2.png")).expect("RGBA fixture must exist");
        assert!(
            bytes.len() > 45,
            "fixture must be long enough to hold an IDAT chunk"
        );
        bytes[45] ^= 0xFF;
        let path = write_temp("flipped-idat.png", &bytes);

        // When: loaded. Then: InvalidFormat, never a panic.
        let err = load_texture(&path).expect_err("corrupt IDAT must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "CRC-broken IDAT must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn png_trailing_iend_crc_flip_stays_ok_with_correct_pixels() {
        // Given: the RGBA fixture with its LAST byte flipped — inside the
        // trailing IEND CRC, which guards no pixel data (Phase 09 W1).
        let mut bytes = std::fs::read(fixture("rgba2x2.png")).expect("RGBA fixture must exist");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let path = write_temp("trailing-crc-flip.png", &bytes);

        // When: loaded. Then: Ok with byte-correct pixels — trailing-CRC
        // leniency is classified legitimate (the `png` crate does not gate
        // pixel output on it), NOT a failure. This pins the W1
        // classification so a future `png` bump that hardens CRC handling
        // shows up here as an intended-contract change, not a mystery.
        let texture = load_texture(&path).expect("trailing CRC flip must stay Ok");
        let reference = load_texture(&fixture("rgba2x2.png")).expect("fixture must load");
        assert_eq!(
            texture.rgba8(),
            reference.rgba8(),
            "trailing-CRC leniency must never corrupt pixels"
        );
    }

    /// One hostile shape applied to the 75-byte RGBA fixture: the chaos
    /// feed (mid-IDAT truncation, IDAT mutation) plus IHDR-lie patches
    /// with and without a repaired CRC, plus byte flips and truncation.
    #[derive(Debug, Clone)]
    enum PngFuzzMutation {
        /// XOR one byte with a nonzero mask.
        Flip { offset: usize, xor: u8 },
        /// Cut the file to `len` bytes (0 = empty file).
        Truncate { len: usize },
        /// Overwrite the IHDR width/height (big-endian bytes 16..24),
        /// optionally repairing the IHDR CRC so the lie reaches the
        /// budget pre-check instead of dying at the CRC gate.
        PatchIhdrDims {
            width: u32,
            height: u32,
            fix_crc: bool,
        },
    }

    /// Whether this mutation stays inside the trailing 12-byte IEND
    /// chunk (length + type + CRC): the only region where an Ok with
    /// unchanged pixels is legitimate leniency rather than a wrong-Ok.
    fn touches_only_trailing_iend(mutation: &PngFuzzMutation, base_len: usize) -> bool {
        match *mutation {
            PngFuzzMutation::Flip { offset, .. } => {
                offset % base_len >= base_len.saturating_sub(12)
            }
            PngFuzzMutation::Truncate { .. } | PngFuzzMutation::PatchIhdrDims { .. } => false,
        }
    }

    /// Strategy over [`PngFuzzMutation`]; offsets clamp onto the 75-byte
    /// fixture at apply time so input construction itself stays total.
    fn png_mutation_strategy(
        max_len: usize,
    ) -> impl proptest::strategy::Strategy<Value = PngFuzzMutation> {
        use proptest::prelude::*;
        prop_oneof![
            (0..max_len, 1u8..=255).prop_map(|(offset, xor)| PngFuzzMutation::Flip { offset, xor }),
            (0..=max_len).prop_map(|len| PngFuzzMutation::Truncate { len }),
            (
                prop_oneof![
                    Just(0u32),
                    Just(1u32),
                    Just(2u32),
                    Just(0x1000u32),
                    Just(u32::MAX),
                    any::<u32>(),
                ],
                prop_oneof![
                    Just(0u32),
                    Just(1u32),
                    Just(2u32),
                    Just(0x1000u32),
                    Just(u32::MAX),
                    any::<u32>(),
                ],
                proptest::bool::ANY,
            )
                .prop_map(|(width, height, fix_crc)| PngFuzzMutation::PatchIhdrDims {
                    width,
                    height,
                    fix_crc,
                }),
        ]
    }

    /// Applies a mutation to `base`; total over all inputs like its GLB
    /// twin in `mesh.rs` tests. The IHDR patch path reuses the
    /// IEEE-CRC32 helper above so `fix_crc` truly repairs the chunk.
    fn apply_png_mutation(base: &[u8], mutation: &PngFuzzMutation) -> Vec<u8> {
        assert!(
            base.len() > 33,
            "fuzz base must hold signature + IHDR + IEND"
        );
        match *mutation {
            PngFuzzMutation::Flip { offset, xor } => {
                let mut out = base.to_vec();
                let index = offset % base.len();
                out[index] ^= xor;
                out
            }
            PngFuzzMutation::Truncate { len } => base[..len.min(base.len())].to_vec(),
            PngFuzzMutation::PatchIhdrDims {
                width,
                height,
                fix_crc,
            } => {
                let mut out = base.to_vec();
                out[16..20].copy_from_slice(&width.to_be_bytes());
                out[20..24].copy_from_slice(&height.to_be_bytes());
                if fix_crc {
                    let crc = crc32(&out[12..29]);
                    out[29..33].copy_from_slice(&crc.to_be_bytes());
                }
                out
            }
        }
    }

    /// The texture-side oracle's Ok arm: dimensions are nonzero, the
    /// pixel buffer is exactly `width * height * 4`, and the `pixel`
    /// accessor agrees with the raw buffer on every corner while
    /// refusing out-of-bounds coordinates.
    fn assert_texture_invariants(texture: &Texture) {
        assert!(texture.width() > 0, "decoded width must be nonzero");
        assert!(texture.height() > 0, "decoded height must be nonzero");
        let expected = u64::from(texture.width())
            .checked_mul(u64::from(texture.height()))
            .and_then(|pixels| pixels.checked_mul(4))
            .expect("u32 dimensions times 4 always fits u64");
        assert_eq!(
            texture.rgba8().len() as u64,
            expected,
            "rgba8 must hold exactly width*height*4 bytes"
        );
        let raw = texture.rgba8();
        for (x, y) in [
            (0, 0),
            (texture.width() - 1, 0),
            (0, texture.height() - 1),
            (texture.width() - 1, texture.height() - 1),
        ] {
            let offset = (u64::from(y) * u64::from(texture.width()) + u64::from(x)) * 4;
            let offset = usize::try_from(offset).expect("decoded buffer fits the address space");
            let want = [
                raw[offset],
                raw[offset + 1],
                raw[offset + 2],
                raw[offset + 3],
            ];
            assert_eq!(
                texture.pixel(x, y),
                Some(want),
                "pixel() must agree with the raw buffer at ({x}, {y})"
            );
        }
        assert_eq!(
            texture.pixel(texture.width(), 0),
            None,
            "past the right edge must be None"
        );
        assert_eq!(
            texture.pixel(0, texture.height()),
            None,
            "past the bottom edge must be None"
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config { cases: 256, ..Default::default() })]
        /// Oracle-first bounded PNG fuzz: mutated fixture bytes fed
        /// through [`load_texture_with_budget`] must NEVER panic or
        /// wrong-Ok. Any Err variant is acceptable; an Ok must satisfy
        /// [`assert_texture_invariants`], and an Ok whose mutation stays
        /// inside the trailing IEND chunk must additionally carry the
        /// fixture's exact pixels (W1 leniency classification: tolerate,
        /// never corrupt). The budget is 4 KiB — 256× the fixture's
        /// 16-byte output — so a missed guard can allocate at most
        /// kilobytes before the decoder-internal limit fires. Raw-noise
        /// fuzzing and `cargo-fuzz` were judged unnecessary for the same
        /// reasons as the GLB twin (tiny inputs, validation logic under
        /// test, deterministic proptest regressions).
        #[test]
        fn png_byte_mutations_never_panic_and_ok_means_valid(
            mutation in png_mutation_strategy(75)
        ) {
            // Given: the 75-byte RGBA fixture's exact bytes, hostilely mutated.
            let base = std::fs::read(fixture("rgba2x2.png")).expect("RGBA fixture must exist");
            let bytes = apply_png_mutation(&base, &mutation);
            let path = write_temp("png-fuzz.png", &bytes);

            // When: loaded under a 4 KiB ceiling (file I/O first per the
            // documented read-before-budget R1 order — this fuzzes the
            // decode path, not the disk path).
            let result = load_texture_with_budget(&path, 4096);

            // Then: Err of any typed variant, or a structurally valid Ok;
            // trailing-IEND-only Ok must additionally be pixel-identical.
            if let Ok(texture) = result {
                assert_texture_invariants(&texture);
                if touches_only_trailing_iend(&mutation, base.len()) {
                    let reference =
                        load_texture(&fixture("rgba2x2.png")).expect("fixture must load");
                    proptest::prop_assert_eq!(
                        texture.rgba8(),
                        reference.rgba8(),
                        "trailing-IEND leniency must never corrupt pixels"
                    );
                }
            }
        }
    }
}
