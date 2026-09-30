//! Server-scoped network entity identity assignment (ADR 0027, point 5).
//!
//! [`NetEntityMap`] assigns one [`NetEntityId`](crate::ids::NetEntityId)
//! per live ECS entity for an authoritative session. The map keys on the
//! full `(slot index, generation)` tuple — the same tuple-key discipline as
//! `canary-state`'s `RemapTable` — so recycling a slot for a new entity
//! (new generation) assigns a *fresh* network id and can never alias the
//! previous occupant's id.
//!
//! The map is deliberately ECS-agnostic: callers pass
//! `entity.index()` / `entity.generation()` as raw parts rather than an
//! `Entity` value, so a runtime handle can never slip across the wire
//! unnamed — every crossing names both halves explicitly. The map lives and
//! dies with its session: a reconnect starts a fresh mapping unless the
//! server proves the old baseline is still retained (ADR 0027, point 10).

use std::collections::HashMap;

use crate::ids::NetEntityId;

/// Assigns stable server-scoped network ids to live `(index, generation)`
/// entity handles for one authoritative session.
#[derive(Debug, Default)]
pub struct NetEntityMap {
    /// Every assignment ever made: `(slot index, generation)` to network id.
    /// Entries are never removed or reused — that is what makes slot
    /// recycling alias-free — and the whole map is dropped with the session.
    live_to_net: HashMap<(u32, u64), NetEntityId>,
    /// Next id to assign. Starts at 1: [`NetEntityId`] `0` is reserved as
    /// an invalid sentinel and is never assigned.
    next: u64,
}

impl NetEntityMap {
    /// An empty mapping for a new session.
    #[must_use]
    pub fn new() -> Self {
        Self {
            live_to_net: HashMap::new(),
            // `NetEntityId(0)` is the reserved invalid sentinel.
            next: 1,
        }
    }

    /// Returns the network id for `(index, generation)`, assigning a fresh
    /// one on first sight. The same handle always resolves to the same id;
    /// a recycled slot (same index, bumped generation) is a different key
    /// and gets a different id.
    ///
    /// # Panics
    ///
    /// If 2^64 ids have been assigned in one session — unreachable in any
    /// realistic workload, and aliasing two live entities would be worse
    /// than failing loudly (the same "fail visibly at the limit" precedent
    /// as `World::spawn`'s slot-index exhaustion).
    pub fn assign(&mut self, index: u32, generation: u64) -> NetEntityId {
        if let Some(id) = self.live_to_net.get(&(index, generation)) {
            return *id;
        }
        let assigned = self.next;
        self.next = self
            .next
            .checked_add(1)
            .expect("network entity id space exhausted (2^64 assignments in one session)");
        let id = NetEntityId(assigned);
        self.live_to_net.insert((index, generation), id);
        id
    }

    /// Resolves a previously assigned handle without assigning.
    #[must_use]
    pub fn resolve(&self, index: u32, generation: u64) -> Option<NetEntityId> {
        self.live_to_net.get(&(index, generation)).copied()
    }

    /// How many distinct handles have been assigned ids this session.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live_to_net.len()
    }

    /// Whether any id has been assigned yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live_to_net.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_handle_resolves_to_the_same_id() {
        let mut map = NetEntityMap::new();
        let first = map.assign(5, 0);
        assert_eq!(map.assign(5, 0), first);
        assert_eq!(map.resolve(5, 0), Some(first));
        assert_ne!(first, NetEntityId(0));
    }

    #[test]
    fn slot_recycle_assigns_a_fresh_id_without_aliasing() {
        // Despawns bump the generation while the slot index is recycled:
        // the new occupant must not inherit the old network id, and the
        // old handle must still resolve to its own (retired) id rather
        // than aliasing the newcomer.
        let mut map = NetEntityMap::new();
        let before = map.assign(5, 0);
        let after = map.assign(5, 1);
        assert_ne!(before, after);
        assert_eq!(map.resolve(5, 0), Some(before));
        assert_eq!(map.resolve(5, 1), Some(after));
    }

    #[test]
    fn unknown_handle_resolves_to_none() {
        let map = NetEntityMap::new();
        assert_eq!(map.resolve(9, 0), None);
    }

    #[test]
    fn sentinel_zero_is_never_assigned() {
        let mut map = NetEntityMap::new();
        for index in 0..256 {
            assert_ne!(map.assign(index, 0), NetEntityId(0));
        }
    }
}
