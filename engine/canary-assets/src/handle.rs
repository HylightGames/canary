// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// A generational key into an [`crate::AssetStore`] holding `T`.
///
/// The shape mirrors [`canary_ecs::Entity`](https://github.com/HylightGames/canary/blob/dev/engine/canary-ecs/src/entity.rs)
/// (`index` + `generation`) deliberately, not coincidentally: bare
/// indices alias after remove-plus-reinsert (slot 2 freed, then handed
/// to a new asset, makes an old handle to slot 2 point at the wrong
/// asset), and that aliasing class is exactly what `Entity`'s
/// generation field already solved for this workspace. Reusing the
/// proven shape means the handle/store invariant reasoning — and its
/// `proptest` style — transfers wholesale. See ADR 0018.
///
/// `T` appears only as [`PhantomData`]: a handle owns no asset data,
/// borrows nothing, and costs two words to copy. Handles are plain
/// keys; the store owns everything, so there is no refcounting (there
/// is no lifetime question yet asking for it — when eviction arrives,
/// the generation field is the seam it hangs on).
///
/// Equality, hashing, and ordering consider `index` and `generation`
/// only, never `T`: two handles to the same slot-generation are the
/// same key even across different static `T`s in generic code, and —
/// critically — the manual impls below carry no `T: PartialEq`-style
/// bounds. A `#[derive]` would add those bounds implicitly (the
/// manual-before-derive precedent from ADR 0010's resolution), making
/// e.g. `Option<AssetHandle<NotEq>>` unusable for no semantic reason.
pub struct AssetHandle<T> {
    index: u32,
    generation: u64,
    marker: PhantomData<T>,
}

// Manual impls, all intentionally bound-free over `T` (see type docs).
// `Copy` is sound: the handle is two integers plus a zero-sized marker.
// `Debug` is manual for the same reason as the rest: a `#[derive(Debug)]`
// would demand `T: Debug`, making handles to non-`Debug` assets
// unprintable despite identity never involving `T`.

impl<T> fmt::Debug for AssetHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetHandle({}v{})", self.index, self.generation)
    }
}

impl<T> Clone for AssetHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for AssetHandle<T> {}

impl<T> PartialEq for AssetHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}

impl<T> Eq for AssetHandle<T> {}

impl<T> Hash for AssetHandle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}

impl<T> AssetHandle<T> {
    /// The slot index this handle points at. Exposed for debugging and
    /// for boundaries that must serialize a handle to raw parts — never
    /// a stable identifier on its own, because slots are recycled and
    /// only [`AssetHandle::generation`] disambiguates reuses.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The generation of the slot this handle was issued for. A handle
    /// is live only while the store's slot still carries this exact
    /// generation; any mismatch (removed, or recycled for a new asset)
    /// makes the handle stale, and stale handles resolve to `None`,
    /// never to another asset's data.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Reconstructs a handle from raw `index` then `generation` parts,
    /// matching [`AssetHandle::index`]/[`AssetHandle::generation`]'s
    /// order.
    ///
    /// Exists for boundaries that cannot pass an opaque handle through
    /// directly (serialization, FFI-adjacent records) and must rebuild
    /// one from bits. Like `Entity::from_raw_parts`, this verifies
    /// nothing: feeding the result to [`crate::AssetStore::get`] is
    /// exactly as safe as passing a genuinely stale handle, because
    /// the store checks liveness itself and returns `None`. Prefer a
    /// handle from [`crate::AssetStore::insert`] wherever one is
    /// available.
    pub fn from_raw_parts(index: u32, generation: u64) -> Self {
        AssetHandle {
            index,
            generation,
            marker: PhantomData,
        }
    }

    /// Reinterprets this handle's slot identity as a handle for a
    /// different asset type `U`.
    ///
    /// Sound only when the caller knows the slot actually holds `U`
    /// (e.g. a loader that inserted untyped bytes and now completes
    /// them as a parsed type). The index and generation pass through
    /// unchanged — identity is type-independent by construction (see
    /// the `PartialEq` impl), so this converts the key without
    /// touching what it points at.
    ///
    /// # Deprecation: cross-type aliasing is provable, not theoretical
    ///
    /// Stores are per-type (`AssetStore<Mesh>` and `AssetStore<Texture>`
    /// allocate slot indices independently, both starting at 0), so a
    /// reinterpreted handle can resolve to a *live, unrelated* asset in
    /// the other store: insert one mesh and one texture, and
    /// `mesh_handle.with_type::<Texture>()` resolves to that texture.
    /// The `cross_type_reinterpretation_can_alias_a_live_asset` test
    /// proves it. Nothing checks that the slot "actually holds `U`" —
    /// the soundness condition above is unenforceable at the call site.
    /// Prefer inserting into the correctly typed store and threading
    /// the resulting handle through; a typed removal path is Phase 11's
    /// disposition for this method.
    #[deprecated(
        since = "0.0.2",
        note = "cross-type reinterpretation can alias a live asset in the other store; \
                insert into the correctly typed store instead"
    )]
    pub fn with_type<U>(&self) -> AssetHandle<U> {
        AssetHandle::from_raw_parts(self.index, self.generation)
    }
}

