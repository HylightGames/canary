// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::any::Any;

use crate::archetype::ArchetypeId;
use crate::column::Tick;

/// Per-slot bookkeeping: whether it's currently occupied, the
/// generation to stamp on the next entity that occupies it, and (while
/// occupied) where its row currently lives.
///
/// `generation` is `u64` -- see the type-level docs on
/// [`crate::Entity`] for why.
#[derive(Default)]
pub(crate) struct Slot {
    pub(crate) generation: u64,
    pub(crate) alive: bool,
    pub(crate) location: Option<EntityLocation>,
}

/// A single resource's storage: its value plus the [`Tick`] it was last
/// written at, mirroring what [`crate::column::TypedColumn`] tracks
/// per-row for components -- see
/// `docs/architecture/execution-model.md#resources`.
pub(crate) struct ResourceEntry {
    pub(crate) value: Box<dyn Any + Send + Sync>,
    pub(crate) changed_tick: Tick,
}

/// Where one entity's row currently lives: which archetype, and which
/// row within it. Updated every time an entity's component set changes
/// (moving it to a different archetype) or another entity's
/// `swap_remove`-driven relocation lands on top of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntityLocation {
    pub(crate) archetype: ArchetypeId,
    pub(crate) row: usize,
}
