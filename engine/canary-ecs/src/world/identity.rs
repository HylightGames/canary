// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use super::World;
use crate::component_identity::CanaryComponent;
use crate::error::EcsError;
use std::any::TypeId;

impl World {
    /// Registers `T`'s stable [`CanaryComponent::SCHEMA_ID`] against its
    /// host-internal `TypeId`, so the identity can later be resolved
    /// back via [`World::type_id_for_schema`] -- the "registry mapping
    /// [the stable] identity to the host's `TypeId` at runtime for the
    /// fast path" described in ADR 0010 (see the type-level docs on
    /// [`World`]). Idempotent when called more than once for the same
    /// `T`.
    ///
    /// A component only needs this if it might cross the plugin,
    /// replication, or marketplace boundary; purely internal components
    /// can skip it and just use `T: Send + Sync + 'static` with
    /// [`World::insert`]/[`World::get`]/[`World::query`] as normal.
    ///
    /// Returns [`EcsError::DuplicateSchemaId`] if a *different* type is
    /// already registered under `T::SCHEMA_ID` -- two component types
    /// racing for the same stable identity is exactly the
    /// cross-language collision this registry exists to catch early.
    pub fn register_component<T: CanaryComponent>(&mut self) -> Result<(), EcsError> {
        let type_id = TypeId::of::<T>();
        match self.schema_registry.get(T::SCHEMA_ID) {
            Some(&existing) if existing != type_id => {
                Err(EcsError::DuplicateSchemaId(T::SCHEMA_ID))
            }
            Some(_) => Ok(()),
            None => {
                self.schema_registry.insert(T::SCHEMA_ID, type_id);
                Ok(())
            }
        }
    }

    /// Resolves a stable [`CanaryComponent::SCHEMA_ID`] back to the
    /// host-internal `TypeId` it was [`World::register_component`]ed
    /// under, if any. This is the boundary a Tier A (WASM) plugin
    /// loader, a replication decoder, or a marketplace tool crosses
    /// through in the target design -- see the type-level docs on
    /// [`World`] and ADR 0010. As of `v0.0.3`, this is exactly what the
    /// real Tier A loader uses: `engine/canary-plugin-api/src/tier_a.rs`'s
    /// `HostState::get`/`set` resolve a WASM guest's `schema-id` string
    /// through this method before touching ECS storage. Replication and
    /// marketplace tooling remain future consumers of the same seam.
    pub fn type_id_for_schema(&self, schema_id: &str) -> Option<TypeId> {
        self.schema_registry.get(schema_id).copied()
    }
}
