// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Per-invocation host view: the capability grant type, the loaned-world
//! slot, and the `ecs-read`/`ecs-write` host-function bodies.

use std::collections::HashSet;
use std::sync::Arc;

use canary_ecs::{Tick, World};
use wasmtime::StoreLimits;

use crate::capability::Capability;
use crate::component_value::CodecRegistry;

use super::budget::ResourceBudget;
use super::codec::canary::plugin::{ecs_read, ecs_write};
use super::codec::{
    from_wit_entity, from_wit_value, guard_host_call, to_wit_value, WitComponentValue,
    WitEntityHandle,
};

/// What one Tier A instance may do with the loaned active world.
/// Decided once at load; enforced structurally by linker wiring (an
/// ungranted interface is never linked, so the guest cannot even name
/// it), mirrored here so host functions and audits read the same
/// truth. The per-interface host bodies additionally consult this as
/// a backstop, returning their benign default when the grant does not
/// cover the interface — unreachable while linking and grants agree,
/// which [`crate::WasmComponentPlugin::call_scoped`] keeps true by storing
/// the call's grant into the slot before running guest code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedGrant {
    /// The capability subset actually linked for this instance:
    /// any subset of [`crate::Capability::ReadEcsWorld`] /
    /// [`crate::Capability::WriteEcsWorld`].
    pub capabilities: HashSet<Capability>,
    /// Fuel + memory budget, re-armed per guest entry.
    pub budget: ResourceBudget,
}

impl ScopedGrant {
    /// Whether the `ecs-read` interface was linked for this instance.
    pub fn can_read(&self) -> bool {
        self.capabilities.contains(&Capability::ReadEcsWorld)
    }

    /// Whether the `ecs-write` interface was linked for this instance.
    pub fn can_write(&self) -> bool {
        self.capabilities.contains(&Capability::WriteEcsWorld)
    }
}

/// Per-invocation host view. Owns the loaned active world for exactly
/// one guest call: `world` is `Some` only between loan and reclaim,
/// the runtime's `Option` is `None` for that window (so no schedule
/// can start — there is nothing to schedule against — and no second
/// loan can issue), and no borrow exists across `Func::call`, only
/// owned values moved in and home again.
///
/// `HostState` (owning a separate `World` by value) proved capability
/// checks against real world data; this slot replaces that
/// simplification with the ownership loan the R-34 review specifies,
/// keeping the same host-function bodies behind a panic-catching,
/// grant-checked wrapper.
pub(crate) struct WorldSlot {
    /// The loaned active world; `None` while home in the runtime or
    /// before any loan. Host functions only observe it through
    /// short-lived borrows that end before the host call returns —
    /// nothing outlives the invocation.
    pub(crate) world: Option<World>,
    /// Fixed at loader construction; never per-invocation.
    pub(crate) codecs: Arc<CodecRegistry>,
    /// The capability subset actually linked for this instance,
    /// refreshed from the call's grant on every scoped invoke.
    pub(crate) grant: ScopedGrant,
    /// Backs the memory half of [`crate::ResourceBudget`] — wired to the
    /// [`wasmtime::Store`] via [`wasmtime::Store::limiter`] in
    /// [`crate::WasmPluginLoader::instantiate`]. The fuel half doesn't need a
    /// field here; it's set directly on the `Store` via
    /// [`wasmtime::Store::set_fuel`].
    pub(crate) limits: StoreLimits,
    /// Loader-supplied name, for host-panic diagnostics only. Never
    /// exposed to the guest.
    pub(crate) plugin_name: String,
    /// Reentrancy tripwire: greater than zero while inside a host
    /// call. Host functions are synchronous and never re-enter the
    /// guest, so this returns to zero before every `Func::call`
    /// returns; [`crate::WasmComponentPlugin::call_scoped`] debug-asserts
    /// that on entry. Nested loans are refused by the `Option`
    /// ownership checks, not by this counter — it exists so a future
    /// reentrant path fails loudly in debug builds instead of
    /// silently.
    pub(crate) depth: u32,
    /// Tick captured at loan time; debug-asserted equal after reclaim
    /// (plugin phases never advance the ECS tick — writes stamp
    /// component ticks, the counter must not move).
    pub(crate) tick_before: Option<Tick>,
    /// A caught host panic for the current invocation, converted to a
    /// [`crate::PluginError::GuestTrap`] once the guest call settles. Later
    /// host calls in the same invocation short-circuit to their
    /// benign default once this is set.
    pub(crate) pending_trap: Option<String>,
}

