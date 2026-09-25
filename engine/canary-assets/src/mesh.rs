// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! GLB mesh loading: file bytes become validated [`Mesh`] values.
//!
//! The one format this release reads is GLB, the binary container for glTF
//! 2.0 (ADR 0018): a single self-contained file — JSON chunk plus BIN
//! chunk — so fixtures are hash-stable with no sidecar-buffer path
//! resolution to drift. Parsing is delegated to the `gltf` crate, but its
//! types never escape this module: the public surface is [`Mesh`] (plain
//! `Vec`s of arrays) and [`load_mesh`] (a path in, validated values out).
//!
//! Fixture provenance: `tests/fixtures/quad.glb` (one triangle primitive)
//! and `tests/fixtures/box.glb` (two triangle primitives sharing one
//! buffer) are checked in and hash-stable; see
//! `tests/fixtures/README.md` for how they were generated. Tests assert
//! loader *output values* (positions, indices, counts) against checked-in
//! expectations — never hashes of toolchain behavior.

use std::path::Path;

use crate::io::{read_file_with_budget, resolve_in_root, MAX_ASSET_FILE_BYTES};
use crate::AssetError;

/// Maximum vertices accepted from a single GLB primitive.
///
/// Untrusted-file discipline: a corrupt header could otherwise claim
/// billions of vertices and make the collect below allocate first and
/// apologize after. One million vertices is orders of magnitude past this
/// release's kilobyte fixtures while still fitting comfortably in memory
/// (~12 MiB of positions); the value is **provisional** pending measured
/// calibration against real content, exactly like the texture budgets in
/// the `texture` module.
pub const MAX_MESH_VERTICES_PER_PRIMITIVE: usize = 1 << 20;

/// Maximum indices accepted from a single GLB primitive.
///
/// Same rationale as [`MAX_MESH_VERTICES_PER_PRIMITIVE`]: bounds the
/// index collect before it happens. Sixteen million `u32` indices (~64
/// MiB) dwarfs anything this release loads; provisional pending
/// calibration.
pub const MAX_MESH_INDICES_PER_PRIMITIVE: usize = 1 << 24;

/// Triangle mesh loaded from a GLB primitive: indexed geometry with optional
/// normals and UVs.
///
/// Positions and indices are always present; normals and UVs are `Some`
/// exactly when the source primitive carried them. They are loaded and
/// stored even though the renderer ignores them this release (the roadmap
/// records them as "loaded, stored, unused"): dropping them at load time
/// would silently discard author intent, and a later phase should not have
/// to re-derive what the file already said.
///
/// Why triangle-only: the Phase 3 bridge expands indices into the
/// renderer's triangle-soup bake, so points/lines/strips have no consumer
/// and would need a parallel untested path. [`load_mesh`] rejects them as
/// [`AssetError::UnsupportedFeature`] — the file is fine, the loader is
/// deliberately narrow — rather than misrendering them as triangles.
///
/// Why indices are preserved, not pre-expanded: expanding index triples
/// into soup at load time would bake in one consumer's layout and hide the
/// vertex-duplication cost of doing so. The asset stays honest
/// (shared vertices shared); index-to-soup expansion happens at the
/// renderer bridge, which is where the soup layout is actually known.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Option<Vec<[f32; 2]>>,
    indices: Vec<u32>,
}

impl Mesh {
    /// Vertex positions in model space, one per vertex.
    ///
    /// Non-empty by construction: [`load_mesh`] rejects primitives with
    /// zero vertices, so downstream code may rely on this being populated
    /// without re-checking.
    pub fn positions(&self) -> &[[f32; 3]] {
        &self.positions
    }

    /// Vertex normals, present exactly when the source primitive carried a
    /// `NORMAL` attribute.
    ///
    /// When present, its length always equals [`Mesh::positions`]' length:
    /// [`load_mesh`] rejects inconsistent attribute counts rather than
    /// zipping uneven arrays and silently dropping the tail.
    pub fn normals(&self) -> Option<&[[f32; 3]]> {
        self.normals.as_deref()
    }

    /// Texture coordinates (glTF `TEXCOORD_0`), present exactly when the
    /// source primitive carried them.
    ///
    /// Only set 0 is read: multi-UV workflows belong to the deferred
    /// materials system, not to this release's single-texture slice. Same
    /// length guarantee as [`Mesh::normals`].
    pub fn uvs(&self) -> Option<&[[f32; 2]]> {
        self.uvs.as_deref()
    }

    /// Triangle indices into [`Mesh::positions`], always a multiple of 3.
    ///
    /// Every index is bounds-checked at load time, so indexing with these
    /// cannot panic for any successfully loaded mesh. Non-empty by
    /// construction, like positions.
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// The number of vertices (length of [`Mesh::positions`]).
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// The number of triangles (length of [`Mesh::indices`] divided by 3).
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Loads every triangle primitive from the GLB file at `path` as its own
/// [`Mesh`] value, in mesh-then-primitive document order.
///
/// One primitive becomes one mesh — never merged, never split — because
/// primitives are the file's own unit of "one draw with one material
/// slot", and merging them would destroy the boundary a later materials
/// phase needs. A file with no meshes, or meshes with no primitives,
/// yields [`AssetError::InvalidFormat`]: an empty result would let a
/// missing-geometry bug travel silently to the renderer.
///
/// Identity (which content is this?) is intentionally *not* part of the
/// return: callers hash the same file bytes with
/// [`crate::AssetId::for_file`], which mixes in
/// [`crate::LOADER_VERSION`] so a loader fix deterministically changes
/// IDs. Keeping identity beside loading (not inside it) means the loader
/// never has to agree with the hasher about what "the bytes" were.
///
/// Failure taxonomy (all variants carry `path`):
/// - Missing/unreadable file → [`AssetError::Io`].
/// - File past [`MAX_ASSET_FILE_BYTES`] → [`AssetError::OverBudget`],
///   refused before its contents are allocated (untrusted-file
///   discipline: the capped read runs before any decode).
/// - Unparseable bytes, missing `POSITION`, empty geometry, inconsistent
///   attribute counts, out-of-bounds indices, index count not a multiple
///   of 3 → [`AssetError::InvalidFormat`] (the *file* is broken).
/// - Non-triangle primitive modes, non-indexed primitives, buffers outside
///   the GLB's own BIN chunk → [`AssetError::UnsupportedFeature`] (the
///   file is fine; this minimal loader declines it).
/// - Claimed vertex/index counts past the `MAX_MESH_*` budgets →
///   [`AssetError::OverBudget`] (untrusted-file discipline: refused before
///   any large allocation).
///
/// The function is synchronous and total over its inputs: malformed files
/// produce `Err`, never a panic. There is no `async` API because the
/// scheduler has no threading story for background loading yet (ADR 0018).
pub fn load_mesh(path: &Path) -> Result<Vec<Mesh>, AssetError> {
    let bytes = read_file_with_budget(path, MAX_ASSET_FILE_BYTES)?;
    load_mesh_from_bytes(path, &bytes)
}

/// Loads the GLB file at root-relative `candidate` as [`Mesh`] values,
/// refusing anything that escapes `root`.
///
/// The confined twin of [`load_mesh`] for the future asset manager,
/// which joins untrusted listing paths onto a content root: the
/// candidate is resolved with [`crate::resolve_in_root`] first (a
/// `..` escape, an absolute path, or a symlink-out fails as
/// [`AssetError::OutsideRoot` before a byte is read), then decoded
/// exactly like [`load_mesh`] — same budgets, same taxonomy, same
/// values. A valid in-root file loads byte-identical to the direct
/// entry point; confinement only narrows *which* paths are accepted.
pub fn load_mesh_within_root(root: &Path, candidate: &Path) -> Result<Vec<Mesh>, AssetError> {
    let resolved = resolve_in_root(root, candidate)?;
    let bytes = read_file_with_budget(&resolved, MAX_ASSET_FILE_BYTES)?;
    load_mesh_from_bytes(&resolved, &bytes)
}

/// Parses GLB `bytes` (attributed to `path` in errors) into [`Mesh`] values.
///
/// Split from [`load_mesh`] so tests can feed in-memory mutations of fixture
/// bytes (truncated, mode-swapped, index-corrupted) without touching disk,
/// while every error still names the originating file. Private: callers
/// outside this crate load from paths, keeping "which file" unambiguous.
fn load_mesh_from_bytes(path: &Path, bytes: &[u8]) -> Result<Vec<Mesh>, AssetError> {
    // Totality boundary for the whole traversal, not just the container
    // parse: the `gltf` crate indexes some of its tables by file-supplied
    // indices without bounds-checking every path first, so hostile bytes
    // can panic inside the dependency both at parse time (see
    // `load_mesh_from_bytes_inner`) and later, while reader utilities
    // walk accessors. Found the hard way: a macOS CI run panicked in
    // `gltf`'s own accessor utilities on an input Linux never tripped
    // on. Trapped panics report InvalidFormat — the file is broken.
    // The closure only borrows the input bytes and builds owned values,
    // so unwinding through it leaves no shared state behind. Honest
    // limit, unchanged: this catches unwinding panics, not allocation
    // failure or abort-class faults.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        load_mesh_from_bytes_inner(path, bytes)
    })) {
        Ok(result) => result,
        Err(_) => Err(AssetError::invalid_format(
            path,
            "glTF traversal trapped on hostile input",
        )),
    }
}

