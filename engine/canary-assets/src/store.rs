// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use crate::AssetHandle;

/// One generational slot inside an [`AssetStore`].
///
/// `generation` counts how many times this slot has been recycled;
/// `value` is `Some` exactly while the slot is occupied. Keeping the
/// generation on the slot (rather than in a side table) means a stale
/// handle check is one indexed read plus one integer comparison — no
/// second lookup, no hashing, no allocation on the read path.
#[derive(Debug)]
struct Slot<T> {
    generation: u64,
    value: Option<T>,
}

/// A generational container for assets of one type `T`, addressed by
/// [`AssetHandle<T>`].
///
/// The store is deliberately [`canary_ecs::World`]-free: it knows
/// nothing about entities, ticks, or systems, and takes no `World`
/// parameter on any method. It *lives* as an ECS resource (inserted
/// via `World::insert_resource`, read via `World::resource` — the typed
/// global-per-type slot documented in
/// `docs/architecture/execution-model.md#resources`), but that
/// composition happens upward, at the call site, per this workspace's
/// dependency-direction rule (leaves know nothing about what is
/// composed above them). The payoff is direct: every invariant here is
/// unit-testable without constructing a `World`, and the one
/// integration touchpoint — "the store works as a resource" — is a
/// single test, not a design dependency.
///
/// Stale handles resolve to `None`, never panic, for the same reason
/// `World::resource` returns `None` for a missing resource instead of
/// panicking: asset code runs against content and handles that drift
/// (a file deleted between listing and loading, a handle held across a
/// removal), and drift is a normal condition to report, not a violated
/// invariant to trap on. Callers that must distinguish staleness from
/// other failures use [`crate::AssetError::UnknownHandle`]; the store
/// itself stays total and infallible on reads.
///
/// Slot recycling reuses indices via a free list with a bumped
/// generation, so a new asset never aliases a removed one under an old
/// handle. `generation` is `u64` for the same reason `Entity`'s is
/// (see `engine/canary-ecs/src/entity.rs`): a recycled-billions-of-
/// times wraparound must stay outside any realistic runtime, not
/// merely unlikely.
#[derive(Debug, Default)]
pub struct AssetStore<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> AssetStore<T> {
    /// Creates an empty store holding no assets.
    ///
    /// Empty rather than pre-sized: asset counts are workload-driven
    /// (a handful of fixtures in tests, hundreds of files in a game),
    /// so the store grows on demand instead of guessing a capacity
    /// that is wrong in both directions.
    pub fn new() -> Self {
        AssetStore {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Inserts `asset`, returning the live handle for it.
    ///
    /// Recycled slots are preferred over growing: the free list pops
    /// the most recently freed index, keeping the slot vector dense
    /// and reusing warm cache lines rather than extending the
    /// allocation on every insert-remove cycle. The reused slot keeps
    /// its bumped generation (see [`AssetStore::remove`]), so the new
    /// handle is unequal to every handle the slot previously issued.
    pub fn insert(&mut self, asset: T) -> AssetHandle<T> {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(
                slot.value.is_none(),
                "free-list index must point at an empty slot"
            );
            slot.value = Some(asset);
            AssetHandle::from_raw_parts(index, slot.generation)
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 0,
                value: Some(asset),
            });
            AssetHandle::from_raw_parts(index, 0)
        }
    }

    /// Returns the asset for `handle`, or `None` if the handle is
    /// stale (removed, recycled, out of bounds, or never issued by
    /// this store).
    ///
    /// Returns `None` rather than panicking or erroring because
    /// staleness is routine drift (see the type-level docs): hot paths
    /// (extract systems resolving handles per frame) need a cheap
    /// liveness check, not an error to construct and discard. The
    /// check itself is total — every mismatch class (bad index, wrong
    /// generation, empty slot) funnels to the same `None`, so no
    /// caller can observe *which* way a handle died and branch on it.
    pub fn get(&self, handle: AssetHandle<T>) -> Option<&T> {
        let slot = self.slots.get(handle.index() as usize)?;
        if slot.generation != handle.generation() {
            return None;
        }
        slot.value.as_ref()
    }

    /// Whether `handle` is currently live in this store.
    ///
    /// Purely additive readiness seam (research C3): exactly
    /// `self.get(handle).is_some()`, for callers that need liveness
    /// without borrowing the asset (readiness checks, skip decisions
    /// before a frame). No behavior change anywhere: every existing
    /// read path keeps calling [`AssetStore::get`] directly.
    pub fn contains(&self, handle: AssetHandle<T>) -> bool {
        self.get(handle).is_some()
    }

    /// Removes the asset for `handle`, returning it if the handle was
    /// live, or `None` if it was already stale.
    ///
    /// Removal bumps the slot's generation and returns the index to
    /// the free list, which is what makes every outstanding handle to
    /// the old asset stale atomically: no per-handle bookkeeping, no
    /// tombstone sweep, just one integer the old handles no longer
    /// match. Double-removal returns `None` rather than panicking —
    /// removal races (two systems dropping the same asset) are the
    /// same routine drift as stale reads.
    pub fn remove(&mut self, handle: AssetHandle<T>) -> Option<T> {
        let slot = self.slots.get_mut(handle.index() as usize)?;
        if slot.generation != handle.generation() {
            return None;
        }
        let value = slot.value.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(handle.index());
        Some(value)
    }

    /// The number of live assets currently in the store.
    ///
    /// Counts occupied slots, not allocated ones: freed slots awaiting
    /// reuse are not assets, and reporting them would mislead budget
    /// checks into counting ghosts.
    pub fn len(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.value.is_some())
            .count()
    }

    /// Whether the store holds no live assets.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_then_get_returns_the_asset() {
        let mut store = AssetStore::new();
        let handle = store.insert(String::from("quad"));
        assert_eq!(store.get(handle), Some(&String::from("quad")));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn get_after_remove_returns_none() {
        let mut store = AssetStore::new();
        let handle = store.insert(42u32);
        let removed = store.remove(handle);
        assert_eq!(removed, Some(42u32), "live removal must return the asset");
        assert_eq!(
            store.get(handle),
            None,
            "a removed handle must go stale, not keep resolving"
        );
        assert!(store.is_empty());
    }

    #[test]
    fn recycled_slot_does_not_alias_the_stale_handle() {
        let mut store = AssetStore::new();
        let stale = store.insert(String::from("first"));
        store.remove(stale);
        let live = store.insert(String::from("second"));
        assert_eq!(
            store.get(stale),
            None,
            "negative control: the old handle must not see the recycled slot's new asset"
        );
        assert_eq!(
            store.get(live),
            Some(&String::from("second")),
            "the recycled slot's new handle must resolve"
        );
    }

    #[test]
    fn invented_handles_resolve_to_none() {
        let store = AssetStore::<String>::new();
        let invented = AssetHandle::from_raw_parts(99, 0);
        assert_eq!(
            store.get(invented),
            None,
            "a handle this store never issued must not resolve"
        );
    }

    #[test]
    fn contains_mirrors_get_liveness_exactly() {
        let mut store = AssetStore::new();
        let live = store.insert(1u32);
        let invented = AssetHandle::from_raw_parts(99, 0);
        assert!(store.contains(live), "a live handle must read ready");
        assert!(
            !store.contains(invented),
            "a never-issued handle must read not-ready"
        );
        store.remove(live);
        assert!(
            !store.contains(live),
            "a removed handle must read not-ready, matching get returning None"
        );
        assert_eq!(
            store.contains(live),
            store.get(live).is_some(),
            "contains must never disagree with get"
        );
    }

    #[test]
    fn double_remove_returns_none_the_second_time() {
        let mut store = AssetStore::new();
        let handle = store.insert(7u32);
        assert_eq!(store.remove(handle), Some(7u32));
        assert_eq!(
            store.remove(handle),
            None,
            "second removal of the same handle must be a quiet None, not a panic"
        );
    }

    #[test]
    fn store_lives_as_an_ecs_resource() {
        let mut world = canary_ecs::World::new();
        let mut store = AssetStore::new();
        let handle = store.insert(String::from("resident"));
        world.insert_resource(store);
        let stored = world
            .resource::<AssetStore<String>>()
            .expect("store must be retrievable as a resource");
        assert_eq!(stored.get(handle), Some(&String::from("resident")));
    }

    #[test]
    fn remove_with_wrong_generation_leaves_the_live_asset_untouched() {
        let mut store = AssetStore::new();
        let live = store.insert(11u32);
        let forged = AssetHandle::from_raw_parts(live.index(), live.generation() + 1);
        assert_eq!(
            store.remove(forged),
            None,
            "a forged generation must not remove the live asset"
        );
        assert_eq!(
            store.get(live),
            Some(&11u32),
            "the live asset must survive a forged-generation removal"
        );
        assert_eq!(
            store.get(forged),
            None,
            "the forged handle itself must never resolve"
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn invented_generation_on_a_live_index_resolves_to_none() {
        let mut store = AssetStore::new();
        let live = store.insert(22u32);
        let invented = AssetHandle::from_raw_parts(live.index(), live.generation() + 7);
        assert_eq!(
            store.get(invented),
            None,
            "a never-issued generation on a live index must not see the asset"
        );
        assert!(
            !store.contains(invented),
            "contains must agree with get on the invented generation"
        );
        assert_eq!(
            store.get(live),
            Some(&22u32),
            "the live handle must keep resolving alongside the invented one"
        );
    }

    #[test]
    fn len_counts_live_assets_across_recycle_not_slots() {
        let mut store = AssetStore::new();
        let first = store.insert(1u32);
        let second = store.insert(2u32);
        assert_eq!(store.len(), 2);
        store.remove(first);
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        let third = store.insert(3u32);
        assert_eq!(store.len(), 2, "a recycled slot holds a live asset again");
        assert_ne!(
            first, third,
            "the recycled handle must differ from the stale one by generation"
        );
        assert_eq!(
            first.index(),
            third.index(),
            "recycle must reuse the freed slot"
        );
        assert_eq!(store.get(first), None);
        assert_eq!(store.get(second), Some(&2u32));
        assert_eq!(store.get(third), Some(&3u32));
    }

    /// One randomized operation against the store, targeting an issued
    /// handle by index into the test's own ledger (out-of-range targets
    /// are no-ops, so in-range handles stay hot).
    #[derive(Debug, Clone)]
    enum StoreOp {
        Insert(u8),
        Remove(usize),
        Check(usize),
        /// Remove through a handle whose generation is bumped by one:
        /// must be a quiet None that leaves the live asset untouched —
        /// unless the slot recycled exactly once since issue, in which
        /// case the "forged" handle is genuinely live and the op is
        /// skipped (see `assert_forged_is_stale`).
        RemoveForged(usize),
        /// Liveness check through a bumped-generation handle: must read
        /// not-ready under the same skip condition.
        CheckForged(usize),
    }

    fn store_op_strategy() -> impl proptest::strategy::Strategy<Value = StoreOp> {
        use proptest::prelude::*;
        prop_oneof![
            (0u8..16).prop_map(StoreOp::Insert),
            (0usize..8).prop_map(StoreOp::Remove),
            (0usize..8).prop_map(StoreOp::Check),
            (0usize..8).prop_map(StoreOp::RemoveForged),
            (0usize..8).prop_map(StoreOp::CheckForged),
        ]
    }

    /// The forged-generation oracle: a handle with a bumped generation
    /// must resolve to None and remove to None without disturbing the
    /// ledger — total, like every other stale-handle path.
    ///
    /// Returns `Ok(true)` when the assertion ran, `Ok(false)` when the
    /// op was correctly skipped: if the slot recycled exactly once
    /// since the targeted handle was issued, the bumped generation
    /// equals a genuinely live handle, so there is nothing forged to
    /// assert about (asserting None there would pin a wrong oracle).
    fn assert_forged_is_stale(
        store: &mut AssetStore<u32>,
        issued: &[(AssetHandle<u32>, Option<u32>)],
        target: usize,
        do_remove: bool,
    ) -> Result<bool, proptest::test_runner::TestCaseError> {
        let slot = target % issued.len();
        let (handle, expected) = issued[slot];
        let forged_gen = handle.generation().wrapping_add(1);
        let collides = issued.iter().any(|(other, live)| {
            live.is_some() && other.index() == handle.index() && other.generation() == forged_gen
        });
        if collides {
            return Ok(false);
        }
        let forged = AssetHandle::from_raw_parts(handle.index(), forged_gen);
        proptest::prop_assert_eq!(
            store.get(forged),
            None,
            "a bumped-generation handle must never resolve"
        );
        proptest::prop_assert!(
            !store.contains(forged),
            "contains must agree with get on the forged handle"
        );
        if do_remove {
            proptest::prop_assert_eq!(
                store.remove(forged),
                None,
                "a forged-generation removal must be a quiet None"
            );
        }
        proptest::prop_assert_eq!(
            store.get(handle),
            expected.as_ref(),
            "the original handle's expectation must survive the forged op"
        );
        Ok(true)
    }

    proptest::proptest! {
        /// The generational-aliasing invariant behind `AssetHandle`'s shape
        /// (see `handle.rs`): over arbitrary insert/remove sequences, a
        /// stale handle must never resolve — even when its slot was recycled
        /// — and every live handle must keep resolving to exactly the value
        /// it was inserted with. Mirrors the ECS
        /// `despawned_entities_never_alias_a_later_spawn` proptest style:
        /// a model ledger tracks expected liveness, the store is the system
        /// under test, and every op asserts agreement on the spot plus a
        /// full-ledger sweep at the end.
        #[test]
        fn recycled_slots_never_alias_stale_handles(
            ops in proptest::collection::vec(store_op_strategy(), 1..80)
        ) {
            let mut store = AssetStore::new();
            let mut issued: Vec<(AssetHandle<u32>, Option<u32>)> = Vec::new();

            for op in ops {
                match op {
                    StoreOp::Insert(value) => {
                        let handle = store.insert(u32::from(value));
                        for (old, live) in &issued {
                            proptest::prop_assert!(
                                live.is_none() || old.index() != handle.index(),
                                "a recycled slot must have no live handle pointing at it"
                            );
                        }
                        issued.push((handle, Some(u32::from(value))));
                    }
                    StoreOp::Remove(target) => {
                        if issued.is_empty() {
                            continue;
                        }
                        let slot = target % issued.len();
                        let (handle, expected) = issued[slot];
                        let removed = store.remove(handle);
                        proptest::prop_assert_eq!(
                            removed, expected,
                            "live removal must return the inserted value; stale removal must be None"
                        );
                        if expected.is_some() {
                            issued[slot].1 = None;
                        }
                    }
                    StoreOp::Check(target) => {
                        if issued.is_empty() {
                            continue;
                        }
                        let (handle, expected) = issued[target % issued.len()];
                        proptest::prop_assert_eq!(
                            store.get(handle), expected.as_ref(),
                            "stale handles resolve to None, live ones to their value"
                        );
                        proptest::prop_assert_eq!(
                            store.contains(handle), expected.is_some(),
                            "contains must never disagree with get"
                        );
                    }
                    StoreOp::RemoveForged(target) => {
                        if issued.is_empty() {
                            continue;
                        }
                        assert_forged_is_stale(&mut store, &issued, target, true)?;
                    }
                    StoreOp::CheckForged(target) => {
                        if issued.is_empty() {
                            continue;
                        }
                        assert_forged_is_stale(&mut store, &issued, target, false)?;
                    }
                }
            }

            let mut live_count = 0usize;
            for (handle, expected) in &issued {
                proptest::prop_assert_eq!(
                    store.get(*handle), expected.as_ref(),
                    "final sweep: every issued handle must agree with the ledger"
                );
                if expected.is_some() {
                    live_count += 1;
                }
            }
            proptest::prop_assert_eq!(
                store.len(), live_count,
                "len must count exactly the live assets, not slots"
            );
            proptest::prop_assert_eq!(
                store.is_empty(), live_count == 0,
                "is_empty must agree with len"
            );
            let mut live_indices: Vec<u32> =
                issued.iter().filter_map(|(h, v)| v.map(|_| h.index())).collect();
            live_indices.sort_unstable();
            live_indices.dedup();
            proptest::prop_assert_eq!(
                live_indices.len(), live_count,
                "no two live handles may share a slot index"
            );
        }
    }

    #[test]
    fn ten_thousand_insert_remove_cycles_advance_generation_without_aliasing() {
        // Given: one slot churned hard — remove + reinsert 10k times.
        let mut store = AssetStore::new();
        let first = store.insert(0u32);
        let mut live = first;
        for cycle in 1..=10_000u32 {
            // When: the live asset is removed and a new one takes its slot.
            let removed = store.remove(live);
            assert_eq!(
                removed,
                Some(cycle - 1),
                "cycle {cycle}: live removal must return the inserted value"
            );
            live = store.insert(cycle);
            // Then: the same slot is reused with a bumped generation, the
            // very first handle stays stale, and exactly one asset is live.
            assert_eq!(
                live.index(),
                first.index(),
                "cycle {cycle}: one slot must be reused, not grown"
            );
            assert_eq!(
                live.generation(),
                u64::from(cycle),
                "cycle {cycle}: each recycle must bump the generation exactly once"
            );
            assert_eq!(
                store.get(first),
                None,
                "cycle {cycle}: the first handle must never re-resolve"
            );
            assert_eq!(store.get(live), Some(&cycle));
            assert_eq!(store.len(), 1);
        }
        assert_eq!(store.get(live), Some(&10_000u32));
        assert!(store.contains(live));
    }
}
