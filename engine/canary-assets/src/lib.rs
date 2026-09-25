// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine asset loading: [`AssetId`] + [`AssetHandle<T>`] +
//! [`AssetStore<T>`] + [`AssetError`].
//!
//! See `docs/architecture/asset-system.md` for the full design and
//! [ADR 0018](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0018-asset-handles-and-synchronous-loading.md)
//! for the handle-design decisions this crate records.
//!
//! # What is implemented (v0.0.10 Phases 1–2)
//!
//! Identity, handles, storage, errors, and the first two loaders:
//!
//! - [`AssetId`]: opaque content hash of (file bytes + loader version),
//!   with hex/[`std::fmt::Display`] rendering only.
//! - [`AssetHandle<T>`]: generational `index` + `generation` key into
//!   an [`AssetStore<T>`], mirroring `Entity`'s proven shape.
//! - [`AssetStore<T>`]: generational slots with insert/get/remove;
//!   stale handles resolve to `None`, never panic. Lives as an ECS
//!   resource via `World::insert_resource`; the store itself takes no
//!   `World` dependency.
//! - [`AssetError`]: typed failures with path context (`Io`,
//!   `InvalidFormat`, `UnsupportedFeature`, `OverBudget`,
//!   `UnknownHandle`) plus `InvalidId`, which carries malformed hex
//!   text as data rather than as a path.
//! - [`Mesh`] + [`load_mesh`]: every triangle primitive of a
//!   self-contained GLB file becomes its own validated mesh value
//!   (index bounds, attribute consistency, triangle-mode only enforced
//!   at load time).
//! - [`Texture`] + [`load_texture`] / [`load_texture_with_budget`]:
//!   PNG decoded and normalized to RGBA8 (>8-bit samples documentedly
//!   downsampled) under an enforced decode budget.
//!
//! [`AssetId::for_file`] hashes raw file bytes without interpreting them,
//! so loaders reuse it for identity rather than re-implementing file
//! reads: file bytes go through the loader for values and through
//! `AssetId` for identity, with [`LOADER_VERSION`] mixed into every hash
//! input so a loader fix deterministically changes IDs.
//!
//! # What is explicitly stub (later phases own it)
//!
//! - **Renderer wiring** (Phase 3): `MeshRenderable`, bridge extraction,
//!   and any RHI surface. [`Mesh`]/[`Texture`] are renderer-agnostic
//!   values; index-to-soup expansion and texture upload happen at the
//!   bridge, not here.
//! - **Async loading, caching, hot reload, importers-as-plugins**
//!   (later asset milestones per `asset-system.md`): this release is
//!   synchronous and path-based only, because the scheduler has no
//!   threading story for background loading yet.
//!
//! # A note on dependencies
//!
//! This crate depends only on `canary-ecs` (for the resource seam its
//! store plugs into — used in tests, not in store logic), `thiserror`
//! (for [`AssetError`]), `sha2` (for [`AssetId`]'s hash backend, kept
//! private behind hex rendering), `gltf` without default features plus
//! only `utils` (for [`load_mesh`]'s primitive reader — the `import`
//! feature's `image` subtree is deliberately declined; see `Cargo.toml`),
//! and `png` (for [`load_texture`]'s decoder). It knows nothing about
//! `canary-render` or any backend; composition flows upward, keeping
//! `canary-render`'s zero-dependency invariant intact.
//!
//! Third-party loader types (`gltf`, `png`) appear only in function
//! bodies and private helpers of `mesh` and `texture` — never in a
//! public signature (backend-trait hygiene extended to the asset
//! boundary, checked via `cargo doc`).

mod error;
mod handle;
mod id;
mod io;
mod mesh;
mod store;
mod texture;

pub use error::AssetError;
pub use handle::AssetHandle;
pub use id::{AssetId, LOADER_VERSION};
pub use io::{resolve_in_root, MAX_ASSET_FILE_BYTES};
pub use mesh::{
    load_mesh, load_mesh_within_root, Mesh, MAX_MESH_INDICES_PER_PRIMITIVE,
    MAX_MESH_VERTICES_PER_PRIMITIVE,
};
pub use store::AssetStore;
pub use texture::{
    load_texture, load_texture_with_budget, load_texture_with_budget_within_root,
    load_texture_within_root, Texture, DEFAULT_MAX_TEXTURE_BYTES,
};