fn load_mesh_from_bytes_inner(path: &Path, bytes: &[u8]) -> Result<Vec<Mesh>, AssetError> {
    let invalid = |reason: &str| AssetError::invalid_format(path, reason);
    let unsupported = |feature: &str| AssetError::unsupported_feature(path, feature);

    // The `gltf` crate's own container validation indexes its tables by
    // file-supplied indices without bounds-checking every path first: a
    // hostile JSON chunk with dangling references (e.g. a primitive
    // pointing at a nonexistent accessor) panics inside the dependency
    // Container validation runs here; panics deeper inside the
    // dependency (dangling table references, hostile accessor walks)
    // are trapped by the `load_mesh_from_bytes` boundary above, so this
    // site only maps the error type the crate actually returns.
    let gltf = gltf::Gltf::from_slice(bytes)
        .map_err(|source| invalid(&format!("not a parseable glTF file: {source}")))?;

    // A self-contained GLB carries exactly one buffer — its own BIN chunk,
    // exposed as `blob`. Anything else (a `.gltf` JSON sidecar, a data-URI
    // buffer, a second buffer entry) is precisely the external-resolution
    // machinery this release declined when it skipped the `gltf` crate's
    // `import` feature (see the dependency rationale in `Cargo.toml`), so
    // it fails here as UnsupportedFeature rather than resolving halfway.
    // Checked before buffer resolution on purpose: a file with no
    // geometry at all is InvalidFormat regardless of what its buffer
    // table looks like, and reporting it as such keeps the "which
    // failure class" answer independent of unrelated file sections.
    if gltf.document.meshes().next().is_none() {
        return Err(invalid("file contains no mesh primitives"));
    }

    let mut buffers = gltf.document.buffers();
    let blob: &[u8] = match (buffers.next(), buffers.next(), gltf.blob.as_deref()) {
        (Some(buffer), None, Some(data)) => {
            if !matches!(buffer.source(), gltf::buffer::Source::Bin) {
                return Err(unsupported("external mesh buffer referenced by URI"));
            }
            data
        }
        _ => {
            return Err(unsupported(
                "mesh buffers outside the GLB's embedded BIN chunk",
            ));
        }
    };

    let mut meshes = Vec::new();
    for mesh in gltf.document.meshes() {
        for primitive in mesh.primitives() {
            meshes.push(read_primitive(path, &primitive, blob)?);
        }
    }
    if meshes.is_empty() {
        return Err(invalid("file contains no mesh primitives"));
    }
    Ok(meshes)
}

