// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Entity-level replication opt-in (ADR 0027, point 3; `v0.0.15` WP2).
//!
//! [`Replicated`] is a zero-sized marker component: an entity carrying it
//! is a replication candidate whose opted-in components the server may
//! publish to clients. It composes with — and does not replace — the
//! per-schema opt-in registry in `canary-net` (`ReplicationRegistry`,
//! keyed by stable schema id): a component replicates for an entity only
//! when the entity carries [`Replicated`] *and* the component's schema is
//! registered there. Entity-level ("which entities") times type-level
//! ("which components") keeps a player-local UI widget or a predicted
//! client replica out of the replicated set without inventing a parallel
//! dirty-flag system — mutation tracking stays
//! [`World::query_changed_since`](crate::World::query_changed_since)
//! (ADR 0014), consulted alongside the durable removal/destruction log in
//! `canary-net`.
//!
//! Inserting or removing the marker moves the entity across archetypes like
//! any other component add/remove, and preserves the existing change ticks
//! of the entity's other components.

/// Zero-sized marker component opting an entity into replication.
///
/// Insert it on entities the server may publish (e.g. shared gameplay
/// state); leave it off entities that must stay local (presentation-only
/// widgets, client prediction scratch). This marks *candidacy*, not a
/// transport promise: the server still filters by its per-schema registry,
/// change detection still comes from
/// [`World::query_changed_since`](crate::World::query_changed_since), and
/// removals/destructions still travel through the `canary-net` tombstone
/// log rather than this marker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Replicated;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;

    #[test]
    fn replicated_marker_can_be_inserted_queried_and_removed() {
        let mut world = World::new();
        let entity = world.spawn();
        assert!(world.query::<Replicated>().next().is_none());
        world.insert(entity, Replicated).expect("insert marker");
        assert_eq!(world.query::<Replicated>().count(), 1);
        let removed: Option<Replicated> = world.remove(entity);
        assert_eq!(removed, Some(Replicated));
        assert!(world.query::<Replicated>().next().is_none());
    }

    #[test]
    fn marker_insert_preserves_sibling_change_ticks() {
        #[derive(Debug, PartialEq)]
        struct Health(u32);
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Health(10)).expect("insert health");
        world.advance_tick();
        let written_at = world.change_tick();
        world.advance_tick();
        // Opting the entity into replication must not look like a write
        // to its other components.
        world.insert(entity, Replicated).expect("insert marker");
        assert!(world
            .query_changed_since::<Health>(written_at)
            .next()
            .is_none());
    }
}
