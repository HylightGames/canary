// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

/// A resource budget applied to every Tier A instance a given
/// [`crate::WasmPluginLoader`] loads — a sandboxing property genuinely
/// separate from [`crate::Capability`]-based authority: a component with
/// *zero* granted capabilities can still attempt to exhaust memory or
/// spin the CPU forever, and "sandboxed" here means both "can't reach
/// what it wasn't granted" (capabilities) *and* "can't consume
/// unbounded host resources" (this). See
/// `docs/roadmap/v0.0.3-roadmap.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudget {
    /// Maximum bytes of linear memory and table storage **in total**
    /// that one instance's store may create and grow. Every linear
    /// memory and every table the guest allocates draws from this one
    /// ledger (see `tier_a::limits::GuestLimits`), so four memories
    /// cannot each consume the full figure. A `memory.grow` past it
    /// fails (returns `-1` to the guest, per core WASM semantics for a
    /// failed grow) rather than trapping — the same behavior as
    /// genuinely running out of host memory, which well-behaved guest
    /// code already has to handle.
    ///
    /// Was `v0.1.0`'s per-memory ceiling. The change to a store-wide
    /// total is the fix for the final review's
    /// `CANARY-REL-008` (a guest could hold "4x its memory budget in
    /// aggregate"); the default value is unchanged.
    pub max_memory_bytes: usize,
    /// Execution budget, in Wasmtime "fuel" units — consumed
    /// approximately per WASM instruction executed. Exhausting it traps
    /// the current call, which is how an infinite (or just
    /// pathologically long) loop in untrusted guest code gets bounded,
    /// without needing a separate watchdog thread.
    pub fuel: u64,
}

impl Default for ResourceBudget {
    /// 64 MiB of memory, 10,000,000 fuel units — generous enough for
    /// legitimate plugin logic in this first cut, not tuned against any
    /// real workload yet. Revisit once one exists.
    fn default() -> Self {
        Self {
            max_memory_bytes: 64 * 1024 * 1024,
            fuel: 10_000_000,
        }
    }
}
