// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Store-wide Tier A resource limits.
//!
//! [`crate::ResourceBudget::max_memory_bytes`] is enforced here as a
//! **total** ceiling shared by every linear memory and table in one
//! store, not as a per-memory ceiling. A guest that creates four
//! memories, each individually inside the old per-memory cap, could
//! otherwise commit four times the budget; the same was true of
//! funcref tables, which had no cap at all. Wasmtime reports creation
//! and growth through [`wasmtime::ResourceLimiter`], so this type
//! accounts for both and refuses the first allocation that would cross
//! the total.
//!
//! Table elements are charged at one pointer per element, which is what
//! Wasmtime allocates for a table. Counts are capped separately
//! ([`MAX_GUEST_MEMORIES`], [`MAX_GUEST_TABLES`], [`MAX_GUEST_INSTANCES`])
//! so a guest cannot spend the whole budget on millions of empty
//! objects. Fuel remains the per-call execution bound and is not part of
//! this accounting.

use std::mem::size_of;

use wasmtime::{ResourceLimiter, Result as WasmResult};

use super::budget::ResourceBudget;

/// Most linear memories one Tier A store may create. A Component Model
/// guest needs one per core instance; four is generous for the current
/// single-purpose plugins while still bounding count-independent
/// overhead.
pub(crate) const MAX_GUEST_MEMORIES: usize = 4;

/// Most tables one Tier A store may create. Tables are the one
/// allocation Wasmtime's per-memory limit never covered.
pub(crate) const MAX_GUEST_TABLES: usize = 16;

/// Most component instances one Tier A store may create.
pub(crate) const MAX_GUEST_INSTANCES: usize = 64;

/// Store-wide resource accounting for one Tier A instance.
///
/// Wired to the store through [`wasmtime::Store::limiter`]. A refused
/// growth returns `Ok(false)` (the guest observes `memory.grow`/`table.grow`
/// returning `-1`, matching Wasmtime's own growth-failure behavior), never
/// a panic or an ignored limit.
pub(crate) struct GuestLimits {
    /// Total bytes every memory and table in this store may occupy.
    budget_bytes: usize,
    /// Bytes currently reserved by allowed creations and growths.
    used_bytes: usize,
    /// Bytes reserved by the most recent allowed memory growth, so a
    /// failed host allocation can return them.
    pending_memory_delta: usize,
    /// Bytes reserved by the most recent allowed table growth, for the
    /// same reason.
    pending_table_delta: usize,
}

impl GuestLimits {
    /// Builds an empty ledger for one store.
    pub(crate) fn new(budget: &ResourceBudget) -> Self {
        Self {
            budget_bytes: budget.max_memory_bytes,
            used_bytes: 0,
            pending_memory_delta: 0,
            pending_table_delta: 0,
        }
    }

    /// Reserves `bytes` if the store-wide budget still allows them.
    /// Reservations are never released: Wasmtime offers no destruction
    /// callback, and nothing in a store frees a memory or table before
    /// the store itself drops.
    fn try_reserve(&mut self, bytes: usize) -> bool {
        let Some(total) = self.used_bytes.checked_add(bytes) else {
            return false;
        };
        if total > self.budget_bytes {
            return false;
        }
        self.used_bytes = total;
        true
    }

    /// Bytes a table of `elements` entries occupies, or `None` if the
    /// multiplication overflows.
    fn table_bytes(elements: usize) -> Option<usize> {
        elements.checked_mul(size_of::<usize>())
    }
}

