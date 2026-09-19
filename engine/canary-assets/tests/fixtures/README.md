# `canary-assets` Phase 2 fixtures

Checked-in, hash-stable loader fixtures. Generated once by hand (no
exporter, no toolchain behavior to drift); tests assert loader *output
values* — positions, indices, RGBA bytes — never hashes of these files.

## How they were generated

A short dependency-free Python script (`python3`, standard library only:
`struct`, `json`, `zlib`) wrote each file byte-for-byte with fixed JSON
separators, sorted keys, explicit little-endian packing, and `zlib`
compression level 6 — all deterministic for identical input, so
regenerating from the same description yields identical bytes. The
generation logic (kept with the Phase 2 work, not checked in — the files
below are the artifact) was:

- **GLB**: 12-byte header (`0x46546C67`, version 2, total length), a `JSON`
  chunk (space-padded to 4 bytes), and a `BIN` chunk (zero-padded).
  Buffer views are tightly packed sequential slices; every accessor,
  offset, and count below was verified against the written bytes before
  check-in (header length equals file length; `buffers[0].byteLength`
  equals the BIN size).
- **PNG**: 8-byte signature, `IHDR` / single-`IDAT` / `IEND` chunks with
  CRC32, one filter byte (`0`, none) per scanline, `zlib.compress(raw,
  6)`.

## Contents

- `quad.glb` (772 bytes): one mesh (`"quad"`), one primitive, triangle
  mode. `POSITION` (4×VEC3 float, with spec-mandated min/max):
  (-0.5,-0.5,0), (0.5,-0.5,0), (0.5,0.5,0), (-0.5,0.5,0). `TEXCOORD_0` (4×VEC2 float):
  (0,0), (1,0), (1,1), (0,1). Indices (6×u16): 0,1,2, 0,2,3. No normals —
  the loader's `None` path is asserted against this file.
- `box.glb` (1292 bytes): one mesh (`"box"`), **two** primitives sharing
  one 328-byte buffer, proving one-primitive→one-mesh mapping. Both use
  the same 8-corner `POSITION` (±0.5 cube, with min/max). Primitive 0 carries the first
  18 indices (+z, -z, +x faces) with no optional attributes; primitive 1
  carries the remaining 18 (−x, +y, −y faces) plus `NORMAL` (8×VEC3, exact
  axis unit vectors cycling +x/+y/+z/−x/−y/−z/+x/+y) and `TEXCOORD_0`
  (8×VEC2 repeating (0,0),(1,0),(1,1),(0,1)).
- `rgba2x2.png` (2×2, 8-bit RGBA): row-major top-to-bottom red
  (255,0,0,255), green (0,255,0,255) / blue (0,0,255,255), white
  (255,255,255,255). The loader must return these 16 bytes exactly.
- `rgba16-2x1.png` (2×1, 16-bit RGBA): pixel 0
  (0xFFFF,0x0000,0x8000,0xFFFF) → (255,0,128,255); pixel 1
  (0x1234,0x5678,0x9ABC,0xDEF0) → (0x12,0x56,0x9A,0xDE). Proves the
  documented most-significant-byte downsampling with hand-checkable
  values.

## Negative controls

Corrupt, truncated, over-budget, and missing inputs are built at test
runtime (mutated copies of the files above, or nonexistent paths), not
checked in: they assert *rejection behavior* (`Err`, never panic), which
needs no stable bytes — only the positive fixtures pin values.
