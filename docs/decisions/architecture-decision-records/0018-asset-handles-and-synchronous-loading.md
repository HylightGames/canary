# 0018. Asset handles: content-hashed IDs, generational handles, synchronous loading

**Status:** Accepted

## Context

`v0.0.10` introduces `canary-assets` — per
[`docs/roadmap/v0.1.0-plan.md`](../../roadmap/v0.1.0-plan.md), the
loading primitive audio (`v0.0.12`) and everything file-fed depends
on, built once against the renderer as its first consumer. Three
coupled choices had to be made before any code, because each is
expensive to reverse once loaders, the renderer bridge, and (later)
audio all exist against them: how an asset is *identified*
(`AssetId`), how game code *holds* one (`AssetHandle<T>`), and
*whether loading is synchronous*.

A prior decision constrains the first: `asset-system.md` already
settles content addressing by hash of (source bytes + importer
version + import settings) for the cook/cache layer, and the
September review triage confirmed the genuinely open half is the
*runtime-facing* ergonomic API — an actual handle type game code
holds — not the identity scheme. Related constraints: ADR 0010's
"stable nominal IDs + host-internal registry, manual impl before
derive" precedent; triage #8's "every ID type stays its own type"
rule; `state-and-versioning.md`'s "stable asset ID is not a runtime
`Entity`"; and the scheduler's missing pieces (no work-stealing
pool, no concurrent disjoint writes, no command buffers), which any
async design would have to route around.

## Decision

- **`AssetId` is an opaque, crate-owned type** wrapping a content
  hash of (file bytes + loader version string), with hex/`Display`
  and value semantics (`Eq + Hash + Clone + Copy`-friendly). Baking
  the loader version into the hash input is `asset-system.md`'s
  "(source bytes + importer version)" rule in miniature: a loader
  fix deterministically changes IDs instead of silently serving
  stale bytes. The byte layout is documented as **provisional** —
  version bumps change IDs by design, so nothing downstream may
  treat IDs as permanent before the cooked-format ADR exists.
- **`AssetHandle<T>` is a generational handle** (`index` +
  `generation`, mirroring `Entity`'s proven shape) into an
  `AssetStore<T>`, with `T = Mesh | Texture` in this release.
  Stale-handle access returns `None` or a typed error, never panics
  — the same convention as `World::resource`. If R-33's future
  tombstone mechanism ever covers asset-handle removal, this shape
  already has a generation field to hang it on; until then,
  stale→`None` is the honest local answer, not a placeholder for
  that mechanism.
- **Loading is synchronous and path-based** (`load_mesh(path)`,
  `load_texture(path)`), with no cache directory, no file watching,
  and no async API. The store lives as an ECS **resource**, keeping
  load/store logic unit-testable without a `World`.
- **GLB for meshes, PNG for textures** — exactly one format each.
  GLB is self-contained (no sidecar-buffer path resolution),
  deterministic as a fixture, and designed as a runtime
  transmission format ("load, don't cook"); its `reader()` API maps
  1:1 onto `Mesh`. PNG via the pure-Rust `png` crate (with decode
  budgets enforced) matches this project's Rust-first values; the
  full `image` crate's dozen formats are attack surface, pins, and
  compile time this release's "at least one texture format" bar
  does not need.

See [`docs/roadmap/v0.0.10-roadmap.md`](../../roadmap/v0.0.10-roadmap.md)
for the release scope this decision belongs to (including the
two-slice renderer integration and everything explicitly deferred).

## Alternatives considered

- **`PathBuf` (or path-string) identity.** Rejected: already decided
  against at the docs level (content addressing is the design;
  triage confirmed it). Paths conflate location with content and
  break the moment cooking, caching, or packages remap either.
- **A generic/blessed ID type shared across assets, entities, or
  plugins.** Rejected per triage #8: `AssetId` is its own type, the
  same separation ADR 0010 keeps between `TypeId` and `SCHEMA_ID`.
- **Bare-index or refcounted handles.** Rejected: bare indices alias
  after remove+reinsert (pinned by a `proptest`, mirroring
  `canary-ecs`'s own generation reasoning); refcounting answers a
  lifetime question nothing yet asks — the store owns everything,
  handles are just keys.
- **OBJ instead of GLB.** Rejected: multi-file sidecars (`.obj` +
  `.mtl` + textures) complicate the single-file fixture story, and
  ecosystem direction (Khronos, Bevy's default loader) is glTF.
- **Full `image` crate instead of `png`.** Rejected: breadth this
  release cannot verify or bound; format breadth is future
  importer-plugin work.
- **Async / background loading now.** Rejected: with no pool, no
  concurrent writes, and no command buffers, a background loader
  would invent a threading story disconnected from the scheduler.
  Synchronous loading of kilobyte fixtures is honest at this scale;
  the async story starts when a measured load exceeds budget.

## Consequences

- Every subsystem that loads files (renderer bridge now, audio in
  `v0.0.12`, UI later) goes through `AssetId`/`AssetHandle<T>`/
  `AssetStore<T>` — one loading primitive, not per-subsystem file
  I/O. Third-party loader types (`gltf`, `png`) never appear in
  public signatures (backend-trait hygiene extended to the asset
  boundary).
- Fixture files (GLB quad/box, tiny RGBA PNG) are checked in and
  hash-stable; loader-output values, not toolchain behavior, are
  what tests pin.
- `v0.0.10` proves file-bytes-to-pixels for geometry with zero RHI
  churn, plus a minimal bounded texture slice; everything else
  (cooking, cache, hot reload, importers-as-plugins, materials,
  depth, swapchain, async) stays explicitly deferred with owning
  releases — none partially implemented.
- The new dependencies (`gltf`, `png`, hash crate + transitives)
  follow `build-system.md` pin policy with rationale comments and
  implementation-time re-verification, since none are in the
  lockfile today.
