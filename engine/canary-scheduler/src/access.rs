// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::any::TypeId;
use std::collections::HashSet;

/// What a system reads and writes -- component types and resource types
/// tracked separately, since a system reading resource `Foo` and a
/// system reading/writing component `Foo` don't actually contend for
/// the same storage even if `Foo` the type happens to be shared (an
/// unusual case in practice, but not one this type should get wrong).
/// This is the literal "declares what it reads and writes *before* it
/// runs" from `docs/architecture/execution-model.md`'s Access
/// invariant -- [`Schedule`](crate::Schedule) uses exactly this
/// declaration, and nothing else, to decide what can run concurrently.
///
/// Built with the same "small, chainable builder" shape as
/// `canary_ecs`'s own APIs rather than a derive macro inferring access
/// from a system function's signature -- inferring access
/// automatically (the way a fuller `SystemParam`-style framework would)
/// is real, useful future work, but doing it correctly for arbitrary
/// query shapes is a substantially bigger undertaking than this crate's
/// first release; declaring it explicitly is honest about that, not a
/// placeholder for it.
#[derive(Debug, Default, Clone)]
pub struct SystemAccess {
    component_reads: HashSet<TypeId>,
    component_writes: HashSet<TypeId>,
    resource_reads: HashSet<TypeId>,
    resource_writes: HashSet<TypeId>,
}

impl SystemAccess {
    /// An access declaration for a system that touches nothing yet --
    /// chain `.reads::<T>()`/`.writes::<T>()`/`.reads_resource::<T>()`/
    /// `.writes_resource::<T>()` to build up the real declaration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares that this system reads component type `T`.
    pub fn reads<T: 'static>(mut self) -> Self {
        self.component_reads.insert(TypeId::of::<T>());
        self
    }

    /// Declares that this system writes component type `T`.
    pub fn writes<T: 'static>(mut self) -> Self {
        self.component_writes.insert(TypeId::of::<T>());
        self
    }

    /// Declares that this system reads resource type `T`.
    pub fn reads_resource<T: 'static>(mut self) -> Self {
        self.resource_reads.insert(TypeId::of::<T>());
        self
    }

    /// Declares that this system writes resource type `T`.
    pub fn writes_resource<T: 'static>(mut self) -> Self {
        self.resource_writes.insert(TypeId::of::<T>());
        self
    }

    /// Whether this declaration writes nothing at all -- component or
    /// resource. A read-only system can always share a
    /// [`Schedule`](crate::Schedule) stage with other read-only
    /// systems, regardless of what either one reads, since multiple
    /// shared borrows of the same data are always safe; see
    /// `docs/architecture/execution-model.md#the-scheduler` for why a
    /// system that writes *anything* never shares a stage with anything
    /// else, even another write this declaration alone can prove is
    /// disjoint.
    pub(crate) fn is_read_only(&self) -> bool {
        self.component_writes.is_empty() && self.resource_writes.is_empty()
    }

    /// Whether `self` and `other` contend for the same storage: either
    /// one writes something the other reads or writes. Two read-only
    /// declarations never conflict, regardless of overlapping reads --
    /// shared reads are always safe to run concurrently.
    pub(crate) fn conflicts_with(&self, other: &SystemAccess) -> bool {
        !self.component_writes.is_disjoint(&other.component_reads)
            || !self.component_writes.is_disjoint(&other.component_writes)
            || !self.component_reads.is_disjoint(&other.component_writes)
            || !self.resource_writes.is_disjoint(&other.resource_reads)
            || !self.resource_writes.is_disjoint(&other.resource_writes)
            || !self.resource_reads.is_disjoint(&other.resource_writes)
    }

    /// Folds `other`'s declared access into `self` -- used by
    /// [`Schedule`](crate::Schedule) to track a whole stage's cumulative
    /// access as systems are greedily added to it.
    pub(crate) fn merge(&mut self, other: &SystemAccess) {
        self.component_reads.extend(&other.component_reads);
        self.component_writes.extend(&other.component_writes);
        self.resource_reads.extend(&other.resource_reads);
        self.resource_writes.extend(&other.resource_writes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Position;
    struct Velocity;

    #[test]
    fn two_purely_read_only_declarations_never_conflict() {
        let a = SystemAccess::new().reads::<Position>().reads::<Velocity>();
        let b = SystemAccess::new().reads::<Position>();
        assert!(!a.conflicts_with(&b));
        assert!(!b.conflicts_with(&a));
    }

    #[test]
    fn a_write_conflicts_with_a_read_of_the_same_type() {
        let writer = SystemAccess::new().writes::<Position>();
        let reader = SystemAccess::new().reads::<Position>();
        assert!(writer.conflicts_with(&reader));
        assert!(reader.conflicts_with(&writer));
    }

    #[test]
    fn two_writers_of_the_same_type_conflict() {
        let a = SystemAccess::new().writes::<Position>();
        let b = SystemAccess::new().writes::<Position>();
        assert!(a.conflicts_with(&b));
    }

    #[test]
    fn disjoint_access_does_not_conflict() {
        let a = SystemAccess::new().writes::<Position>();
        let b = SystemAccess::new().writes::<Velocity>();
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn resource_access_is_tracked_separately_from_component_access() {
        // A system writing component Position and a system reading
        // resource Position (an unusual but not incoherent setup) must
        // not conflict -- they're different storage, even though the
        // type is shared.
        let component_writer = SystemAccess::new().writes::<Position>();
        let resource_reader = SystemAccess::new().reads_resource::<Position>();
        assert!(!component_writer.conflicts_with(&resource_reader));
    }

    #[test]
    fn is_read_only_is_false_if_anything_at_all_is_written() {
        assert!(SystemAccess::new().reads::<Position>().is_read_only());
        assert!(!SystemAccess::new().writes::<Position>().is_read_only());
        assert!(!SystemAccess::new()
            .writes_resource::<Position>()
            .is_read_only());
    }
}