/// Reads one GLB primitive into a validated [`Mesh`].
///
/// The `blob` parameter is the file's whole BIN chunk; buffer views slice
/// into it through the `gltf` reader closure, which borrows rather than
/// copies. Every rejection below names the failure class documented on
/// [`load_mesh`].
fn read_primitive(
    path: &Path,
    primitive: &gltf::Primitive<'_>,
    blob: &[u8],
) -> Result<Mesh, AssetError> {
    let invalid = |reason: String| AssetError::invalid_format(path, reason);
    let unsupported = |feature: String| AssetError::unsupported_feature(path, feature);

    // Triangle-mode only (see `Mesh` docs for why): note glTF defaults an
    // omitted `mode` to triangles, and `primitive.mode()` already applies
    // that default, so an absent mode loads rather than rejects.
    if primitive.mode() != gltf::mesh::Mode::Triangles {
        return Err(unsupported(format!(
            "primitive mode {:?}: only triangle primitives are supported",
            primitive.mode()
        )));
    }

    let position_accessor = primitive
        .get(&gltf::Semantic::Positions)
        .ok_or_else(|| invalid("primitive is missing the POSITION attribute".to_string()))?;
    let position_count = position_accessor.count();
    if position_count == 0 {
        return Err(invalid("primitive has no vertices".to_string()));
    }
    check_count_budget(path, position_count, MAX_MESH_VERTICES_PER_PRIMITIVE)?;

    // Indices are required, not synthesized (see `Mesh` docs for why):
    // a non-indexed primitive would force this loader to invent the very
    // expansion policy it exists to defer to the bridge.
    let index_accessor = primitive
        .indices()
        .ok_or_else(|| unsupported("non-indexed primitive: indices are required".to_string()))?;
    let index_count = index_accessor.count();
    if index_count == 0 {
        return Err(invalid("primitive has no indices".to_string()));
    }
    check_count_budget(path, index_count, MAX_MESH_INDICES_PER_PRIMITIVE)?;
    if index_count % 3 != 0 {
        return Err(invalid(format!(
            "triangle primitive has {index_count} indices, not a multiple of 3"
        )));
    }

    let normal_accessor = primitive.get(&gltf::Semantic::Normals);
    if let Some(accessor) = &normal_accessor {
        check_attribute_count(accessor, position_count, "NORMAL").map_err(invalid)?;
    }
    let uv_accessor = primitive.get(&gltf::Semantic::TexCoords(0));
    if let Some(accessor) = &uv_accessor {
        check_attribute_count(accessor, position_count, "TEXCOORD_0").map_err(invalid)?;
    }

    let reader = primitive.reader(|buffer| {
        if buffer.index() == 0 {
            Some(blob)
        } else {
            None
        }
    });

    // Each collect is cross-checked against the accessor's declared count
    // immediately below: the `gltf` iterators stop at the end of the
    // available buffer data, so a short/truncated BIN chunk surfaces as a
    // short collect, not a panic — and a short collect is InvalidFormat.
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| invalid("POSITION attribute is unreadable".to_string()))?
        .collect();
    if positions.len() != position_count {
        return Err(invalid(format!(
            "POSITION holds {} vertices but declares {position_count}",
            positions.len()
        )));
    }

    let normals: Option<Vec<[f32; 3]>> = match (normal_accessor, reader.read_normals()) {
        (None, _) => None,
        (Some(_), Some(iter)) => {
            let values: Vec<[f32; 3]> = iter.collect();
            if values.len() != position_count {
                return Err(invalid(format!(
                    "NORMAL holds {} vertices but declares {position_count}",
                    values.len()
                )));
            }
            Some(values)
        }
        (Some(_), None) => {
            return Err(invalid("NORMAL attribute is unreadable".to_string()));
        }
    };

    let uvs: Option<Vec<[f32; 2]>> = match (uv_accessor, reader.read_tex_coords(0)) {
        (None, _) => None,
        (Some(_), Some(iter)) => {
            let values: Vec<[f32; 2]> = read_tex_coords_into_f32(iter);
            if values.len() != position_count {
                return Err(invalid(format!(
                    "TEXCOORD_0 holds {} vertices but declares {position_count}",
                    values.len()
                )));
            }
            Some(values)
        }
        (Some(_), None) => {
            return Err(invalid("TEXCOORD_0 attribute is unreadable".to_string()));
        }
    };

    let indices: Vec<u32> = reader
        .read_indices()
        .ok_or_else(|| invalid("indices are unreadable".to_string()))?
        .into_u32()
        .collect();
    if indices.len() != index_count {
        return Err(invalid(format!(
            "indices hold {} entries but declare {index_count}",
            indices.len()
        )));
    }
    let vertex_count_u32 = u32::try_from(position_count).unwrap_or(u32::MAX);
    for index in &indices {
        if *index >= vertex_count_u32 {
            return Err(invalid(format!(
                "index {index} is out of bounds for {position_count} vertices"
            )));
        }
    }

    Ok(Mesh {
        positions,
        normals,
        uvs,
        indices,
    })
}

/// Rejects a per-vertex attribute whose declared count differs from the
/// primitive's position count.
///
/// Returns the reason clause for [`AssetError::InvalidFormat`] on
/// mismatch, `Ok` on agreement. A mismatch means the file's own arrays
/// disagree with each other — no zipping convention (truncate? pad with
/// zeros?) could rescue it without inventing vertex data, so it is an
/// error, not a warning.
fn check_attribute_count(
    accessor: &gltf::Accessor<'_>,
    position_count: usize,
    name: &str,
) -> Result<(), String> {
    let count = accessor.count();
    if count != position_count {
        return Err(format!(
            "{name} holds {count} vertices but POSITION holds {position_count}"
        ));
    }
    Ok(())
}

/// Refuses a primitive whose declared element `count` exceeds the `limit`
/// budget.
///
/// Shared by the vertex-count and index-count gates: both report
/// [`AssetError::OverBudget`] with the budget limit and the claimed count.
/// One function so the two gates cannot drift into different taxonomies
/// for the same condition.
fn check_count_budget(path: &Path, count: usize, limit: usize) -> Result<(), AssetError> {
    if count > limit {
        return Err(AssetError::over_budget(path, limit as u64, count as u64));
    }
    Ok(())
}

