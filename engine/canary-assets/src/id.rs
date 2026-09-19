// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::fmt;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::AssetError;

/// Version string mixed into every [`AssetId`] hash input.
///
/// Baking the loader version into the hash is
/// `docs/architecture/asset-system.md`'s "(source bytes + importer
/// version)" rule in miniature (see ADR 0018): a loader fix changes the
/// input, so it deterministically changes every ID instead of silently
/// serving stale bytes under an old ID. Bump this — and only this —
/// when a loader's *interpretation* of bytes changes; identical bytes
/// under one version always yield one ID.
pub const LOADER_VERSION: &str = "canary-assets-loader/1";

/// Lowercase hex alphabet used by [`AssetId::to_hex`].
const HEX_ALPHABET: &[u8; 16] = b"0123456789abcdef";

/// Length of an [`AssetId`]'s hex rendering (32 digest bytes × 2).
const HEX_LENGTH: usize = 64;

/// Opaque content identifier for one asset: SHA-256 over (loader
/// version + file bytes).
///
/// `AssetId` answers "which *content* is this?" — never "where does it
/// live?". Paths conflate location with content and break the moment
/// cooking, caching, or packages remap either, which is why ADR 0018
/// rejected path-string identity outright. Two loads of byte-identical
/// files agree; a loader fix (see [`LOADER_VERSION`]) disagrees by
/// design.
///
/// The byte layout is **provisional**: version bumps change IDs, so
/// nothing downstream may persist an `AssetId` as permanent or compare
/// IDs across loader versions until the cooked-format ADR exists. This
/// is documented on the type itself (not just the roadmap) because the
/// type is where a future persister would reach for it.
///
/// The digest bytes stay private — not out of secrecy, but as the
/// backend-trait hygiene this workspace already enforces (no
/// third-party types in public signatures): exposing `sha2`'s output
/// type would weld this API to one hash crate. Hex text and [`fmt::Display`]
/// are the only renderings, and both are crate-computed strings, so the
/// hash backend can change without touching a single call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetId {
    digest: [u8; 32],
}

impl AssetId {
    /// Computes the ID for in-memory `bytes` under the current
    /// [`LOADER_VERSION`].
    ///
    /// Hash input layout is `LOADER_VERSION` bytes, one `0x00`
    /// separator, then `bytes`. The separator exists so that no
    /// (version, bytes) pair can collide with another pair whose
    /// version is a prefix-extension of the first — without it,
    /// version `"ab"` + bytes `"c"` and version `"a"` + bytes `"bc"`
    /// would hash identically.
    pub fn new(bytes: &[u8]) -> Self {
        Self::with_version(bytes, LOADER_VERSION)
    }

    /// Computes the ID for in-memory `bytes` under an explicit
    /// `version` string instead of [`LOADER_VERSION`].
    ///
    /// This is the seam that makes version-sensitivity testable: tests
    /// (and, later, versioned loaders) can prove that a bump changes
    /// IDs without mutating the global constant. Production loaders
    /// call [`AssetId::new`]; this entry point is for anything that
    /// needs to name *which* version it hashed under.
    pub fn with_version(bytes: &[u8], version: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(version.as_bytes());
        hasher.update([0x00]);
        hasher.update(bytes);
        let output = hasher.finalize();
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&output);
        AssetId { digest }
    }

    /// Reads the file at `path` and returns its [`AssetId`] under the
    /// current [`LOADER_VERSION`].
    ///
    /// This is identity, not loading: it moves bytes from disk to a
    /// hash and parses nothing, so Phase 2 loaders reuse it rather
    /// than re-implementing file reads. A missing or unreadable file
    /// yields [`AssetError::Io`] with the path attached — never a
    /// panic — because asset code runs against user-supplied paths.
    pub fn for_file(path: &Path) -> Result<Self, AssetError> {
        let bytes = std::fs::read(path).map_err(|source| AssetError::io(path, source))?;
        Ok(Self::new(&bytes))
    }

    /// Renders this ID as 64 lowercase hex characters.
    ///
    /// Hex (not base64, not raw bytes) because IDs appear in logs,
    /// filenames, and future cache keys, where case-sensitive,
    /// padding-free, filesystem-safe text survives every transport.
    /// Encoded by hand rather than via a `hex` dependency: it is six
    /// lines, and a whole crate for it would be a pin to maintain
    /// against the workspace's transitive-pin policy for nothing.
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(HEX_LENGTH);
        for byte in self.digest {
            out.push(HEX_ALPHABET[(byte >> 4) as usize] as char);
            out.push(HEX_ALPHABET[(byte & 0x0F) as usize] as char);
        }
        out
    }

    /// Parses an ID back from its [`AssetId::to_hex`] rendering.
    ///
    /// The round trip (`to_hex` → `from_hex`) is what future cache
    /// filenames and log scrapers depend on, so it is tested here,
    /// not left as an implied property. Garbage (wrong length or
    /// non-hex characters) yields [`AssetError::InvalidFormat`] —
    /// never a panic — with the offending text as the "path", since a
    /// malformed ID string is malformed *content* arriving from outside
    /// the trust boundary, the same category as malformed file bytes.
    pub fn from_hex(hex: &str) -> Result<Self, AssetError> {
        let invalid = |reason: &str| AssetError::invalid_format(Path::new(hex), reason);
        if hex.len() != HEX_LENGTH {
            return Err(invalid("asset id hex must be 64 characters"));
        }
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("asset id hex contains non-hex characters"));
        }
        let mut digest = [0u8; 32];
        for (slot, chunk) in digest.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let pair = std::str::from_utf8(chunk)
                .map_err(|_| invalid("asset id hex is not valid UTF-8"))?;
            *slot = u8::from_str_radix(pair, 16)
                .map_err(|_| invalid("asset id hex contains non-hex characters"))?;
        }
        Ok(AssetId { digest })
    }
}