impl WorldSlot {
    /// Runs one host-function body under the Decision 2 policy: a
    /// caught panic becomes [`Self::pending_trap`] (surfaced as a
    /// typed trap by the scoped entry point) instead of unwinding
    /// through Wasmtime frames, and the benign `default` reaches the
    /// guest for the remainder of this invocation.
    fn invoke_guarded<T>(
        &mut self,
        op: &'static str,
        default: T,
        body: impl FnOnce(&mut Self) -> T,
    ) -> T {
        if self.pending_trap.is_some() {
            return default;
        }
        // Host calls are not engine hot paths (one WASM-boundary
        // crossing each); the saturating arithmetic documents that a
        // wrap here is unreachable without deep reentrancy, which the
        // depth tripwire exists to catch in debug builds.
        self.depth = self.depth.saturating_add(1);
        // Cloned so the `&str` handed to `guard_host_call` does not
        // borrow `self` while the guarded closure mutably borrows it.
        let plugin = self.plugin_name.clone();
        let outcome = guard_host_call(&plugin, op, std::panic::AssertUnwindSafe(|| body(self)));
        self.depth = self.depth.saturating_sub(1);
        match outcome {
            Ok(value) => value,
            Err(error) => {
                self.pending_trap = Some(error.to_string());
                default
            }
        }
    }
}

impl ecs_read::Host for WorldSlot {
    fn entity_count(&mut self) -> u32 {
        self.invoke_guarded("ecs-read.entity-count", 0, |slot| {
            if !slot.grant.can_read() {
                return 0;
            }
            let Some(world) = slot.world.as_ref() else {
                return 0;
            };
            // ECS entity counts never approach `u32::MAX` in practice;
            // saturate rather than wrap or panic on the absurd input.
            u32::try_from(world.entity_count()).unwrap_or(u32::MAX)
        })
    }

    fn is_valid_entity(&mut self, entity: WitEntityHandle) -> bool {
        self.invoke_guarded("ecs-read.is-valid-entity", false, |slot| {
            if !slot.grant.can_read() {
                return false;
            }
            let Some(world) = slot.world.as_ref() else {
                return false;
            };
            world.is_alive(from_wit_entity(entity))
        })
    }

    fn has_component(&mut self, entity: WitEntityHandle, schema_id: String) -> bool {
        self.invoke_guarded("ecs-read.has-component", false, |slot| {
            if !slot.grant.can_read() {
                return false;
            }
            let Some(world) = slot.world.as_ref() else {
                return false;
            };
            let entity = from_wit_entity(entity);
            let Some(type_id) = world.type_id_for_schema(&schema_id) else {
                return false;
            };
            world.has_component_erased(entity, type_id)
        })
    }

    fn get(&mut self, entity: WitEntityHandle, schema_id: String) -> Option<WitComponentValue> {
        self.invoke_guarded("ecs-read.get", None, |slot| {
            if !slot.grant.can_read() {
                return None;
            }
            let world = slot.world.as_ref()?;
            let entity = from_wit_entity(entity);
            let type_id = world.type_id_for_schema(&schema_id)?;
            let erased = world.get_erased(entity, type_id)?;
            let value = slot.codecs.to_value(type_id, erased)?;
            Some(to_wit_value(value))
        })
    }
}

impl ecs_write::Host for WorldSlot {
    fn set(
        &mut self,
        entity: WitEntityHandle,
        schema_id: String,
        value: WitComponentValue,
    ) -> bool {
        self.invoke_guarded("ecs-write.set", false, |slot| {
            if !slot.grant.can_write() {
                return false;
            }
            // Uniform-immediate write semantics (Decision 1): one
            // overwrite via the existing `set_erased` path, applied
            // now and visible to the next host call in this
            // invocation and to the next runtime phase after the
            // world moves home. `false` changes nothing — there is
            // no partial application because there is no multi-step
            // operation. No tick advance here (Decision 3): the
            // overwrite stamps the current tick, which the next real
            // scheduled pass observes through its normal probe.
            let Some(world) = slot.world.as_mut() else {
                return false;
            };
            let entity = from_wit_entity(entity);
            let Some(type_id) = world.type_id_for_schema(&schema_id) else {
                return false;
            };
            let Some(Ok(boxed)) = slot.codecs.from_value(type_id, from_wit_value(value)) else {
                return false;
            };
            world.set_erased(entity, type_id, boxed)
        })
    }
}
