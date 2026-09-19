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
}