impl fmt::Display for AssetId {
    /// Renders the ID exactly as [`AssetId::to_hex`] does, so `format!("{id}")`
    /// is usable wherever a cache key, filename, or log field is needed
    /// without the caller remembering which method to call.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_bytes_yield_identical_ids() {
        let left = AssetId::new(b"quad positions");
        let right = AssetId::new(b"quad positions");
        assert_eq!(left, right, "content addressing must be deterministic");
    }

    #[test]
    fn different_bytes_yield_different_ids() {
        let left = AssetId::new(b"quad");
        let right = AssetId::new(b"box");
        assert_ne!(left, right, "distinct content must not share an ID");
    }

    #[test]
    fn loader_version_bump_changes_the_id() {
        let under_v1 = AssetId::with_version(b"same bytes", "canary-assets-loader/1");
        let under_v2 = AssetId::with_version(b"same bytes", "canary-assets-loader/2");
        assert_ne!(
            under_v1, under_v2,
            "a loader fix must deterministically change IDs, not silently reuse them"
        );
    }

    #[test]
    fn current_version_matches_the_constant() {
        assert_eq!(
            AssetId::new(b"probe"),
            AssetId::with_version(b"probe", LOADER_VERSION),
            "`new` must hash under LOADER_VERSION, not a stale copy of it"
        );
    }

    #[test]
    fn hex_round_trip_recovers_the_id() {
        let id = AssetId::new(b"round trip me");
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64, "SHA-256 must render as 64 hex chars");
        assert_eq!(
            AssetId::from_hex(&hex).expect("hex we just rendered must parse"),
            id,
            "hex must parse back to the same ID"
        );
        assert_eq!(id.to_string(), hex, "Display must agree with to_hex");
    }

    #[test]
    fn from_hex_rejects_garbage_without_panicking() {
        assert!(
            AssetId::from_hex("not hex at all").is_err(),
            "short garbage must be Err"
        );
        assert!(
            AssetId::from_hex(&"zz".repeat(32)).is_err(),
            "64 non-hex chars must be Err"
        );
        assert!(
            AssetId::from_hex(&"ab".repeat(31)).is_err(),
            "62 hex chars must be Err"
        );
    }

    #[test]
    fn for_file_hashes_disk_bytes_like_new() {
        let dir = std::env::temp_dir().join("canary-assets-id-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("probe.bin");
        std::fs::write(&path, b"on-disk bytes").expect("fixture write must succeed");
        let from_disk = AssetId::for_file(&path).expect("readable file must hash");
        assert_eq!(from_disk, AssetId::new(b"on-disk bytes"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn for_file_reports_a_missing_file_as_io_error() {
        let missing = Path::new("definitely-not-an-asset.glb");
        let err = AssetId::for_file(missing).expect_err("missing file must fail");
        assert!(
            matches!(err, AssetError::Io { .. }),
            "missing file must be Io, got: {err:?}"
        );
    }
}
