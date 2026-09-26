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
//! - [`Sound`] + [`load_sound`] / [`load_sound_with_budget`]:
//!   WAV (8/16/24/32-bit integer PCM, 32-bit float) and Ogg Vorbis
//!   decoded and normalized to interleaved `f32` PCM in `[-1, 1]`
//!   (mono/stereo only) under an enforced decode budget, with the same
//!   root confinement as every other loader.
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
//! `png` (for [`load_texture`]'s decoder), and `hound` + `lewton`
//! (for [`load_sound`]'s WAV and Ogg Vorbis decoders — pure Rust,
//! MIT/Apache-2.0 only; Symphonia's MPL-2.0 is deliberately declined,
//! see `Cargo.toml`). It knows nothing about
//! `canary-render` or any backend; composition flows upward, keeping
//! `canary-render`'s zero-dependency invariant intact.
//!
//! Third-party loader types (`gltf`, `png`, `hound`, `lewton`) appear only in function
//! bodies and private helpers of `mesh` and `texture` — never in a
//! public signature (backend-trait hygiene extended to the asset
//! boundary, checked via `cargo doc`).

mod error;
mod handle;
mod id;
mod io;
mod mesh;
mod sound;
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
pub use sound::{
    load_sound, load_sound_with_budget, load_sound_with_budget_within_root, load_sound_within_root,
    Sound, DEFAULT_MAX_SOUND_SAMPLES,
};
pub use store::AssetStore;
pub use texture::{
    load_texture, load_texture_with_budget, load_texture_with_budget_within_root,
    load_texture_within_root, Texture, DEFAULT_MAX_TEXTURE_BYTES,
};