impl ResourceLimiter for GuestLimits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> WasmResult<bool> {
        if maximum.is_some_and(|max| desired > max) {
            return Ok(false);
        }
        let delta = desired.saturating_sub(current);
        if delta == 0 {
            return Ok(true);
        }
        if !self.try_reserve(delta) {
            return Ok(false);
        }
        self.pending_memory_delta = delta;
        Ok(true)
    }

    fn memory_grow_failed(&mut self, _error: wasmtime::Error) -> WasmResult<()> {
        self.used_bytes = self.used_bytes.saturating_sub(self.pending_memory_delta);
        self.pending_memory_delta = 0;
        Ok(())
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> WasmResult<bool> {
        if maximum.is_some_and(|max| desired > max) {
            return Ok(false);
        }
        let added = desired.saturating_sub(current);
        if added == 0 {
            return Ok(true);
        }
        let Some(delta) = Self::table_bytes(added) else {
            return Ok(false);
        };
        if !self.try_reserve(delta) {
            return Ok(false);
        }
        self.pending_table_delta = delta;
        Ok(true)
    }

    fn table_grow_failed(&mut self, _error: wasmtime::Error) -> WasmResult<()> {
        self.used_bytes = self.used_bytes.saturating_sub(self.pending_table_delta);
        self.pending_table_delta = 0;
        Ok(())
    }

    fn instances(&self) -> usize {
        MAX_GUEST_INSTANCES
    }

    fn tables(&self) -> usize {
        MAX_GUEST_TABLES
    }

    fn memories(&self) -> usize {
        MAX_GUEST_MEMORIES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(bytes: usize) -> GuestLimits {
        GuestLimits::new(&ResourceBudget {
            max_memory_bytes: bytes,
            fuel: 1,
        })
    }

    #[test]
    fn memory_budget_is_shared_across_memories() {
        let mut limits = limits(64 * 1024);
        for _ in 0..4 {
            assert!(
                limits
                    .memory_growing(0, 16 * 1024, None)
                    .expect("limiter must not fail"),
                "each 16 KiB memory is inside the 64 KiB total"
            );
        }
        assert!(
            !limits
                .memory_growing(0, 16 * 1024, None)
                .expect("limiter must not fail"),
            "a fifth 16 KiB memory must be refused: the budget is store-wide"
        );
    }

    #[test]
    fn a_failed_growth_returns_its_reservation() {
        let mut limits = limits(64 * 1024);
        assert!(limits
            .memory_growing(0, 32 * 1024, None)
            .expect("limiter must not fail"));
        limits
            .memory_grow_failed(wasmtime::Error::msg("host allocation failed"))
            .expect("reclaiming must succeed");
        assert!(
            limits
                .memory_growing(0, 64 * 1024, None)
                .expect("limiter must not fail"),
            "the budget freed by the failed growth must be reusable"
        );
    }

    #[test]
    fn memory_growth_respects_the_declared_maximum() {
        let mut limits = limits(64 * 1024);
        assert!(!limits
            .memory_growing(0, 32 * 1024, Some(16 * 1024))
            .expect("limiter must not fail"));
    }

    #[test]
    fn table_budget_is_shared_across_tables() {
        let mut limits = limits(64 * 1024);
        let half = 64 * 1024 / size_of::<usize>() / 2;
        for _ in 0..2 {
            assert!(
                limits
                    .table_growing(0, half, None)
                    .expect("limiter must not fail"),
                "each half budget table is inside the total"
            );
        }
        assert!(
            !limits
                .table_growing(0, 1, None)
                .expect("limiter must not fail"),
            "a table that would cross the total must be refused"
        );
    }

    #[test]
    fn table_growth_accounts_only_the_added_elements() {
        let mut limits = limits(64 * 1024);
        assert!(limits
            .table_growing(0, 1024, None)
            .expect("limiter must not fail"));
        assert!(
            limits
                .table_growing(1024, 2048, None)
                .expect("limiter must not fail"),
            "growing by 1,024 elements charges 1,024 elements, not 2,048"
        );
    }

    #[test]
    fn counts_are_bounded() {
        let limits = limits(64 * 1024);
        assert_eq!(limits.memories(), MAX_GUEST_MEMORIES);
        assert_eq!(limits.tables(), MAX_GUEST_TABLES);
        assert_eq!(limits.instances(), MAX_GUEST_INSTANCES);
    }
}