/// Converts glTF texture coordinates into the stored `[f32; 2]` values.
///
/// glTF genericizes texcoords over `u8`, `u16`, and `f32` component types;
/// the reader surfaces one iterator per representation, while [`Mesh`]
/// stores exactly one. Normalization keeps the asset honest the same way
/// the texture loader's RGBA8 normalization does: one in-memory layout,
/// converted once at the boundary, documented here rather than silently
/// assumed by every consumer. Normalized-integer inputs divide by the
/// type maximum, matching the glTF specification's own mapping of stored
/// integers onto `[0, 1]` texture space.
fn read_tex_coords_into_f32(coords: gltf::mesh::util::ReadTexCoords<'_>) -> Vec<[f32; 2]> {
    use gltf::mesh::util::ReadTexCoords;
    match coords {
        ReadTexCoords::U8(iter) => iter
            .map(|array| {
                [
                    f32::from(array[0]) / f32::from(u8::MAX),
                    f32::from(array[1]) / f32::from(u8::MAX),
                ]
            })
            .collect(),
        ReadTexCoords::U16(iter) => iter
            .map(|array| {
                [
                    f32::from(array[0]) / f32::from(u16::MAX),
                    f32::from(array[1]) / f32::from(u16::MAX),
                ]
            })
            .collect(),
        ReadTexCoords::F32(iter) => iter.collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Checked-in fixture path (tests run with the crate dir as CWD, but
    /// `CARGO_MANIFEST_DIR` keeps this robust to invocation differences).
    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// Scratch file for runtime-built (negative-control) inputs, unique per
    /// test name so parallel tests never share a path.
    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("canary-assets-mesh-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir must be creatable");
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("scratch fixture must be writable");
        path
    }

    /// Exact float comparison via bit patterns: loader outputs round-trip
    /// the file's little-endian bytes with no arithmetic in between, so
    /// bitwise equality is the honest assertion (and it sidesteps
    /// epsilon debates entirely).
    fn assert_f32_slice_eq(actual: &[f32], expected: &[f32]) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "length mismatch: {} vs {}",
            actual.len(),
            expected.len()
        );
        for (index, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                a.to_bits(),
                e.to_bits(),
                "float mismatch at flat index {index}: {a} vs {e}"
            );
        }
    }

    fn flatten3(values: &[[f32; 3]]) -> Vec<f32> {
        values.iter().flat_map(|v| *v).collect()
    }

    fn flatten2(values: &[[f32; 2]]) -> Vec<f32> {
        values.iter().flat_map(|v| *v).collect()
    }

    /// Frames `json` + `bin` as a GLB file, mirroring the checked-in
    /// generator's layout (12-byte header, space-padded JSON chunk,
    /// zero-padded BIN chunk).
    fn glb_bytes(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json_padded = json.as_bytes().to_vec();
        while json_padded.len() % 4 != 0 {
            json_padded.push(b' ');
        }
        let mut bin_padded = bin.to_vec();
        while bin_padded.len() % 4 != 0 {
            bin_padded.push(0);
        }
        let total = 12 + 8 + json_padded.len() + 8 + bin_padded.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&0x46546C67u32.to_le_bytes());
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_padded.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_padded);
        out.extend_from_slice(&(bin_padded.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\x00");
        out.extend_from_slice(&bin_padded);
        out
    }

    /// Canonical quad document; `{MODE}` is substituted with the primitive
    /// mode under test (4 = triangles).
    const QUAD_JSON_TEMPLATE: &str = r#"{"accessors":[{"bufferView":0,"componentType":5126,"count":4,"max":[0.5,0.5,0.0],"min":[-0.5,-0.5,0.0],"type":"VEC3"},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"}],"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":48,"byteOffset":0},{"buffer":0,"byteLength":32,"byteOffset":48},{"buffer":0,"byteLength":12,"byteOffset":80}],"buffers":[{"byteLength":92}],"meshes":[{"name":"quad","primitives":[{"attributes":{"POSITION":0,"TEXCOORD_0":1},"indices":2,"mode":{MODE}}]}],"nodes":[{"mesh":0}],"scene":0,"scenes":[{"nodes":[0]}]}"#;

    /// Canonical quad buffer: 4 positions, 4 UVs, 6 u16 indices.
    fn quad_bin_with_indices(indices: &[u16; 6]) -> Vec<u8> {
        let positions: [f32; 12] = [
            -0.5, -0.5, 0.0, 0.5, -0.5, 0.0, 0.5, 0.5, 0.0, -0.5, 0.5, 0.0,
        ];
        let uvs: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        let mut bin = Vec::with_capacity(92);
        for v in positions {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        for v in uvs {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        bin
    }

    #[test]
    fn quad_fixture_loads_with_known_positions_and_indices() {
        let meshes = load_mesh(&fixture("quad.glb")).expect("quad fixture must load");
        assert_eq!(meshes.len(), 1, "quad holds a single primitive");
        let mesh = &meshes[0];
        assert_f32_slice_eq(
            &flatten3(mesh.positions()),
            &[
                -0.5, -0.5, 0.0, 0.5, -0.5, 0.0, 0.5, 0.5, 0.0, -0.5, 0.5, 0.0,
            ],
        );
        assert_eq!(
            mesh.indices(),
            &[0, 1, 2, 0, 2, 3],
            "quad triangulation must match the fixture README"
        );
        assert_eq!(mesh.vertex_count(), 4);
        assert_eq!(mesh.triangle_count(), 2);
    }

    #[test]
    fn quad_fixture_carries_uvs_but_no_normals() {
        let meshes = load_mesh(&fixture("quad.glb")).expect("quad fixture must load");
        let uvs = meshes[0].uvs().expect("quad carries TEXCOORD_0");
        assert_f32_slice_eq(&flatten2(uvs), &[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]);
        assert_eq!(
            meshes[0].normals(),
            None,
            "quad has no NORMAL attribute, so normals must be None"
        );
    }

    #[test]
    fn box_fixture_yields_one_mesh_per_primitive() {
        let meshes = load_mesh(&fixture("box.glb")).expect("box fixture must load");
        assert_eq!(meshes.len(), 2, "two primitives must become two meshes");
        assert_eq!(meshes[0].vertex_count(), 8);
        assert_eq!(meshes[0].indices().len(), 18);
        assert_eq!(meshes[0].triangle_count(), 6);
        assert_eq!(meshes[1].vertex_count(), 8);
        assert_eq!(meshes[1].indices().len(), 18);
        assert_eq!(
            meshes[0].indices(),
            &[4, 5, 6, 4, 6, 7, 1, 0, 3, 1, 3, 2, 5, 1, 2, 5, 2, 6],
            "first primitive covers the +z/-z/+x faces per the README"
        );
    }

    #[test]
    fn box_second_primitive_carries_normals_and_uvs() {
        let meshes = load_mesh(&fixture("box.glb")).expect("box fixture must load");
        assert_eq!(
            meshes[0].normals(),
            None,
            "first primitive has no NORMAL attribute"
        );
        assert_eq!(meshes[0].uvs(), None, "first primitive has no UVs");
        let normals = meshes[1]
            .normals()
            .expect("second primitive carries NORMAL");
        assert_f32_slice_eq(
            &flatten3(normals),
            &[
                1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0,
                0.0, -1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
            ],
        );
        let uvs = meshes[1]
            .uvs()
            .expect("second primitive carries TEXCOORD_0");
        assert_eq!(uvs.len(), 8);
        assert_f32_slice_eq(&uvs[0], &[0.0, 0.0]);
    }

    #[test]
    fn missing_file_is_io_not_a_panic() {
        let err =
            load_mesh(Path::new("definitely-not-a-mesh.glb")).expect_err("missing file must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "missing file must be Io, got: {err:?}"
        );
    }

    #[test]
    fn within_root_loads_valid_files_byte_identical_and_refuses_escapes() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let root = dir.path().join("assets");
        std::fs::create_dir(&root).expect("root must be creatable");
        let bytes = std::fs::read(fixture("quad.glb")).expect("quad fixture must exist");
        std::fs::write(root.join("quad.glb"), &bytes).expect("rooted copy must be writable");
        // The `..` escape needs a real file outside the root: a
        // dangling traversal fails as Io (missing file), which would
        // prove nothing about confinement.
        std::fs::write(dir.path().join("quad.glb"), &bytes).expect("outside copy must be writable");

        let direct = load_mesh(&root.join("quad.glb")).expect("rooted copy must load");
        let confined = load_mesh_within_root(&root, Path::new("quad.glb"))
            .expect("in-root candidate must load");
        assert_eq!(
            confined, direct,
            "confinement must change which paths are accepted, never the values"
        );

        for escape in [
            Path::new("../quad.glb").to_path_buf(),
            dir.path().join("quad.glb"),
        ] {
            let err = load_mesh_within_root(&root, &escape)
                .expect_err("escape must fail before decoding");
            assert!(
                matches!(err, AssetError::OutsideRoot { .. }),
                "escape must be OutsideRoot, got: {err:?}"
            );
        }
    }

    #[test]
    fn oversized_file_is_refused_before_allocating() {
        use crate::io::MAX_ASSET_FILE_BYTES;

        // A sparse file reports past the file budget from metadata
        // while costing (almost) nothing on disk: the loader must
        // refuse it without allocating its contents, before any
        // per-primitive budget is even reached.
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("big.glb");
        let file = std::fs::File::create(&path).expect("sparse fixture must be creatable");
        file.set_len(MAX_ASSET_FILE_BYTES + 1)
            .expect("sparse resize must succeed");
        drop(file);

        let err = load_mesh(&path).expect_err("over-limit file must fail");
        match &err {
            AssetError::OverBudget { limit, actual, .. } => {
                assert_eq!(*limit, MAX_ASSET_FILE_BYTES);
                assert_eq!(*actual, MAX_ASSET_FILE_BYTES + 1);
            }
            other => panic!("over-limit file must be OverBudget, got: {other:?}"),
        }
        assert_eq!(
            err.path(),
            Some(path.as_path()),
            "the error must name the offending file"
        );
    }

    #[test]
    fn garbage_bytes_are_invalid_format() {
        let path = write_temp("garbage.glb", b"this is not a glb file at all");
        let err = load_mesh(&path).expect_err("garbage must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "garbage must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn truncated_fixture_is_invalid_format() {
        let full = std::fs::read(fixture("quad.glb")).expect("quad fixture must exist");
        let path = write_temp("truncated.glb", &full[..20]);
        let err = load_mesh(&path).expect_err("truncated file must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "truncated file must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn points_primitive_is_unsupported_not_misrendered() {
        let json = QUAD_JSON_TEMPLATE.replace("{MODE}", "1");
        let path = write_temp(
            "points.glb",
            &glb_bytes(&json, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("POINTS primitive must fail");
        assert!(
            matches!(err, AssetError::UnsupportedFeature { .. }),
            "non-triangle mode must be UnsupportedFeature, got: {err:?}"
        );
    }

    #[test]
    fn out_of_bounds_index_is_invalid_format() {
        let json = QUAD_JSON_TEMPLATE.replace("{MODE}", "4");
        let path = write_temp(
            "oob.glb",
            &glb_bytes(&json, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 99])),
        );
        let err = load_mesh(&path).expect_err("OOB index must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "OOB index must be InvalidFormat, got: {err:?}"
        );
        assert!(
            err.to_string().contains("99"),
            "message must name the index"
        );
    }

    #[test]
    fn inconsistent_normal_count_is_invalid_format() {
        let mut bin = quad_bin_with_indices(&[0, 1, 2, 0, 2, 3]);
        for _ in 0..9 {
            bin.extend_from_slice(&0.0f32.to_le_bytes());
        }
        let json = r#"{"accessors":[{"bufferView":0,"componentType":5126,"count":4,"max":[0.5,0.5,0.0],"min":[-0.5,-0.5,0.0],"type":"VEC3"},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"},{"bufferView":3,"componentType":5126,"count":3,"type":"VEC3"}],"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":48,"byteOffset":0},{"buffer":0,"byteLength":32,"byteOffset":48},{"buffer":0,"byteLength":12,"byteOffset":80},{"buffer":0,"byteLength":36,"byteOffset":92}],"buffers":[{"byteLength":128}],"meshes":[{"name":"bad","primitives":[{"attributes":{"NORMAL":3,"POSITION":0,"TEXCOORD_0":1},"indices":2,"mode":4}]}],"nodes":[{"mesh":0}],"scene":0,"scenes":[{"nodes":[0]}]}"#;
        let path = write_temp("inconsistent.glb", &glb_bytes(json, &bin));
        let err = load_mesh(&path).expect_err("3 normals vs 4 positions must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "inconsistent counts must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn non_indexed_primitive_is_unsupported() {
        let json = QUAD_JSON_TEMPLATE
            .replace("{MODE}", "4")
            .replace(r#""indices":2,"#, "");
        assert!(
            !json.contains("indices"),
            "surgery must actually drop the indices reference"
        );
        let path = write_temp(
            "non-indexed.glb",
            &glb_bytes(&json, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("non-indexed primitive must fail");
        assert!(
            matches!(err, AssetError::UnsupportedFeature { .. }),
            "non-indexed geometry must be UnsupportedFeature, got: {err:?}"
        );
    }

    #[test]
    fn external_buffer_reference_is_unsupported() {
        // Plain JSON (not GLB): the buffer lives in a sidecar file this
        // minimal loader deliberately never resolves.
        let json = r#"{"accessors":[{"bufferView":0,"componentType":5126,"count":1,"max":[0.0,0.0,0.0],"min":[0.0,0.0,0.0],"type":"VEC3"}],"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":12,"byteOffset":0}],"buffers":[{"byteLength":12,"uri":"sidecar.bin"}],"meshes":[{"name":"ext","primitives":[{"attributes":{"POSITION":0},"mode":4}]}],"nodes":[{"mesh":0}],"scene":0,"scenes":[{"nodes":[0]}]}"#;
        let path = write_temp("external.gltf", json.as_bytes());
        let err = load_mesh(&path).expect_err("external buffer must fail");
        assert!(
            matches!(err, AssetError::UnsupportedFeature { .. }),
            "sidecar buffers must be UnsupportedFeature, got: {err:?}"
        );
    }

    #[test]
    fn file_with_no_meshes_is_invalid_format() {
        let json = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[]}]}"#;
        let path = write_temp("empty.glb", &glb_bytes(json, &[]));
        let err = load_mesh(&path).expect_err("mesh-less file must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "no meshes must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn over_budget_vertex_claim_is_refused_before_allocating() {
        let json = QUAD_JSON_TEMPLATE.replace("{MODE}", "4").replace(
            r#""bufferView":0,"componentType":5126,"count":4"#,
            r#""bufferView":0,"componentType":5126,"count":2097152"#,
        );
        let path = write_temp(
            "over-budget.glb",
            &glb_bytes(&json, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("2M vertex claim must fail");
        assert!(
            matches!(err, AssetError::OverBudget { .. }),
            "over-budget claim must be OverBudget, got: {err:?}"
        );
    }

    #[test]
    fn loaded_mesh_flows_through_asset_store_with_stable_identity() {
        use crate::{AssetId, AssetStore, LOADER_VERSION};

        let path = fixture("quad.glb");
        let bytes = std::fs::read(&path).expect("quad fixture must exist");
        // Identity and values come from the same file bytes: the hash
        // mixes in LOADER_VERSION (proved in `id.rs` tests), so a loader
        // fix deterministically changes IDs without the loader itself
        // hashing anything.
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

        let meshes = load_mesh(&path).expect("quad fixture must load");
        let mut store = AssetStore::new();
        let handle = store.insert(meshes.into_iter().next().expect("one mesh"));
        let stored = store.get(handle).expect("live handle must resolve");
        assert_eq!(stored.triangle_count(), 2);
        assert_eq!(stored.indices(), &[0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn vertex_budget_boundary_accepts_exact_max_but_rejects_one_more() {
        let at_max = QUAD_JSON_TEMPLATE.replace("{MODE}", "4").replace(
            r#""bufferView":0,"componentType":5126,"count":4"#,
            &format!(
                r#""bufferView":0,"componentType":5126,"count":{MAX_MESH_VERTICES_PER_PRIMITIVE}"#
            ),
        );
        let path = write_temp(
            "vertex-at-max.glb",
            &glb_bytes(&at_max, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("tiny data under a max claim must still fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "count == MAX must pass the budget check and fail later on data, got: {err:?}"
        );
        let over_max = QUAD_JSON_TEMPLATE.replace("{MODE}", "4").replace(
            r#""bufferView":0,"componentType":5126,"count":4"#,
            &format!(
                r#""bufferView":0,"componentType":5126,"count":{}"#,
                MAX_MESH_VERTICES_PER_PRIMITIVE + 1
            ),
        );
        let path = write_temp(
            "vertex-over-max.glb",
            &glb_bytes(&over_max, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("MAX+1 vertex claim must fail");
        match err {
            AssetError::OverBudget { limit, actual, .. } => {
                assert_eq!(limit, MAX_MESH_VERTICES_PER_PRIMITIVE as u64);
                assert_eq!(actual, MAX_MESH_VERTICES_PER_PRIMITIVE as u64 + 1);
            }
            other => panic!("count == MAX+1 must be OverBudget, got: {other:?}"),
        }
    }

    #[test]
    fn index_budget_boundary_accepts_exact_max_but_rejects_one_more() {
        let at_max = QUAD_JSON_TEMPLATE.replace("{MODE}", "4").replace(
            r#""bufferView":2,"componentType":5123,"count":6"#,
            &format!(
                r#""bufferView":2,"componentType":5123,"count":{MAX_MESH_INDICES_PER_PRIMITIVE}"#
            ),
        );
        let path = write_temp(
            "index-at-max.glb",
            &glb_bytes(&at_max, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("tiny data under a max claim must still fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "count == MAX must pass the budget check and fail later, got: {err:?}"
        );
        let over_max = QUAD_JSON_TEMPLATE.replace("{MODE}", "4").replace(
            r#""bufferView":2,"componentType":5123,"count":6"#,
            &format!(
                r#""bufferView":2,"componentType":5123,"count":{}"#,
                MAX_MESH_INDICES_PER_PRIMITIVE + 1
            ),
        );
        let path = write_temp(
            "index-over-max.glb",
            &glb_bytes(&over_max, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("MAX+1 index claim must fail");
        match err {
            AssetError::OverBudget { limit, actual, .. } => {
                assert_eq!(limit, MAX_MESH_INDICES_PER_PRIMITIVE as u64);
                assert_eq!(actual, MAX_MESH_INDICES_PER_PRIMITIVE as u64 + 1);
            }
            other => panic!("count == MAX+1 must be OverBudget, got: {other:?}"),
        }
    }

    #[test]
    fn zero_vertex_primitive_is_invalid_format() {
        let json = QUAD_JSON_TEMPLATE.replace("{MODE}", "4").replace(
            r#""bufferView":0,"componentType":5126,"count":4"#,
            r#""bufferView":0,"componentType":5126,"count":0"#,
        );
        let path = write_temp(
            "zero-vertex.glb",
            &glb_bytes(&json, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("zero vertices must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "empty geometry must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn missing_position_attribute_is_invalid_format() {
        let json = QUAD_JSON_TEMPLATE.replace("{MODE}", "4").replace(
            r#""attributes":{"POSITION":0,"TEXCOORD_0":1}"#,
            r#""attributes":{"TEXCOORD_0":1}"#,
        );
        assert!(
            !json.contains("POSITION"),
            "surgery must actually drop the POSITION reference"
        );
        let path = write_temp(
            "no-position.glb",
            &glb_bytes(&json, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("missing POSITION must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "missing POSITION must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn index_count_not_a_multiple_of_three_is_invalid_format() {
        let json = QUAD_JSON_TEMPLATE.replace("{MODE}", "4").replace(
            r#""bufferView":2,"componentType":5123,"count":6"#,
            r#""bufferView":2,"componentType":5123,"count":5"#,
        );
        let path = write_temp(
            "mod3.glb",
            &glb_bytes(&json, &quad_bin_with_indices(&[0, 1, 2, 0, 2, 3])),
        );
        let err = load_mesh(&path).expect_err("5 indices must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "non-multiple-of-3 indices must be InvalidFormat, got: {err:?}"
        );
        assert!(
            err.to_string().contains('5'),
            "message must name the count, got: {err}"
        );
    }

    #[test]
    fn normals_load_alongside_geometry_without_changing_it() {
        let mut bin = quad_bin_with_indices(&[0, 1, 2, 0, 2, 3]);
        for _ in 0..12 {
            bin.extend_from_slice(&0.0f32.to_le_bytes());
        }
        let json = QUAD_JSON_TEMPLATE
            .replace("{MODE}", "4")
            .replace(
                r#"{"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"}"#,
                r#"{"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"},{"bufferView":3,"componentType":5126,"count":4,"type":"VEC3"}"#,
            )
            .replace(
                r#"{"buffer":0,"byteLength":12,"byteOffset":80}"#,
                r#"{"buffer":0,"byteLength":12,"byteOffset":80},{"buffer":0,"byteLength":48,"byteOffset":92}"#,
            )
            .replace(
                r#""buffers":[{"byteLength":92}]"#,
                r#""buffers":[{"byteLength":140}]"#,
            )
            .replace(
                r#""attributes":{"POSITION":0,"TEXCOORD_0":1}"#,
                r#""attributes":{"NORMAL":3,"POSITION":0,"TEXCOORD_0":1}"#,
            );
        let path = write_temp("with-normals.glb", &glb_bytes(&json, &bin));
        let meshes = load_mesh(&path).expect("quad plus NORMAL must load");
        assert_eq!(meshes.len(), 1);
        let plain = load_mesh(&fixture("quad.glb")).expect("quad fixture must load");
        assert_eq!(
            meshes[0].positions(),
            plain[0].positions(),
            "carrying normals must not alter positions"
        );
        assert_eq!(
            meshes[0].indices(),
            plain[0].indices(),
            "carrying normals must not alter indices"
        );
        assert_eq!(
            meshes[0].normals().map(<[[f32; 3]]>::len),
            Some(4),
            "normals load and are stored even though the renderer ignores them \
             (arch H2: Mesh.normals is Phase 12's decision, not this phase's)"
        );
    }

    #[test]
    fn interleaved_position_and_uv_views_resolve_through_stride() {
        // Given: the quad's positions and UVs interleaved per-vertex
        // ([pos:3xf32, uv:2xf32], stride 20) instead of the fixture's
        // two contiguous blocks.
        let positions: [f32; 12] = [
            -0.5, -0.5, 0.0, 0.5, -0.5, 0.0, 0.5, 0.5, 0.0, -0.5, 0.5, 0.0,
        ];
        let uvs: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        let mut bin = Vec::with_capacity(92);
        for vertex in 0..4 {
            for component in &positions[vertex * 3..vertex * 3 + 3] {
                bin.extend_from_slice(&component.to_le_bytes());
            }
            for component in &uvs[vertex * 2..vertex * 2 + 2] {
                bin.extend_from_slice(&component.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2, 0, 2, 3] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let json = r#"{"accessors":[{"bufferView":0,"componentType":5126,"count":4,"max":[0.5,0.5,0.0],"min":[-0.5,-0.5,0.0],"type":"VEC3"},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"}],"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":80,"byteOffset":0,"byteStride":20},{"buffer":0,"byteLength":68,"byteOffset":12,"byteStride":20},{"buffer":0,"byteLength":12,"byteOffset":80}],"buffers":[{"byteLength":92}],"meshes":[{"name":"quad","primitives":[{"attributes":{"POSITION":0,"TEXCOORD_0":1},"indices":2,"mode":4}]}],"nodes":[{"mesh":0}],"scene":0,"scenes":[{"nodes":[0]}]}"#;
        let path = write_temp("interleaved.glb", &glb_bytes(json, &bin));

        // When: loaded.
        let meshes = load_mesh(&path).expect("interleaved views must load");

        // Then: identical geometry to the contiguous-layout fixture —
        // stride arithmetic in the reader must land on the same floats.
        let plain = load_mesh(&fixture("quad.glb")).expect("quad fixture must load");
        assert_eq!(meshes.len(), 1);
        assert_eq!(
            meshes[0].positions(),
            plain[0].positions(),
            "strided positions must match the contiguous fixture"
        );
        assert_eq!(
            meshes[0].uvs(),
            plain[0].uvs(),
            "strided UVs must match the contiguous fixture"
        );
        assert_eq!(meshes[0].indices(), &[0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn sparse_position_override_resolves_instead_of_silently_zeroing() {
        // Given: positions stored as all zeros with a sparse patch
        // replacing vertex 0 with (-0.5, -0.5, 0.0) — the shape an
        // exporter emits for mostly-default morph or delta data.
        let mut bin = vec![0u8; 48];
        for v in [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0] {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        for i in [0u16, 1, 2, 0, 2, 3] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        bin.extend_from_slice(&0u16.to_le_bytes());
        for v in [-0.5f32, -0.5, 0.0] {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(bin.len(), 106);
        let json = r#"{"accessors":[{"bufferView":0,"componentType":5126,"count":4,"max":[0.5,0.5,0.0],"min":[-0.5,-0.5,0.0],"type":"VEC3","sparse":{"count":1,"indices":{"bufferView":3,"componentType":5123},"values":{"bufferView":4}}},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":2,"componentType":5123,"count":6,"type":"SCALAR"}],"asset":{"version":"2.0"},"bufferViews":[{"buffer":0,"byteLength":48,"byteOffset":0},{"buffer":0,"byteLength":32,"byteOffset":48},{"buffer":0,"byteLength":12,"byteOffset":80},{"buffer":0,"byteLength":2,"byteOffset":92},{"buffer":0,"byteLength":12,"byteOffset":94}],"buffers":[{"byteLength":106}],"meshes":[{"name":"sparse","primitives":[{"attributes":{"POSITION":0,"TEXCOORD_0":1},"indices":2,"mode":4}]}],"nodes":[{"mesh":0}],"scene":0,"scenes":[{"nodes":[0]}]}"#;
        let path = write_temp("sparse.glb", &glb_bytes(json, &bin));

        // When: loaded.
        let meshes = load_mesh(&path).expect("sparse positions must load");

        // Then: vertex 0 carries the sparse value — a reader that ignored
        // the sparse patch would silently serve zeros, which the
        // count cross-checks cannot catch (the count still agrees).
        assert_eq!(meshes.len(), 1);
        assert_f32_slice_eq(
            &flatten3(meshes[0].positions()),
            &[-0.5, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
    }

    #[test]
    fn git_lfs_pointer_bytes_are_invalid_format_not_a_panic() {
        // Given: what a fixture resolves to when Git LFS objects were not
        // fetched (CI without `lfs: true`): pointer text, not a GLB.
        let pointer =
            b"version https://git-lfs.github.com/spec/v1\noid sha256:0000000000000000000000000000000000000000000000000000000000000000\nsize 12345\n";
        let path = write_temp("lfs-pointer.glb", pointer);

        // When: loaded. Then: Err (InvalidFormat), never a panic.
        let err = load_mesh(&path).expect_err("LFS pointer text must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "LFS pointer bytes must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn missing_parent_directory_is_io_not_a_panic() {
        // Given: a path under a directory that does not exist at all.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/does-not-exist-dir/quad.glb");

        // When: loaded. Then: Io (the read fails before any parse).
        let err = load_mesh(&path).expect_err("missing directory must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "missing parent directory must be Io, got: {err:?}"
        );
    }

    #[test]
    fn bad_glb_magic_is_invalid_format() {
        // Given: the quad fixture with its 4 magic bytes corrupted.
        let mut bytes = std::fs::read(fixture("quad.glb")).expect("quad fixture must exist");
        bytes[0..4].copy_from_slice(b"BAD!");
        let path = write_temp("bad-magic.glb", &bytes);

        // When: loaded. Then: InvalidFormat, never a panic.
        let err = load_mesh(&path).expect_err("bad magic must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "corrupt magic must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn bad_glb_version_is_invalid_format() {
        // Given: the quad fixture claiming GLB version 99.
        let mut bytes = std::fs::read(fixture("quad.glb")).expect("quad fixture must exist");
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        let path = write_temp("bad-version.glb", &bytes);

        // When: loaded. Then: InvalidFormat, never a panic.
        let err = load_mesh(&path).expect_err("bad version must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "unsupported GLB version must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn lying_json_chunk_length_is_invalid_format_not_a_panic() {
        // Given: the quad fixture with its JSON chunk length (bytes 12..16)
        // inflated to u32::MAX — the chunk overruns the file.
        let mut bytes = std::fs::read(fixture("quad.glb")).expect("quad fixture must exist");
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        let path = write_temp("lying-chunk-length.glb", &bytes);

        // When: loaded. Then: InvalidFormat (bounds-checked), never a panic
        // and never a gigapixel-class allocation attempt.
        let err = load_mesh(&path).expect_err("lying chunk length must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "overrunning chunk length must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn truncated_bin_tail_is_invalid_format() {
        // Given: the quad fixture with its BIN tail cut off — the header
        // still declares the full counts, so the collects come up short.
        let full = std::fs::read(fixture("quad.glb")).expect("quad fixture must exist");
        let path = write_temp("short-bin.glb", &full[..full.len() - 10]);

        // When: loaded. Then: InvalidFormat via the count cross-checks.
        let err = load_mesh(&path).expect_err("short BIN must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "truncated BIN tail must be InvalidFormat, got: {err:?}"
        );
    }

    #[test]
    fn gltf_parser_panic_becomes_invalid_format() {
        // Given: the minimized fuzz finding (Phase 10, oracle-first
        // bounded fuzz at 4096 cases): the quad fixture with 4 JSON-chunk
        // bytes at offset 22 overwritten with 0x46546C67. The rewritten
        // JSON leaves a primitive referencing an accessor index the
        // file no longer declares, and `gltf-json 1.4.1`'s primitive
        // validator indexes `root.accessors` blindly — panicking with
        // "index out of bounds: the len is 0 but the index is 0"
        // instead of returning its error type.
        let mut bytes = std::fs::read(fixture("quad.glb")).expect("quad fixture must exist");
        bytes[22..26].copy_from_slice(&1179937895u32.to_le_bytes());
        let path = write_temp("dangling-accessor.glb", &bytes);

        // When: loaded. Then: InvalidFormat via the parse boundary
        // guard — never a panic propagating out of the dependency.
        let err = load_mesh(&path).expect_err("dangling accessor reference must fail");
        assert!(
            matches!(err, AssetError::InvalidFormat { .. }),
            "a trapped parser panic must surface as InvalidFormat, got: {err:?}"
        );
    }

    /// One hostile shape applied to a valid fixture's exact bytes: the
    /// chaos feed (chunk-length inflation, short-BIN splices, mid-file
    /// truncation) expressed as a composable grammar, plus single-byte
    /// flips and u32 patches over the header/chunk region.
    #[derive(Debug, Clone)]
    enum GlbFuzzMutation {
        /// XOR one byte with a nonzero mask.
        Flip { offset: usize, xor: u8 },
        /// Cut the file to `len` bytes (0 = empty file).
        Truncate { len: usize },
        /// Overwrite 4 bytes with `value` (little-endian: GLB header and
        /// chunk lengths are LE, so this is chunk-length inflation when
        /// it lands on bytes 12..16).
        PatchU32 { offset: usize, value: u32 },
        /// Drop the last `cut` bytes (short-BIN splice when small,
        /// whole-chunk amputation when large).
        CutTail { cut: usize },
    }

    /// Strategy over [`GlbFuzzMutation`]; `max_len` is the longest base
    /// input so every generated offset/length clamps onto a real index
    /// at apply time (never panics while building the input).
    fn glb_mutation_strategy(
        max_len: usize,
    ) -> impl proptest::strategy::Strategy<Value = GlbFuzzMutation> {
        use proptest::prelude::*;
        prop_oneof![
            (0..max_len, 1u8..=255).prop_map(|(offset, xor)| GlbFuzzMutation::Flip { offset, xor }),
            (0..=max_len).prop_map(|len| GlbFuzzMutation::Truncate { len }),
            (
                0..max_len,
                prop_oneof![
                    Just(0u32),
                    Just(1u32),
                    Just(u32::MAX),
                    Just(0x4654_6C67u32),
                    any::<u32>(),
                ],
            )
                .prop_map(|(offset, value)| GlbFuzzMutation::PatchU32 { offset, value }),
            (1..=max_len).prop_map(|cut| GlbFuzzMutation::CutTail { cut }),
        ]
    }

    /// Applies a mutation to `base`; total over all inputs (every
    /// out-of-range offset/length clamps instead of indexing blindly,
    /// so the fuzzer itself can never be the source of a panic).
    fn apply_glb_mutation(base: &[u8], mutation: &GlbFuzzMutation) -> Vec<u8> {
        assert!(
            !base.is_empty(),
            "fuzz bases are checked-in fixtures, never empty"
        );
        match *mutation {
            GlbFuzzMutation::Flip { offset, xor } => {
                let mut out = base.to_vec();
                let index = offset % base.len();
                out[index] ^= xor;
                out
            }
            GlbFuzzMutation::Truncate { len } => base[..len.min(base.len())].to_vec(),
            GlbFuzzMutation::PatchU32 { offset, value } => {
                let mut out = base.to_vec();
                let start = offset % base.len();
                let end = (start + 4).min(base.len());
                let bytes = value.to_le_bytes();
                out[start..end].copy_from_slice(&bytes[..end - start]);
                out
            }
            GlbFuzzMutation::CutTail { cut } => {
                let keep = base.len().saturating_sub(cut);
                base[..keep].to_vec()
            }
        }
    }

    /// The mesh-side oracle's Ok arm: every successfully loaded mesh must
    /// satisfy the loader's own documented invariants (non-empty,
    /// triangle-only index shape, in-bounds indices, consistent
    /// attribute counts). An Ok that violates any of these is a
    /// wrong-Ok — worse than an Err, because a corrupt mesh would flow
    /// silently into the renderer bridge.
    fn assert_mesh_invariants(meshes: &[Mesh]) {
        assert!(
            !meshes.is_empty(),
            "an Ok load must yield at least one mesh, never an empty Vec"
        );
        for mesh in meshes {
            assert!(
                !mesh.positions().is_empty(),
                "positions are non-empty by construction"
            );
            assert!(
                !mesh.indices().is_empty(),
                "indices are non-empty by construction"
            );
            assert_eq!(
                mesh.indices().len() % 3,
                0,
                "indices must stay a multiple of 3, got {}",
                mesh.indices().len()
            );
            let vertices = mesh.vertex_count();
            for index in mesh.indices() {
                assert!(
                    (*index as usize) < vertices,
                    "index {index} escapes {vertices} vertices"
                );
            }
            if let Some(normals) = mesh.normals() {
                assert_eq!(
                    normals.len(),
                    vertices,
                    "NORMAL count must match POSITION count"
                );
            }
            if let Some(uvs) = mesh.uvs() {
                assert_eq!(
                    uvs.len(),
                    vertices,
                    "TEXCOORD_0 count must match POSITION count"
                );
            }
            assert_eq!(
                mesh.triangle_count(),
                mesh.indices().len() / 3,
                "triangle_count must agree with the index buffer"
            );
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config { cases: 256, ..Default::default() })]
        /// Oracle-first bounded GLB fuzz: mutated fixture bytes fed to
        /// `load_mesh_from_bytes` must NEVER panic, hang, or wrong-Ok.
        /// Any Err variant is acceptable (the taxonomy is exhaustive by
        /// construction); an Ok must satisfy [`assert_mesh_invariants`].
        /// Seeded by the chaos feed: chunk-length inflation
        /// (`PatchU32` on the length fields, incl. `u32::MAX`),
        /// short-BIN splices (`CutTail`), mid-file truncation
        /// (`Truncate`), plus byte flips. `cargo-fuzz` was judged
        /// unnecessary: inputs stay under 1.3 KiB, the parser under
        /// test is our validation logic (the `gltf` crate owns raw
        /// container parsing), and proptest yields deterministic
        /// shrinking plus in-repo regression files for free.
        #[test]
        fn glb_byte_mutations_never_panic_and_ok_means_valid(
            use_box in proptest::bool::ANY,
            mutation in glb_mutation_strategy(1292)
        ) {
            // Given: one checked-in fixture's exact bytes, hostilely mutated.
            let name = if use_box { "box.glb" } else { "quad.glb" };
            let base = std::fs::read(fixture(name)).expect("fixture must exist");
            let bytes = apply_glb_mutation(&base, &mutation);

            // When: parsed as a GLB in memory (no disk involved, so the
            // read-before-budget R1 path is out of scope by construction).
            let path = PathBuf::from("fuzz-input.glb");
            let result = load_mesh_from_bytes(&path, &bytes);

            // Then: Err of any typed variant, or an Ok whose every mesh
            // satisfies the loader invariants. A panic fails the run
            // (proptest records it); a hang would exceed the case budget
            // — inputs are ≤1292 B so decode is microseconds.
            if let Ok(meshes) = result {
                assert_mesh_invariants(&meshes);
            }
        }
    }
}