impl<T> fmt::Display for AssetHandle<T> {
    /// Renders as `Asset(index v generation)`, mirroring `Entity`'s
    /// `Display` so logs from the two systems read uniformly.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Asset({}v{})", self.index, self.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mesh;
    struct Texture;

    #[test]
    fn handles_compare_by_index_and_generation() {
        let first = AssetHandle::<Mesh>::from_raw_parts(2, 0);
        let same = AssetHandle::<Mesh>::from_raw_parts(2, 0);
        let newer_generation = AssetHandle::<Mesh>::from_raw_parts(2, 1);
        let other_slot = AssetHandle::<Mesh>::from_raw_parts(3, 0);
        assert_eq!(first, same);
        assert_ne!(
            first, newer_generation,
            "generation bump must break equality"
        );
        assert_ne!(first, other_slot, "different slots must not compare equal");
    }

    #[test]
    fn bare_index_reuse_does_not_alias_across_generations() {
        let stale = AssetHandle::<Mesh>::from_raw_parts(0, 0);
        let recycled = AssetHandle::<Mesh>::from_raw_parts(0, 1);
        assert_ne!(
            stale, recycled,
            "the negative control for slot recycling: a stale handle must never equal the recycled one"
        );
    }

    #[test]
    fn handles_are_copy_and_hash_by_identity() {
        use std::collections::HashSet;
        let handle = AssetHandle::<Mesh>::from_raw_parts(1, 4);
        let copy = handle;
        assert_eq!(handle, copy, "handles must be Copy, like Entity");
        let mut set = HashSet::new();
        set.insert(handle);
        assert!(set.contains(&copy));
        assert!(!set.contains(&AssetHandle::<Mesh>::from_raw_parts(1, 5)));
    }

    #[test]
    fn type_reinterpretation_preserves_slot_identity() {
        let mesh = AssetHandle::<Mesh>::from_raw_parts(7, 2);
        #[allow(deprecated)]
        let texture: AssetHandle<Texture> = mesh.with_type();
        assert_eq!(texture.index(), 7);
        assert_eq!(texture.generation(), 2);
        assert_eq!(mesh.to_string(), "Asset(7v2)");
    }

    #[test]
    fn cross_type_reinterpretation_can_alias_a_live_asset() {
        // Given: one mesh and one texture in their own stores. Both
        // stores allocate slot indices independently from zero, so both
        // live assets sit at index 0, generation 0.
        let mut meshes = crate::AssetStore::new();
        let mesh_handle = meshes.insert(Mesh);
        let mut textures = crate::AssetStore::new();
        let texture_handle = textures.insert(Texture);
        assert_eq!((mesh_handle.index(), mesh_handle.generation()), (0, 0));
        assert_eq!(
            (texture_handle.index(), texture_handle.generation()),
            (0, 0)
        );

        // When: the mesh handle is reinterpreted as a texture handle.
        #[allow(deprecated)]
        let aliased: AssetHandle<Texture> = mesh_handle.with_type();

        // Then: it resolves — to the live, unrelated texture, not to
        // nothing. This is the C2 hazard made observable: the
        // "caller knows the slot holds U" soundness condition is
        // unenforceable, and misuse silently yields the wrong asset
        // rather than a stale handle. Pinned here so Phase 11's
        // disposition (constrain or remove) must move this test first.
        // (Compared by handle parts plus liveness: the test asset types
        // deliberately carry no Debug/PartialEq bounds, mirroring the
        // manual bound-free impls above.)
        assert_eq!(
            (aliased.index(), aliased.generation()),
            (texture_handle.index(), texture_handle.generation()),
            "the reinterpreted handle is slot-identical to the live texture handle"
        );
        assert!(
            textures.get(aliased).is_some(),
            "and it resolves to a live asset — the wrong one — not to a stale None"
        );
    }
}
