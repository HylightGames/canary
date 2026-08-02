/// A generational entity identifier.
///
/// Two `Entity` values are equal only if both `index` and `generation`
/// match, so a stale handle to a despawned (and possibly index-recycled)
/// entity is detectable rather than silently aliasing a new one. See
/// `docs/architecture/core-runtime.md#ecs-architecture`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    pub(crate) index: u32,
    pub(crate) generation: u32,
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
    pub fn generation(&self) -> u32 {
        self.generation
    }
}

impl std::fmt::Display for Entity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Entity({}v{})", self.index, self.generation)
    }
}
