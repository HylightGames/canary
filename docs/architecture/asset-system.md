# Asset System

Architecture for Era 3 (see
[`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md)). No
asset pipeline code exists in v0.0.1.

## Source assets vs. cooked assets

Canary separates **source assets** (the files a content author edits —
`.png`, `.fbx`, `.wav`, hand-authored material/scene descriptions) from
**cooked assets** (the engine-optimized runtime format actually loaded at
play time — compressed textures in a GPU-friendly layout, a compiled mesh
format, etc.). A cook/import step transforms the former into the latter.
This split exists so that:

- Source assets stay in author-friendly, tool-friendly formats (a `.png` can
  be opened by any image editor; a cooked, platform-specific texture format
  generally can't).
- The runtime never has to parse or interpret arbitrary source formats — it
  only ever loads the engine's own cooked format, which simplifies both the
  loader code and its trust boundary (a cooked asset from the pipeline is a
  known, validated shape; a raw third-party file format parser is a much
  larger attack surface).
- Different target platforms can cook the same source asset differently
  (texture compression format, endianness, precision) without the source
  asset itself needing platform-specific variants.

## Content addressing and caching

Cooked assets are identified and cached by a content hash of (source asset
bytes + importer version + import settings), not by file path alone. This
means:

- Re-running the cook step is a no-op for anything unchanged — a large
  project's iteration loop stays fast as it grows, rather than degrading
  linearly with asset count.
- Changing an importer's version invalidates exactly the assets that
  importer produced, deterministically, rather than requiring a manual
  "clean rebuild everything" step.
- The cache is a legitimate target for the build orchestration tooling in
  [`docs/development/build-system.md`](../development/build-system.md) (the
  `xtask` crate) to expose as an explicit, inspectable step, rather than a
  hidden implementation detail of the editor.

## Hot reload

In editor/dev builds, a changed source asset triggers a re-cook and the
running engine swaps the loaded asset in place (a texture updates on
screen; a re-imported mesh updates without a scene reload), following the
same "swap at a defined safe point" pattern used for script hot reload in
[scripting-system.md](scripting-system.md#hot-reload). This is explicitly a
dev/editor-build feature — shipped/release builds load cooked assets from a
sealed package and don't watch the filesystem for source-asset changes.

## Importers as plugins

Asset importers (the code that turns a specific source format into a cooked
asset) are themselves expected to be built on the plugin system
([plugin-system.md](plugin-system.md)) rather than hard-coded into the
engine core — supporting a new source format (a niche 3D format, a
proprietary audio format) should be addable without a core engine change, in
keeping with "everything built around plugins" and "replaceable engine
subsystems" as stated goals. Whether a given importer belongs in the
sandboxed (Tier A) or trusted (Tier B) tier depends on the same
trust/performance tradeoff discussed in
[plugin-system.md](plugin-system.md#why-two-tiers-instead-of-one) — an
importer that only runs at author-time during cooking (never on an
untrusted end user's machine) has a much weaker case for sandboxing than one
that might run inside a shipped game.

## Status in this foundation

`v0.0.10` landed the loading primitive's first slice (see
[`v0.0.10-roadmap.md`](../roadmap/v0.0.10-roadmap.md) and [ADR
0018](../decisions/architecture-decision-records/0018-asset-handles-and-synchronous-loading.md)).
`engine/canary-assets` now holds `AssetId` (an opaque SHA-256 over file
bytes plus a loader-version string, with a layout documented as
provisional), `AssetHandle<T>` (a generational index-plus-generation key
mirroring `Entity`'s proven shape), `AssetStore<T>` (generational slots
kept as an ECS resource, where stale handles resolve to `None` instead of
panicking), and `AssetError` (typed failures carrying path context) — plus
exactly two synchronous, path-based loaders. The GLB mesh loader turns
every triangle primitive of a self-contained GLB file into its own
validated `Mesh` (positions plus indices always present; normals and UVs
loaded and stored even though the renderer ignores them this release;
index bounds, attribute-count consistency, and triangle-mode-only enforced
at load time). The PNG texture loader decodes to RGBA8-normalized
`Texture` values (every color type converted, samples deeper than 8 bits
downsampled to their most significant byte, an enforced decode budget
refusing lying headers before allocating). Fixtures are checked in and
hash-stable (`engine/canary-assets/tests/fixtures/`: `quad.glb`,
`box.glb`, `rgba2x2.png`, `rgba16-2x1.png`), with known-value assertions
and negative controls proving every rejection path returns `Err`. Those
file-loaded meshes and textures reach the screen through the
`canary-render-ecs` bridge (see [`rendering.md`](rendering.md)), and
`examples/spinning-cube` loads its cube faces from `box.glb` instead of
hardcoded arrays.

Deliberately still deferred, none of it partially implemented: async or
background loading, filesystem watching and hot reload, a cache directory,
cooked formats and any `xtask cook` step, importers-as-plugins (exactly
two hardcoded loaders exist), materials, depth testing, blending, buffer
or texture updates, swapchain and presentation, second mesh or texture
formats, mipmaps, and sRGB handling past normalize-to-RGBA8. No cooking,
no cache, no hot reload, and no general material system exist yet.
