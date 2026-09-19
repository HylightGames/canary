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
//! # What is implemented (v0.0.10 Phase 1)
//!
//! Identity, handles, storage, and errors — the Terms in which every
//! later loader is written:
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
//!   `UnknownHandle`).
//!
//! # What is explicitly stub (later phases own it)
//!
//! - **Loaders** (Phase 2): GLB mesh + PNG texture parsing, fixtures,
//!   and known-value tests. Nothing here reads a format —
//!   [`AssetId::for_file`] hashes raw bytes without interpreting them.
//! - **Renderer wiring** (Phase 3): `Mesh`/`Texture` asset types,
//!   `MeshRenderable`, bridge extraction, and any RHI surface.
//! - **Async loading, caching, hot reload, importers-as-plugins**
//!   (later asset milestones per `asset-system.md`): this release is
//!   synchronous and path-based only, because the scheduler has no
//!   threading story for background loading yet.
//!
//! # A note on dependencies
//!
//! This crate depends only on `canary-ecs` (for the resource seam its
//! store plugs into — used in tests, not in store logic), `thiserror`
//! (for [`AssetError`]), and `sha2` (for [`AssetId`]'s hash backend,
//! kept private behind hex rendering). It knows nothing about
//! `canary-render` or any backend; composition flows upward, keeping
//! `canary-render`'s zero-dependency invariant intact.

mod error;
mod handle;
mod id;
mod store;

pub use error::AssetError;
pub use handle::AssetHandle;
pub use id::{AssetId, LOADER_VERSION};
pub use store::AssetStore;
