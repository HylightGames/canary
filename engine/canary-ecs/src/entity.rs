// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

/// A generational entity identifier.
///
/// Two `Entity` values are equal only if both `index` and `generation`
/// match, so a stale handle to a despawned (and possibly index-recycled)
/// entity is detectable rather than silently aliasing a new one. See
/// `docs/architecture/core-runtime.md#ecs-architecture`.
///
/// `generation` is a `u64`, not a `u32`, on purpose: a `u32` generation
/// wrapping on a single, very frequently recycled slot (a projectile
/// spawn point over years of a long-lived server's uptime, say) was a
/// real, if low-probability, correctness risk this project's own
/// architecture review flagged (`docs/reviews/2026-08-senior-architecture-review.md`,
/// Finding 4.2) — a wrapped generation could, in principle, collide with
/// an old handle's generation for the same slot, silently aliasing two
/// different entities. `u64` doesn't eliminate that possibility in
/// theory, but it moves it from "plausible over a long-lived server's
/// real uptime" to "not reachable by any realistic amount of runtime,"
/// which is the standard this project holds correctness properties to.
/// `index` stays `u32`: it's bounded by concurrent entity count, which
/// doesn't have the same "recycled billions of times" exposure that made
/// `generation` worth widening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    pub(crate) index: u32,
    pub(crate) generation: u64,
}

impl Entity {
    /// The slot index this entity occupies. Exposed for debugging,
    /// serialization, and tests — not meant to be treated as a stable
    /// identifier on its own (slots are recycled; [`Entity::generation`]
    /// is what makes a full `Entity` value unambiguous).
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The generation of the slot this entity occupied at spawn time. See
    /// the type-level docs above.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl std::fmt::Display for Entity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Entity({}v{})", self.index, self.generation)
    }
}
