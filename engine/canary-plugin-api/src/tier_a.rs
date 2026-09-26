// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Tier A: sandboxed WASM Component Model plugin loading. See
//! `docs/architecture/plugin-system.md#tier-a--sandboxed-wasm-component-model`
//! and `docs/roadmap/v0.0.3-roadmap.md`.
//!
//! What this proves, end to end, against a real Wasmtime engine:
//!
//! - Loading and instantiating a real WASM Component Model artifact,
//!   either freshly compiled ([`WasmPluginLoader::load`]) or a
//!   precompiled (AOT) artifact deserialized back in — see the AOT
//!   tests in this module for the full round trip.
//! - The [`Plugin`] lifecycle (`on_load`/`on_unload`) working through a
//!   component, not just a native dylib.
//! - **Structural** capability enforcement: [`WasmPluginLoader::load`]
//!   only links a capability's host functions into the instance's
//!   [`Linker`] when that capability was actually granted — proven
//!   independently for both `ecs-read`/[`Capability::ReadEcsWorld`] and
//!   `ecs-write`/[`Capability::WriteEcsWorld`], since capability gating
//!   happens per-interface, not per-grant. A component whose world
//!   imports an ungranted capability's interface has nothing to link
//!   against and fails at *instantiation* — before any of its own code
//!   runs — not merely a call that gets rejected.
//! - **A resource budget** ([`ResourceBudget`]), genuinely separate from
//!   capability-based authority: a memory limit (a `memory.grow` past it
//!   fails, doesn't trap — see the memory-budget test) and a fuel
//!   execution budget (an infinite loop traps once it's exhausted — see
//!   the fuel-budget test), both applied to every instance a given
//!   loader creates, not opt-in.
//! - The full first-cut ECS data ABI: `get`/`set`/`has-component`/
//!   `is-valid-entity`, `SCHEMA_ID`-addressed through
//!   [`canary_ecs::World::type_id_for_schema`] (identity) and
//!   [`crate::component_value::CodecRegistry`] (representation) —
//!   see `crate::component_value`'s module docs for why those are two
//!   different mechanisms.
//!
//! See the tests in this module for all of the above proven directly,
//! not merely asserted in this comment.
//!
//! What it does **not** yet cover — real, separately scoped work, not
//! an oversight:
//!
//! - **Anything beyond the two lifecycle callbacks.** Scoped access
//!   to the running [`canary_ecs::World`] exists —
//!   [`WasmComponentPlugin::call_scoped`] loans the active world for
//!   exactly one guest call (ownership loan, no borrow across
//!   `Func::call`, nested loans refused) and
//!   [`WasmPluginLoader::load_scoped`] is the fallible seam for it —
//!   but only for `on_load`/`on_unload` ([`PluginPhase`]). No
//!   per-frame plugin hook, no editor-panel API.
//!
//! The legacy [`WasmPluginLoader::load`] path (a separately owned
//! `World` moved into the instance) remains for single-owner tests
//! and tooling; it is not the active-game-world path.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use canary_ecs::{Tick, World};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use crate::capability::Capability;
use crate::component_value::{CodecRegistry, ComponentValue, PrimitiveValue};
use crate::error::PluginError;
use crate::plugin::{Plugin, PluginPhase, PluginRequirement};

/// A resource budget applied to every Tier A instance a given
/// [`WasmPluginLoader`] loads — a sandboxing property genuinely
/// separate from [`Capability`]-based authority: a component with
/// *zero* granted capabilities can still attempt to exhaust memory or
/// spin the CPU forever, and "sandboxed" here means both "can't reach
/// what it wasn't granted" (capabilities) *and* "can't consume
/// unbounded host resources" (this). See
/// `docs/roadmap/v0.0.3-roadmap.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudget {
    /// Maximum linear memory, in bytes, any single instance may grow to.
    /// A `memory.grow` past this fails (returns `-1` to the guest, per
    /// core WASM semantics for a failed grow) rather than trapping —
    /// the same behavior as genuinely running out of host memory, which
    /// well-behaved guest code already has to handle.
    pub max_memory_bytes: usize,
    /// Execution budget, in Wasmtime "fuel" units — consumed
    /// approximately per WASM instruction executed. Exhausting it traps
    /// the current call, which is how an infinite (or just
    /// pathologically long) loop in untrusted guest code gets bounded,
    /// without needing a separate watchdog thread.
    pub fuel: u64,
}

impl Default for ResourceBudget {
    /// 64 MiB of memory, 10,000,000 fuel units — generous enough for
    /// legitimate plugin logic in this first cut, not tuned against any
    /// real workload yet. Revisit once one exists.
    fn default() -> Self {
        Self {
            max_memory_bytes: 64 * 1024 * 1024,
            fuel: 10_000_000,
        }
    }
}

wasmtime::component::bindgen!({
    path: "wit",
    world: "tier-a-plugin",
});

use canary::plugin::types::{
    ComponentValue as WitComponentValue, EntityHandle as WitEntityHandle,
    PrimitiveValue as WitPrimitiveValue,
};

fn from_wit_entity(handle: WitEntityHandle) -> canary_ecs::Entity {
    canary_ecs::Entity::from_raw_parts(handle.index, handle.generation)
}

fn to_wit_primitive(value: PrimitiveValue) -> WitPrimitiveValue {
    match value {
        PrimitiveValue::U32(v) => WitPrimitiveValue::U32(v),
        PrimitiveValue::S32(v) => WitPrimitiveValue::S32(v),
        PrimitiveValue::U64(v) => WitPrimitiveValue::U64(v),
        PrimitiveValue::S64(v) => WitPrimitiveValue::S64(v),
        PrimitiveValue::F32(v) => WitPrimitiveValue::F32(v),
        PrimitiveValue::F64(v) => WitPrimitiveValue::F64(v),
        PrimitiveValue::Bool(v) => WitPrimitiveValue::Bool(v),
        PrimitiveValue::Str(v) => WitPrimitiveValue::String(v),
    }
}

fn from_wit_primitive(value: WitPrimitiveValue) -> PrimitiveValue {
    match value {
        WitPrimitiveValue::U32(v) => PrimitiveValue::U32(v),
        WitPrimitiveValue::S32(v) => PrimitiveValue::S32(v),
        WitPrimitiveValue::U64(v) => PrimitiveValue::U64(v),
        WitPrimitiveValue::S64(v) => PrimitiveValue::S64(v),
        WitPrimitiveValue::F32(v) => PrimitiveValue::F32(v),
        WitPrimitiveValue::F64(v) => PrimitiveValue::F64(v),
        WitPrimitiveValue::Bool(v) => PrimitiveValue::Bool(v),
        WitPrimitiveValue::String(v) => PrimitiveValue::Str(v),
    }
}

fn to_wit_value(value: ComponentValue) -> WitComponentValue {
    match value {
        ComponentValue::Primitive(v) => WitComponentValue::Primitive(to_wit_primitive(v)),
        ComponentValue::List(items) => {
            WitComponentValue::List(items.into_iter().map(to_wit_primitive).collect())
        }
        ComponentValue::Record(fields) => WitComponentValue::Record(
            fields
                .into_iter()
                .map(|(name, value)| (name, to_wit_primitive(value)))
                .collect(),
        ),
    }
}

fn from_wit_value(value: WitComponentValue) -> ComponentValue {
    match value {
        WitComponentValue::Primitive(v) => ComponentValue::Primitive(from_wit_primitive(v)),
        WitComponentValue::List(items) => {
            ComponentValue::List(items.into_iter().map(from_wit_primitive).collect())
        }
        WitComponentValue::Record(fields) => ComponentValue::Record(
            fields
                .into_iter()
                .map(|(name, value)| (name, from_wit_primitive(value)))
                .collect(),
        ),
    }
}

/// Outcome of one guarded host-function body: either the body's value
/// or a trap error for the guest. Kept behind [`PluginError`] by the
/// caller — no third-party type crosses this crate's public boundary.
pub(crate) fn guard_host_call<T>(
    plugin: &str,
    op: &'static str,
    body: impl FnOnce() -> T + std::panic::UnwindSafe,
) -> Result<T, wasmtime::Error> {
    std::panic::catch_unwind(body).map_err(|payload| {
        let detail = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|value| (*value).to_string())
            })
            .unwrap_or_else(|| "non-string panic payload".to_string());
        wasmtime::Error::msg(format!(
            "host function `{op}` for plugin `{plugin}` panicked: {detail}"
        ))
    })
}

/// What one Tier A instance may do with the loaned active world.
/// Decided once at load; enforced structurally by linker wiring (an
/// ungranted interface is never linked, so the guest cannot even name
/// it), mirrored here so host functions and audits read the same
/// truth. The per-interface host bodies additionally consult this as
/// a backstop, returning their benign default when the grant does not
/// cover the interface — unreachable while linking and grants agree,
/// which [`WasmComponentPlugin::call_scoped`] keeps true by storing
/// the call's grant into the slot before running guest code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedGrant {
    /// The capability subset actually linked for this instance:
    /// any subset of [`Capability::ReadEcsWorld`] /
    /// [`Capability::WriteEcsWorld`].
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
struct WorldSlot {
    /// The loaned active world; `None` while home in the runtime or
    /// before any loan. Host functions only observe it through
    /// short-lived borrows that end before the host call returns —
    /// nothing outlives the invocation.
    world: Option<World>,
    /// Fixed at loader construction; never per-invocation.
    codecs: Arc<CodecRegistry>,
    /// The capability subset actually linked for this instance,
    /// refreshed from the call's grant on every scoped invoke.
    grant: ScopedGrant,
    /// Backs the memory half of [`ResourceBudget`] — wired to the
    /// [`Store`] via [`Store::limiter`] in
    /// [`WasmPluginLoader::instantiate`]. The fuel half doesn't need a
    /// field here; it's set directly on the `Store` via
    /// [`Store::set_fuel`].
    limits: StoreLimits,
    /// Loader-supplied name, for host-panic diagnostics only. Never
    /// exposed to the guest.
    plugin_name: String,
    /// Reentrancy tripwire: greater than zero while inside a host
    /// call. Host functions are synchronous and never re-enter the
    /// guest, so this returns to zero before every `Func::call`
    /// returns; [`WasmComponentPlugin::call_scoped`] debug-asserts
    /// that on entry. Nested loans are refused by the `Option`
    /// ownership checks, not by this counter — it exists so a future
    /// reentrant path fails loudly in debug builds instead of
    /// silently.
    depth: u32,
    /// Tick captured at loan time; debug-asserted equal after reclaim
    /// (plugin phases never advance the ECS tick — writes stamp
    /// component ticks, the counter must not move).
    tick_before: Option<Tick>,
    /// A caught host panic for the current invocation, converted to a
    /// [`PluginError::GuestTrap`] once the guest call settles. Later
    /// host calls in the same invocation short-circuit to their
    /// benign default once this is set.
    pending_trap: Option<String>,
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

impl canary::plugin::ecs_read::Host for WorldSlot {
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

impl canary::plugin::ecs_write::Host for WorldSlot {
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

/// A loaded, instantiated Tier A plugin.
pub struct WasmComponentPlugin {
    name: String,
    store: Store<WorldSlot>,
    bindings: TierAPlugin,
    /// The per-entry execution budget, re-armed by [`WasmComponentPlugin::refuel`]
    /// before every guest entry point. Wasmtime fuel is consumed
    /// permanently (never self-replenishing), so without re-arming, a
    /// long-lived plugin would trap forever once its first budget ran
    /// out — every later call, including `on_unload`, would fail.
    fuel: u64,
}

impl Plugin for WasmComponentPlugin {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn on_load(&mut self) {
        // A guest trap (e.g. the component's own logic panicking)
        // surfaces as an `Err` here. `Plugin::on_load` is infallible by
        // signature — matching Tier B's C ABI, whose `on_load` vtable
        // entry has no way to report failure either (see
        // `crate::abi::PluginVTable`) — so a trap is reported via
        // `tracing::warn!` rather than propagated, consistent with, not
        // a new gap relative to, the existing trait's shape. The
        // runtime's scoped loader seam (`load_scoped`/`call_scoped`)
        // is the fallible path required plugins use instead.
        self.refuel();
        let result = self
            .bindings
            .canary_plugin_lifecycle()
            .call_on_load(&mut self.store);
        self.report_legacy_trap("on_load", result);
    }

    fn on_unload(&mut self) {
        // See `on_load` above for why a trap here is reported via
        // `tracing::warn!`, not propagated.
        self.refuel();
        let result = self
            .bindings
            .canary_plugin_lifecycle()
            .call_on_unload(&mut self.store);
        self.report_legacy_trap("on_unload", result);
    }
}

impl WasmComponentPlugin {
    /// Re-arms the execution budget before a guest entry point.
    /// Every guest call must go through this first: fuel never
    /// replenishes itself, and an exhausted budget traps the call it
    /// runs out in — so re-arming per entry is what keeps a long-lived
    /// plugin (and its `on_unload`) callable indefinitely.
    fn refuel(&mut self) {
        // `set_fuel` only fails if fuel consumption was never enabled
        // on the engine, which `WasmPluginLoader::new` always enables;
        // an error here would mean the budget is silently unenforced,
        // so fail loudly rather than run unbounded.
        self.store
            .set_fuel(self.fuel)
            .expect("fuel consumption must be enabled on the Wasmtime engine");
    }

    /// Reports a legacy (infallible-trait) guest call: a caught host
    /// panic first, then a guest trap, both via `tracing::warn!`.
    /// Drains `pending_trap` so a later scoped call starts clean.
    fn report_legacy_trap(
        &mut self,
        entrypoint: &'static str,
        result: Result<(), wasmtime::Error>,
    ) {
        if let Some(trap) = self.store.data_mut().pending_trap.take() {
            tracing::warn!(
                plugin = %self.name,
                entrypoint,
                trap = %trap,
                "Tier A host function panicked during {entrypoint}; \
                 the infallible Plugin trait signature cannot propagate it"
            );
        }
        if let Err(error) = result {
            tracing::warn!(
                plugin = %self.name,
                entrypoint,
                ?error,
                "Tier A guest trapped during {entrypoint}; \
                 the infallible Plugin trait signature cannot propagate it"
            );
        }
    }

    /// Invoke one lifecycle callback with the loaned active world.
    ///
    /// The single choke point for scoped Tier A access: takes
    /// `world` (leaving `None`, so no schedule can start and no
    /// second loan can issue while the guest runs), moves it into
    /// the store slot, runs the guest call to return-or-trap with
    /// fuel/memory limits armed, moves the world home, and maps
    /// failure to [`PluginError::GuestTrap`]. Never swallows: no
    /// logging here — the caller decides warn vs abort by
    /// [`PluginRequirement`].
    ///
    /// Exclusive with schedule execution by construction: the caller
    /// holds the runtime `&mut` while this runs, and the schedule is
    /// not re-entered. No world borrow or handle escapes: host
    /// functions hand out only owned values (counts, bools, owned
    /// [`ComponentValue`]s), never references.
    ///
    /// Returns `Ok(())` on clean return; `Err(GuestTrap{..})` on
    /// guest trap or caught host panic;
    /// `Err(WorldUnavailable{..})` when there is no loan to take
    /// (the runtime held `None`) or this instance already holds one
    /// (a nested loan).
    pub fn call_scoped(
        &mut self,
        world: &mut Option<World>,
        grant: &ScopedGrant,
        phase: PluginPhase,
    ) -> Result<(), PluginError> {
        let name = self.name.clone();
        // Scheduler overlap is structurally `None`: the runtime loans
        // by taking its own `Option`, so a `None` here means another
        // loan (or a schedule, which needs the world home) is active.
        let Some(loaned) = world.take() else {
            return Err(PluginError::WorldUnavailable {
                plugin: name,
                phase,
            });
        };
        // Nested loans are refused: this instance must not already
        // hold a world. (`&mut self` already rules out reentrant
        // calls through this handle; this check covers the
        // legacy-owned path — a plugin loaded with `load` still
        // holding its world — being driven through the scoped seam.)
        if self.store.data().world.is_some() {
            *world = Some(loaned);
            return Err(PluginError::WorldUnavailable {
                plugin: name,
                phase,
            });
        }
        debug_assert_eq!(
            self.store.data().depth,
            0,
            "scoped invoke must not re-enter an in-flight host call"
        );
        let tick_before = loaned.change_tick();
        {
            let slot = self.store.data_mut();
            slot.world = Some(loaned);
            slot.grant = grant.clone();
            slot.tick_before = Some(tick_before);
            slot.pending_trap = None;
        }
        self.refuel();
        let call_result = match phase {
            PluginPhase::OnLoad => self
                .bindings
                .canary_plugin_lifecycle()
                .call_on_load(&mut self.store),
            PluginPhase::OnUnload => self
                .bindings
                .canary_plugin_lifecycle()
                .call_on_unload(&mut self.store),
        };
        // Reclaim on every path — normal return, guest trap, or
        // caught host panic — through the same move-home. Host
        // functions only ever borrow the slot's world, never remove
        // it, so it must still be there.
        debug_assert!(
            self.store.data().world.is_some(),
            "the loaned world must still be in the slot: host functions borrow it, never remove it"
        );
        let slot = self.store.data_mut();
        let reclaimed = slot.world.take();
        let pending = slot.pending_trap.take();
        slot.tick_before = None;
        let Some(home) = reclaimed else {
            return Err(PluginError::WorldUnavailable {
                plugin: name,
                phase,
            });
        };
        // Decision 3: a plugin boundary never advances the tick. The
        // guest cannot call `advance_tick` (no such host function is
        // linked, under any capability); this asserts the host side
        // kept that contract too.
        debug_assert_eq!(
            home.change_tick(),
            tick_before,
            "plugin phases never advance the ECS tick"
        );
        *world = Some(home);
        if let Some(trap) = pending {
            return Err(PluginError::GuestTrap {
                plugin: name,
                phase,
                source: wasmtime::Error::msg(trap).into(),
            });
        }
        match call_result {
            Ok(()) => Ok(()),
            Err(source) => Err(PluginError::GuestTrap {
                plugin: name,
                phase,
                source: source.into(),
            }),
        }
    }

    /// Test-only accessor for what the guest cached via the `ecs-read`
    /// capability. Not part of the [`Plugin`] trait surface — specific
    /// to this slice's illustrative `ecs-read` interface, not anything
    /// the trait itself commits to.
    #[cfg(test)]
    fn last_entity_count(&mut self) -> u32 {
        self.bindings
            .canary_plugin_lifecycle()
            .call_last_entity_count(&mut self.store)
            .expect("last-entity-count should not trap for a well-formed test fixture")
    }
}

/// Loader-level reporting for one plugin registration: the fallible
/// seam the runtime's plugin-loading service uses, so required
/// plugins fail startup with a typed error while optional ones
/// degrade to `warn` + continue. [`Plugin::on_load`]/[`Plugin::on_unload`]
/// stay infallible — Tier B's C ABI cannot report failure, and a Tier
/// A concern must not churn it.
#[derive(Debug)]
pub enum PluginOutcome {
    /// The plugin instantiated and its scoped `on_load` returned cleanly.
    Loaded {
        /// The loader-supplied plugin name.
        name: String,
    },
    /// Optional-only: the load or scoped `on_load` failed with
    /// `cause` (already logged via `tracing::warn!`); the run
    /// continues without the plugin.
    SkippedOptional {
        /// The loader-supplied plugin name.
        name: String,
        /// The failure that was degraded to a skip.
        cause: PluginError,
    },
}

/// Loads Tier A (sandboxed WASM Component Model) plugins. See the
/// module-level docs above for exactly what this does and does not yet
/// cover.
pub struct WasmPluginLoader {
    engine: Engine,
    codecs: Arc<CodecRegistry>,
    budget: ResourceBudget,
}

impl WasmPluginLoader {
    /// Creates a loader with the Component Model enabled, `codecs` as
    /// the fixed set of component types Tier A plugins can access via
    /// `ecs-read`/`ecs-write` (if granted), and `budget` applied to
    /// every instance this loader goes on to load. Registration happens
    /// before construction, not after: `codecs` is shared cheaply
    /// (`Arc`) across every plugin this loader goes on to load, rather
    /// than cloned per instance, so there's no `&mut self` registration
    /// method to call once instances exist — register everything this
    /// loader will ever need up front.
    pub fn new(codecs: CodecRegistry, budget: ResourceBudget) -> Result<Self, PluginError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        // Enables the fuel-consumption accounting `budget.fuel` (set per
        // `Store` in `instantiate`, below) relies on; without this, the
        // engine simply doesn't track fuel at all, silently making
        // `set_fuel` a no-op instead of the execution budget it's
        // supposed to be.
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|source| PluginError::WasmEngineSetup {
            source: source.into(),
        })?;
        Ok(Self {
            engine,
            codecs: Arc::new(codecs),
            budget,
        })
    }

    /// Loads and instantiates the Tier A component at `path`, granting
    /// exactly the capabilities in `capabilities` and no others.
    ///
    /// Capability enforcement here is **structural**: this method only
    /// links a capability's host functions into the instance's
    /// [`Linker`] when that capability is present in `capabilities`. A
    /// component whose WIT world imports `ecs-read` but wasn't granted
    /// [`Capability::ReadEcsWorld`] (or `ecs-write` without
    /// [`Capability::WriteEcsWorld`]) has no way to even *reach* that
    /// import — instantiation itself fails with
    /// [`PluginError::WasmInstantiate`] (an unsatisfied-import error),
    /// before any of the component's own code runs, rather than
    /// succeeding and merely having a runtime call rejected.
    ///
    /// `name` is supplied by the caller rather than read from the
    /// component itself — see `wit/plugin.wit`'s module-level comment
    /// for why. `world` is moved into the resulting plugin's host
    /// slot as its starting loan state: this legacy owned-world path
    /// keeps single-owner tests and tooling working without a
    /// runtime. The active-game-world path loans per invocation
    /// through [`WasmComponentPlugin::call_scoped`] instead — never
    /// mix the two on one instance (a scoped call on an instance
    /// still holding its owned world is refused with
    /// [`PluginError::WorldUnavailable`]).
    pub fn load(
        &self,
        path: impl AsRef<Path>,
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
        world: World,
    ) -> Result<WasmComponentPlugin, PluginError> {
        let path = path.as_ref();
        let component =
            Component::from_file(&self.engine, path).map_err(|source| PluginError::WasmParse {
                path: path.to_path_buf(),
                source: source.into(),
            })?;
        let mut plugin = self.instantiate(component, path.to_path_buf(), name, capabilities)?;
        plugin.store.data_mut().world = Some(world);
        Ok(plugin)
    }

    /// Loads, instantiates, and scoped-`on_load`s the Tier A component
    /// at `path`: the fallible loader seam the runtime's
    /// plugin-loading service uses (Decision 4). Instantiation still
    /// denies ungranted capabilities structurally; the scoped
    /// `on_load` then runs against the loaned active `world` through
    /// [`WasmComponentPlugin::call_scoped`], and the world is
    /// reclaimed before this returns on every path.
    ///
    /// Required + failure (instantiation or scoped trap) →
    /// `Err`, so the runtime aborts startup with the typed failure.
    /// Optional + failure → `Ok((None, SkippedOptional{..}))` plus a
    /// `tracing::warn!`, and the run continues without the plugin.
    /// Success → `Ok((Some(plugin), Loaded{..}))`.
    ///
    /// `path` has no sketch counterpart (the review's sketch omits
    /// how bytes reach the loader); it is required the same way as
    /// in [`WasmPluginLoader::load`]. The returned plugin handle
    /// likewise extends the sketch, which returns only the outcome:
    /// the live instance must go somewhere, and dropping it inside
    /// the loader would unload what just loaded.
    pub fn load_scoped(
        &self,
        path: impl AsRef<Path>,
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
        requirement: PluginRequirement,
        world: &mut Option<World>,
    ) -> Result<(Option<WasmComponentPlugin>, PluginOutcome), PluginError> {
        let name = name.into();
        let path = path.as_ref();
        let component =
            Component::from_file(&self.engine, path).map_err(|source| PluginError::WasmParse {
                path: path.to_path_buf(),
                source: source.into(),
            })?;
        let mut plugin =
            match self.instantiate(component, path.to_path_buf(), name.clone(), capabilities) {
                Ok(plugin) => plugin,
                Err(error) => return Self::report_load_outcome(name, requirement, error),
            };
        let grant = ScopedGrant {
            capabilities: capabilities.clone(),
            budget: self.budget,
        };
        match plugin.call_scoped(world, &grant, PluginPhase::OnLoad) {
            Ok(()) => Ok((Some(plugin), PluginOutcome::Loaded { name })),
            Err(error) => Self::report_load_outcome(name, requirement, error),
        }
    }

    /// Maps one loader-level failure by [`PluginRequirement`]:
    /// required propagates, optional warns + skips.
    fn report_load_outcome(
        name: String,
        requirement: PluginRequirement,
        error: PluginError,
    ) -> Result<(Option<WasmComponentPlugin>, PluginOutcome), PluginError> {
        match requirement {
            PluginRequirement::Required => Err(error),
            PluginRequirement::Optional => {
                tracing::warn!(
                    plugin = %name,
                    ?error,
                    "optional Tier A plugin failed to load; continuing without it"
                );
                Ok((None, PluginOutcome::SkippedOptional { name, cause: error }))
            }
        }
    }

    /// Instantiates a Tier A component from in-memory bytes (rather
    /// than a file), granting exactly `capabilities` and starting
    /// with no loaned world — the handle source for scoped calls
    /// driven outside [`WasmPluginLoader::load_scoped`] (tests,
    /// tooling, and [`WasmComponentPlugin::call_scoped`] callers that
    /// hold the handle themselves). The runtime's own flow does not
    /// need this; it keeps the handles `load_scoped` returns.
    pub fn instantiate_from_bytes(
        &self,
        bytes: &[u8],
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
    ) -> Result<WasmComponentPlugin, PluginError> {
        let component =
            Component::new(&self.engine, bytes).map_err(|source| PluginError::WasmParse {
                path: PathBuf::from("<in-memory component>"),
                source: source.into(),
            })?;
        self.instantiate(
            component,
            PathBuf::from("<in-memory component>"),
            name,
            capabilities,
        )
    }

    /// Shared instantiation core for [`WasmPluginLoader::load`] (which
    /// parses `component` from a file) and this module's tests (which
    /// parse one from an inline WAT fixture instead, so this slice's
    /// tests need nothing beyond `cargo test` — no external
    /// `cargo-component`/`wit-bindgen` toolchain, per
    /// `docs/roadmap/v0.0.3-roadmap.md`'s documented fallback).
    ///
    /// Starts with no loaned world in the slot: scoped access arrives
    /// per invocation via [`WasmComponentPlugin::call_scoped`].
    fn instantiate(
        &self,
        component: Component,
        path_for_errors: PathBuf,
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
    ) -> Result<WasmComponentPlugin, PluginError> {
        let name = name.into();
        let mut linker = Linker::new(&self.engine);
        if capabilities.contains(&Capability::ReadEcsWorld) {
            canary::plugin::ecs_read::add_to_linker::<_, HasSelf<_>>(
                &mut linker,
                |state: &mut WorldSlot| state,
            )
            .map_err(|source| PluginError::WasmEngineSetup {
                source: source.into(),
            })?;
        }
        if capabilities.contains(&Capability::WriteEcsWorld) {
            canary::plugin::ecs_write::add_to_linker::<_, HasSelf<_>>(
                &mut linker,
                |state: &mut WorldSlot| state,
            )
            .map_err(|source| PluginError::WasmEngineSetup {
                source: source.into(),
            })?;
        }
        // `Filesystem`/`Network` have no corresponding Tier A interface
        // yet (see `wit/plugin.wit`'s module docs), so there is nothing
        // further to conditionally link for them at this stage.

        let limits = StoreLimitsBuilder::new()
            .memory_size(self.budget.max_memory_bytes)
            .build();
        let mut store = Store::new(
            &self.engine,
            WorldSlot {
                world: None,
                codecs: Arc::clone(&self.codecs),
                grant: ScopedGrant {
                    capabilities: capabilities.clone(),
                    budget: self.budget,
                },
                limits,
                plugin_name: name.clone(),
                depth: 0,
                tick_before: None,
                pending_trap: None,
            },
        );
        // Ties memory growth to `WorldSlot.limits` -- without this call,
        // the `StoreLimits` built above is inert data, not an enforced
        // budget.
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(self.budget.fuel)
            .map_err(|source| PluginError::WasmEngineSetup {
                source: source.into(),
            })?;

        let bindings =
            TierAPlugin::instantiate(&mut store, &component, &linker).map_err(|source| {
                PluginError::WasmInstantiate {
                    path: path_for_errors,
                    source: source.into(),
                }
            })?;

        Ok(WasmComponentPlugin {
            name,
            store,
            bindings,
            fuel: self.budget.fuel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_ecs::CanaryComponent;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::Event;
    use tracing_subscriber::layer::{Context, Layer};
    use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
    use tracing_subscriber::Registry;

    /// Captures `tracing` events for tests that prove a failure mode
    /// stays observable (trap→warn, optional-skip warn).
    struct CapturingLayer {
        events: Arc<Mutex<Vec<(tracing::Level, String)>>>,
    }

    struct MessageVisitor(String);
    impl Visit for MessageVisitor {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            let _ = write!(self.0, "{}={value:?} ", field.name());
        }
    }

    impl<S: tracing::Subscriber> Layer<S> for CapturingLayer {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);
            self.events
                .lock()
                .expect("test mutex should not be poisoned")
                .push((*event.metadata().level(), visitor.0));
        }
    }

    /// Runs `body` with a capturing subscriber and returns the
    /// captured `(level, fields)` events.
    fn captured_events(body: impl FnOnce()) -> Vec<(tracing::Level, String)> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let layer = CapturingLayer {
            events: Arc::clone(&events),
        };
        let subscriber = Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, body);
        let captured = events
            .lock()
            .expect("test mutex should not be poisoned")
            .clone();
        captured
    }

    /// A minimal, hand-written Component Model fixture: imports
    /// `ecs-read`, caches `entity-count` on `on-load`, and exports it
    /// back out via `last-entity-count` so a test can observe it. See
    /// the module docs for why this is WAT text rather than a compiled
    /// artifact.
    const TEST_COMPONENT_WAT: &str = r#"
        (component
          (import "canary:plugin/ecs-read@0.1.0" (instance $ecs-read-import
            (export "entity-count" (func (result u32)))
          ))

          (core module $guest
            (import "host" "entity-count" (func $entity_count (result i32)))
            (global $cached (mut i32) (i32.const 0))
            (func (export "on-load")
              (global.set $cached (call $entity_count)))
            (func (export "on-unload"))
            (func (export "last-entity-count") (result i32)
              (global.get $cached))
          )

          (core func $entity_count_lowered
            (canon lower (func $ecs-read-import "entity-count")))

          (core instance $guest_instance (instantiate $guest
            (with "host" (instance
              (export "entity-count" (func $entity_count_lowered))
            ))
          ))

          (func $on_load_lifted (canon lift (core func $guest_instance "on-load")))
          (func $on_unload_lifted (canon lift (core func $guest_instance "on-unload")))
          (func $last_entity_count_lifted (result u32)
            (canon lift (core func $guest_instance "last-entity-count")))

          (instance $lifecycle_export
            (export "on-load" (func $on_load_lifted))
            (export "on-unload" (func $on_unload_lifted))
            (export "last-entity-count" (func $last_entity_count_lifted))
          )
          (export "canary:plugin/lifecycle@0.1.0" (instance $lifecycle_export))
        )
    "#;

    fn world_with_entities(count: usize) -> World {
        let mut world = World::new();
        for _ in 0..count {
            let _ = world.spawn();
        }
        world
    }

    // -- Resource budget -----------------------------------------------

    #[test]
    fn fuel_budget_traps_a_component_that_loops_forever() {
        const INFINITE_LOOP_WAT: &str = r#"
            (component
              (core module $guest
                (func (export "on-load")
                  (loop $forever
                    br $forever
                  )
                )
                (func (export "on-unload"))
                (func (export "last-entity-count") (result i32) (i32.const 0))
              )
              (core instance $guest_instance (instantiate $guest))
              (func $on_load_lifted (canon lift (core func $guest_instance "on-load")))
              (func $on_unload_lifted (canon lift (core func $guest_instance "on-unload")))
              (func $last_entity_count_lifted (result u32)
                (canon lift (core func $guest_instance "last-entity-count")))
              (instance $lifecycle_export
                (export "on-load" (func $on_load_lifted))
                (export "on-unload" (func $on_unload_lifted))
                (export "last-entity-count" (func $last_entity_count_lifted))
              )
              (export "canary:plugin/lifecycle@0.1.0" (instance $lifecycle_export))
            )
        "#;

        let low_fuel = ResourceBudget {
            fuel: 1_000,
            ..ResourceBudget::default()
        };
        let loader = WasmPluginLoader::new(CodecRegistry::new(), low_fuel)
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, INFINITE_LOOP_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<fuel test fixture>"),
                "fuel-test-plugin",
                &HashSet::new(),
            )
            .expect("instantiation itself should succeed -- the loop only runs once called");

        // Bypassing the `Plugin` trait's `on_load` here specifically to
        // observe the `Result` it otherwise swallows (see `on_load`'s
        // own doc comment) -- this test's whole point is confirming
        // *that* a trap happens, which the trait's infallible signature
        // can't surface.
        let result = plugin
            .bindings
            .canary_plugin_lifecycle()
            .call_on_load(&mut plugin.store);
        assert!(
            result.is_err(),
            "an infinite loop should trap once its fuel budget is exhausted, not run forever"
        );
    }

    #[test]
    fn memory_budget_fails_growth_past_the_limit_rather_than_trapping() {
        const MEMORY_HUNGRY_WAT: &str = r#"
            (component
              (core module $guest
                (memory (export "mem") 1)
                (global $grow_result (mut i32) (i32.const -2))
                (func (export "on-load")
                  ;; 10,000 pages is roughly 640 MiB -- comfortably past
                  ;; ResourceBudget::default()'s 64 MiB limit. A failed
                  ;; memory.grow returns -1 (does not trap), so this
                  ;; observes enforcement without needing fuel to also
                  ;; run out first.
                  (global.set $grow_result (memory.grow (i32.const 10000)))
                )
                (func (export "on-unload"))
                ;; Repurposed for this fixture: reports the grow result
                ;; rather than a real entity count.
                (func (export "last-entity-count") (result i32) (global.get $grow_result))
              )
              (core instance $guest_instance (instantiate $guest))
              (func $on_load_lifted (canon lift (core func $guest_instance "on-load")))
              (func $on_unload_lifted (canon lift (core func $guest_instance "on-unload")))
              (func $last_entity_count_lifted (result u32)
                (canon lift (core func $guest_instance "last-entity-count")))
              (instance $lifecycle_export
                (export "on-load" (func $on_load_lifted))
                (export "on-unload" (func $on_unload_lifted))
                (export "last-entity-count" (func $last_entity_count_lifted))
              )
              (export "canary:plugin/lifecycle@0.1.0" (instance $lifecycle_export))
            )
        "#;

        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, MEMORY_HUNGRY_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<memory test fixture>"),
                "memory-test-plugin",
                &HashSet::new(),
            )
            .expect("instantiation itself should succeed");

        plugin.on_load();
        // `last-entity-count` is repurposed by this fixture (see above)
        // to report `memory.grow`'s actual result: `-1` (reinterpreted
        // as `u32::MAX` through the WIT `u32` return type) means the
        // grow failed, exactly as an over-budget request should.
        assert_eq!(
            plugin.last_entity_count(),
            u32::MAX,
            "growing memory far past the configured budget should fail (-1), not succeed or trap"
        );
    }

    // -- Direct HostState tests: the get/set/has-component/is-valid-entity
    // logic itself (schema resolution, type-erased ECS access, codec
    // conversion), called as plain Rust trait methods rather than through
    // a parsed WASM guest. A hand-written WAT fixture exercising `get`'s
    // `option<component-value>` return would need real Canonical-ABI
    // memory/realloc wiring (that return type's static shape includes
    // `list`, so it can't be passed in registers regardless of which
    // case is actually returned) -- disproportionate effort for a fixture
    // that isn't representative of how a real, toolchain-compiled plugin
    // would work anyway. This exercises the exact same `HostState` code a
    // real guest call would reach; the capability-*linking* mechanism
    // itself is proven separately, below, via the WASM boundary.

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Health {
        value: f32,
    }

    impl canary_ecs::CanaryComponent for Health {
        const SCHEMA_ID: &'static str = "canary-plugin-api-tests:health@1";
    }

    impl crate::component_value::ComponentValueCodec for Health {
        fn to_component_value(&self) -> ComponentValue {
            ComponentValue::Record(vec![("value".to_string(), PrimitiveValue::F32(self.value))])
        }

        fn from_component_value(
            value: ComponentValue,
        ) -> Result<Self, crate::component_value::ComponentValueError> {
            let ComponentValue::Record(fields) = value else {
                return Err(crate::component_value::ComponentValueError(
                    "Health expects a record".to_string(),
                ));
            };
            let PrimitiveValue::F32(value) = fields
                .into_iter()
                .find_map(|(name, v)| (name == "value").then_some(v))
                .ok_or_else(|| {
                    crate::component_value::ComponentValueError("missing field `value`".to_string())
                })?
            else {
                return Err(crate::component_value::ComponentValueError(
                    "Health.value expects an f32".to_string(),
                ));
            };
            Ok(Health { value })
        }
    }

    fn slot_with_a_health_entity(value: f32) -> (WorldSlot, canary_ecs::Entity) {
        let mut codecs = CodecRegistry::new();
        codecs.register::<Health>();

        let mut world = World::new();
        world.register_component::<Health>().unwrap();
        let entity = world.spawn();
        world.insert(entity, Health { value }).unwrap();

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);
        capabilities.insert(Capability::WriteEcsWorld);
        (
            WorldSlot {
                world: Some(world),
                codecs: Arc::new(codecs),
                grant: ScopedGrant {
                    capabilities,
                    budget: ResourceBudget::default(),
                },
                limits: StoreLimitsBuilder::new().build(),
                plugin_name: "slot-test".to_string(),
                depth: 0,
                tick_before: None,
                pending_trap: None,
            },
            entity,
        )
    }

    /// The exact host-function bodies a real guest call reaches (see
    /// the module docs for why WAT fixtures stop at the boundary for
    /// `get`/`set`): `set` applies immediately through `set_erased`
    /// and a later `get` in stub order reads it back.
    #[test]
    fn host_slot_set_then_get_reads_back_in_stub_order() {
        let (mut slot, entity) = slot_with_a_health_entity(42.0);
        let handle = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation(),
        };
        let schema_id = Health::SCHEMA_ID.to_string();
        let tick_before = slot
            .world
            .as_ref()
            .expect("slot holds the world")
            .change_tick();

        assert!(canary::plugin::ecs_read::Host::is_valid_entity(
            &mut slot, handle
        ));
        assert!(canary::plugin::ecs_read::Host::has_component(
            &mut slot,
            handle,
            schema_id.clone()
        ));

        let value = canary::plugin::ecs_read::Host::get(&mut slot, handle, schema_id.clone())
            .map(from_wit_value);
        assert_eq!(
            value,
            Some(ComponentValue::Record(vec![(
                "value".to_string(),
                PrimitiveValue::F32(42.0)
            )]))
        );

        let new_value =
            WitComponentValue::Record(vec![("value".to_string(), WitPrimitiveValue::F32(99.0))]);
        let wrote =
            canary::plugin::ecs_write::Host::set(&mut slot, handle, schema_id.clone(), new_value);
        assert!(wrote);
        // Immediate visibility within one invocation: the next `get`
        // reads back the write with no flush or commit step between.
        let reread =
            canary::plugin::ecs_read::Host::get(&mut slot, handle, schema_id).map(from_wit_value);
        assert_eq!(
            reread,
            Some(ComponentValue::Record(vec![(
                "value".to_string(),
                PrimitiveValue::F32(99.0)
            )]))
        );
        assert_eq!(
            slot.world
                .as_ref()
                .expect("slot holds the world")
                .get::<Health>(entity),
            Some(&Health { value: 99.0 })
        );
        // Decision 3: overwrites stamp component ticks, never the
        // tick counter.
        assert_eq!(
            slot.world
                .as_ref()
                .expect("slot holds the world")
                .change_tick(),
            tick_before
        );
    }

    #[test]
    fn host_slot_get_returns_none_for_an_unknown_schema_id() {
        let (mut slot, entity) = slot_with_a_health_entity(1.0);
        let handle = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation(),
        };

        let value =
            canary::plugin::ecs_read::Host::get(&mut slot, handle, "no-such-schema".to_string())
                .map(from_wit_value);
        assert_eq!(value, None);
    }

    #[test]
    fn host_slot_set_returns_false_and_changes_nothing_for_a_stale_entity() {
        let (mut slot, entity) = slot_with_a_health_entity(1.0);
        // A different generation for the same slot -- a stale handle,
        // per Entity::from_raw_parts's documented safety property.
        let stale = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation() + 1,
        };

        let wrote = canary::plugin::ecs_write::Host::set(
            &mut slot,
            stale,
            Health::SCHEMA_ID.to_string(),
            WitComponentValue::Record(vec![("value".to_string(), WitPrimitiveValue::F32(999.0))]),
        );

        assert!(!wrote);
        assert_eq!(
            slot.world
                .as_ref()
                .expect("slot holds the world")
                .get::<Health>(entity),
            Some(&Health { value: 1.0 }),
            "the real entity's data must be untouched by a set() targeting a stale handle"
        );
    }

    /// The grant backstop: even with `ecs-write` linked, a call whose
    /// grant withholds the write capability is denied at the host
    /// boundary. Unreachable while linking and grants agree (the
    /// guest could not import `set` at all), but the host body must
    /// still read the same truth the linker enforced.
    #[test]
    fn host_slot_set_is_denied_when_the_grant_withholds_write() {
        let (mut slot, entity) = slot_with_a_health_entity(1.0);
        slot.grant.capabilities.remove(&Capability::WriteEcsWorld);
        assert!(!slot.grant.can_write());
        let handle = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation(),
        };

        let wrote = canary::plugin::ecs_write::Host::set(
            &mut slot,
            handle,
            Health::SCHEMA_ID.to_string(),
            WitComponentValue::Record(vec![("value".to_string(), WitPrimitiveValue::F32(2.0))]),
        );

        assert!(!wrote);
        assert_eq!(
            slot.world
                .as_ref()
                .expect("slot holds the world")
                .get::<Health>(entity),
            Some(&Health { value: 1.0 })
        );
    }

    /// Nothing escapes: `get` hands out an owned value, so mutating
    /// what the guest received cannot reach back into the world.
    #[test]
    fn host_slot_get_hands_out_owned_values_only() {
        let (mut slot, entity) = slot_with_a_health_entity(5.0);
        let handle = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation(),
        };

        let mut value =
            canary::plugin::ecs_read::Host::get(&mut slot, handle, Health::SCHEMA_ID.to_string())
                .map(from_wit_value)
                .expect("the component is present");
        if let ComponentValue::Record(fields) = &mut value {
            for (_, item) in fields.iter_mut() {
                *item = PrimitiveValue::F32(0.0);
            }
        }

        assert_eq!(
            slot.world
                .as_ref()
                .expect("slot holds the world")
                .get::<Health>(entity),
            Some(&Health { value: 5.0 }),
            "mutating a returned value must not reach back into the world"
        );
    }

    /// Generation parity (§6 ruling): entity handles round-trip as
    /// `(u32, u64)` with no truncation — the WIT `entity-handle`
    /// shape already mirrors `Entity` exactly, pinned here so any
    /// future narrowing breaks loudly instead of aliasing live
    /// entities at the trust boundary.
    #[test]
    fn entity_handles_round_trip_with_u64_generations() {
        let mut world = World::new();
        let entity = world.spawn();
        let rebuilt = canary_ecs::Entity::from_raw_parts(entity.index(), entity.generation());
        assert_eq!(rebuilt, entity);
        assert!(world.is_alive(rebuilt));
        // Neighbour generations on either side of the true one are
        // dead handles — a truncated/wrapped generation must never
        // alias a live entity.
        assert!(!world.is_alive(canary_ecs::Entity::from_raw_parts(
            entity.index(),
            entity.generation().wrapping_add(1)
        )));
        assert!(!world.is_alive(canary_ecs::Entity::from_raw_parts(
            entity.index(),
            entity.generation().wrapping_sub(1)
        )));
    }

    // -- AOT compilation --------------------------------------------------

    /// Proves the AOT (ahead-of-time) compilation path end to end:
    /// `Engine::precompile_component` on real WASM bytes, then
    /// `Component::deserialize` on the result, then instantiating and
    /// running that deserialized component exactly as if it had been
    /// loaded fresh. This proves the *mechanism* -- WASM in, a
    /// precompiled artifact out, that artifact loads and runs correctly
    /// -- not that it's fast; no benchmark backs a performance claim
    /// here, and none should be inferred from a successful round trip.
    /// See `docs/roadmap/v0.0.3-roadmap.md` for why `.cwasm` is treated
    /// as a toolchain-tied cache artifact, not the canonical
    /// distribution format (that stays the plain `.wasm`/WAT source).
    #[test]
    fn a_precompiled_component_loads_and_runs_identically_to_a_freshly_compiled_one() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");

        let wasm_bytes = wat::parse_str(TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse to a valid binary component");
        let precompiled = loader
            .engine
            .precompile_component(&wasm_bytes)
            .expect("precompilation of a valid component should succeed");

        // SAFETY: `precompiled` was just produced, above, by this exact
        // `loader.engine`'s own `precompile_component` -- the
        // "compatible engine configuration" `Component::deserialize`
        // requires is trivially satisfied here, not merely assumed.
        let component = unsafe { Component::deserialize(&loader.engine, precompiled) }.expect(
            "deserializing an artifact from the same engine that precompiled it should succeed",
        );

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);
        let grant = ScopedGrant {
            capabilities: capabilities.clone(),
            budget: ResourceBudget::default(),
        };
        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<precompiled test fixture>"),
                "aot-test-plugin",
                &capabilities,
            )
            .expect("instantiating a deserialized precompiled component should succeed");

        // The scoped loan reaches the active world: the guest reads
        // the loaned world's real entity count, not a fixture copy.
        let mut world = Some(world_with_entities(7));
        let tick_before = world
            .as_ref()
            .expect("world is home before the loan")
            .change_tick();
        plugin
            .call_scoped(&mut world, &grant, PluginPhase::OnLoad)
            .expect("a well-formed scoped on_load must succeed");
        assert_eq!(
            plugin.last_entity_count(),
            7,
            "a precompiled component must behave identically to a freshly compiled one, \
             including reaching the loaned active World through a granted capability"
        );
        assert!(
            world.is_some(),
            "the loaned world must move home after the scoped call"
        );
        assert_eq!(
            world.as_ref().expect("world moved home").change_tick(),
            tick_before,
            "a read-only plugin phase must not advance the ECS tick"
        );
    }

    // -- WASM-boundary tests: the capability *linking* mechanism itself --

    #[test]
    fn fuel_is_rearmed_before_every_guest_entry_point() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);
        let grant = ScopedGrant {
            capabilities: capabilities.clone(),
            budget: ResourceBudget::default(),
        };

        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<test fixture>"),
                "test-plugin",
                &capabilities,
            )
            .expect("instantiation with the granted capability should succeed");

        // Wasmtime fuel never replenishes itself: without per-entry
        // re-arming, the second call would run on whatever the first
        // left over (strictly less), and a long-lived plugin would
        // eventually trap forever. Equal remaining fuel after two
        // identical entries proves each was re-armed, not drained.
        let mut world = Some(world_with_entities(5));
        plugin
            .call_scoped(&mut world, &grant, PluginPhase::OnLoad)
            .expect("scoped on_load must succeed");
        assert_eq!(plugin.last_entity_count(), 5);
        let fuel_after_first = plugin
            .store
            .get_fuel()
            .expect("fuel consumption is enabled on this engine");
        plugin
            .call_scoped(&mut world, &grant, PluginPhase::OnLoad)
            .expect("scoped on_load must succeed");
        assert_eq!(plugin.last_entity_count(), 5);
        let fuel_after_second = plugin
            .store
            .get_fuel()
            .expect("fuel consumption is enabled on this engine");
        assert_eq!(
            fuel_after_first, fuel_after_second,
            "each guest entry must start from a full budget, not the previous call's remainder"
        );
    }

    #[test]
    fn granting_read_ecs_world_lets_the_component_read_real_world_state() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);
        let grant = ScopedGrant {
            capabilities: capabilities.clone(),
            budget: ResourceBudget::default(),
        };

        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<test fixture>"),
                "test-plugin",
                &capabilities,
            )
            .expect("instantiation with the granted capability should succeed");

        assert_eq!(plugin.name(), "test-plugin");
        // Scoped R/W through granted interfaces, with exact stub-order
        // proof: the guest's `on-load` runs against the loaned active
        // world and caches what it read; the host observes the same
        // value the guest saw, in order, after the world moves home.
        let mut world = Some(world_with_entities(5));
        let tick_before = world
            .as_ref()
            .expect("world is home before the loan")
            .change_tick();
        plugin
            .call_scoped(&mut world, &grant, PluginPhase::OnLoad)
            .expect("scoped on_load must succeed");
        assert_eq!(
            plugin.last_entity_count(),
            5,
            "the component should have read the loaned active World's entity count through \
             the ecs-read capability, not a fake or stale value"
        );
        let home = world.as_ref().expect("the loaned world must move home");
        assert_eq!(home.entity_count(), 5);
        assert_eq!(home.change_tick(), tick_before);
    }

    /// No schedule overlap: with the runtime holding `None` (a loan
    /// already out, or a pass running), a second scoped call is
    /// refused before any guest code runs — and the instance is
    /// left untouched for a later, properly loaned call.
    #[test]
    fn scoped_call_without_a_loan_is_refused() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);
        let grant = ScopedGrant {
            capabilities: capabilities.clone(),
            budget: ResourceBudget::default(),
        };
        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<test fixture>"),
                "test-plugin",
                &capabilities,
            )
            .expect("instantiation with the granted capability should succeed");

        let mut no_world = None;
        match plugin.call_scoped(&mut no_world, &grant, PluginPhase::OnLoad) {
            Err(PluginError::WorldUnavailable { plugin, phase }) => {
                assert_eq!(plugin, "test-plugin");
                assert_eq!(phase, PluginPhase::OnLoad);
            }
            Err(other) => panic!("expected WorldUnavailable, got: {other}"),
            Ok(()) => panic!("a scoped call with no loaned world must be refused"),
        }

        // The refused instance still works once properly loaned.
        let mut world = Some(world_with_entities(2));
        plugin
            .call_scoped(&mut world, &grant, PluginPhase::OnLoad)
            .expect("scoped on_load must succeed after the refusal");
        assert_eq!(plugin.last_entity_count(), 2);
    }

    /// Nested loans are refused: an instance still holding a world
    /// (here via the legacy owned-world `load` path) cannot take a
    /// second loan through the scoped seam, and the runtime's world
    /// is handed back untouched.
    #[test]
    fn scoped_call_refuses_a_nested_loan() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);
        let grant = ScopedGrant {
            capabilities: capabilities.clone(),
            budget: ResourceBudget::default(),
        };
        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<test fixture>"),
                "test-plugin",
                &capabilities,
            )
            .expect("instantiation with the granted capability should succeed");
        // Simulate an outstanding loan: the slot already holds a world.
        plugin.store.data_mut().world = Some(world_with_entities(11));

        let mut runtime_world = Some(world_with_entities(3));
        match plugin.call_scoped(&mut runtime_world, &grant, PluginPhase::OnLoad) {
            Err(PluginError::WorldUnavailable { .. }) => {}
            Err(other) => panic!("expected WorldUnavailable, got: {other}"),
            Ok(()) => panic!("a nested scoped loan must be refused"),
        }
        assert_eq!(
            runtime_world
                .as_ref()
                .expect("the refused loan must be handed back")
                .entity_count(),
            3
        );
    }

    /// No escape across invocations: each scoped call sees exactly
    /// the world loaned for that call — never the previous loan's
    /// contents, never a retained handle.
    #[test]
    fn scoped_loans_see_exactly_the_currently_loaned_world() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);
        let grant = ScopedGrant {
            capabilities: capabilities.clone(),
            budget: ResourceBudget::default(),
        };
        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<test fixture>"),
                "test-plugin",
                &capabilities,
            )
            .expect("instantiation with the granted capability should succeed");

        let mut first = Some(world_with_entities(3));
        plugin
            .call_scoped(&mut first, &grant, PluginPhase::OnLoad)
            .expect("first scoped on_load must succeed");
        assert_eq!(plugin.last_entity_count(), 3);

        let mut second = Some(world_with_entities(9));
        plugin
            .call_scoped(&mut second, &grant, PluginPhase::OnLoad)
            .expect("second scoped on_load must succeed");
        assert_eq!(
            plugin.last_entity_count(),
            9,
            "the second invocation must see only the second loan, nothing retained from the first"
        );
        assert_eq!(
            first
                .as_ref()
                .expect("first world moved home")
                .entity_count(),
            3
        );
    }

    /// The point of this module: proves capability denial is
    /// *structural*. If this regressed to a runtime-checked-and-denied
    /// call instead, instantiation below would succeed and the failure
    /// (if any) would show up when calling `on_load`, not here.
    #[test]
    fn denying_read_ecs_world_makes_the_import_structurally_unreachable() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let capabilities = HashSet::new(); // Nothing granted.

        let result = loader.instantiate(
            component,
            PathBuf::from("<test fixture>"),
            "test-plugin",
            &capabilities,
        );

        match result {
            Err(PluginError::WasmInstantiate { .. }) => {}
            Err(other) => panic!(
                "expected WasmInstantiate (an unsatisfied-import failure at instantiation \
                 time), got a different error: {other}"
            ),
            Ok(_) => panic!(
                "instantiation succeeded without the ReadEcsWorld capability granted -- \
                 capability enforcement has regressed from structural to merely advisory"
            ),
        }
    }

    /// Symmetric to the `ecs-read` proof above, for `ecs-write`: even a
    /// component that *would* be granted `ecs-read` fine still can't
    /// instantiate if it separately imports `ecs-write` and wasn't
    /// granted `WriteEcsWorld` -- each capability gates its own
    /// interface independently, not "any capability lets everything in".
    #[test]
    fn denying_write_ecs_world_makes_that_import_structurally_unreachable_even_with_read_granted() {
        const WRITE_IMPORTING_COMPONENT_WAT: &str = r#"
            (component
              (import "canary:plugin/ecs-write@0.1.0" (instance $ecs-write-import
                (export "set" (func (result bool)))
              ))
              (core module $guest
                (func (export "on-load"))
                (func (export "on-unload"))
                (func (export "last-entity-count") (result i32) (i32.const 0))
              )
              (core instance $guest_instance (instantiate $guest))
              (func $on_load_lifted (canon lift (core func $guest_instance "on-load")))
              (func $on_unload_lifted (canon lift (core func $guest_instance "on-unload")))
              (func $last_entity_count_lifted (result u32)
                (canon lift (core func $guest_instance "last-entity-count")))
              (instance $lifecycle_export
                (export "on-load" (func $on_load_lifted))
                (export "on-unload" (func $on_unload_lifted))
                (export "last-entity-count" (func $last_entity_count_lifted))
              )
              (export "canary:plugin/lifecycle@0.1.0" (instance $lifecycle_export))
            )
        "#;

        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, WRITE_IMPORTING_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        // Grant ReadEcsWorld (irrelevant to this component, which doesn't
        // import ecs-read) but deliberately not WriteEcsWorld.
        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);

        let result = loader.instantiate(
            component,
            PathBuf::from("<test fixture>"),
            "write-test-plugin",
            &capabilities,
        );

        match result {
            Err(PluginError::WasmInstantiate { .. }) => {}
            Err(other) => {
                panic!("expected WasmInstantiate (an unsatisfied-import failure), got: {other}")
            }
            Ok(_) => panic!(
                "instantiation succeeded without WriteEcsWorld granted -- ecs-write's \
                 capability gating has regressed"
            ),
        }
    }

    /// A trapped guest entry must stay observable: `Plugin::on_load`
    /// is infallible by signature, so the trap itself is still
    /// swallowed — but a `tracing::warn!` event must fire, otherwise a
    /// crash-looping guest is invisible to the host.
    #[test]
    fn trapped_on_load_emits_a_warn_event() {
        const TRAPPING_WAT: &str = r#"
            (component
              (core module $guest
                (func (export "on-load")
                  (loop $forever
                    br $forever
                  )
                )
                (func (export "on-unload"))
                (func (export "last-entity-count") (result i32) (i32.const 0))
              )
              (core instance $guest_instance (instantiate $guest))
              (func $on_load_lifted (canon lift (core func $guest_instance "on-load")))
              (func $on_unload_lifted (canon lift (core func $guest_instance "on-unload")))
              (func $last_entity_count_lifted (result u32)
                (canon lift (core func $guest_instance "last-entity-count")))
              (instance $lifecycle_export
                (export "on-load" (func $on_load_lifted))
                (export "on-unload" (func $on_unload_lifted))
                (export "last-entity-count" (func $last_entity_count_lifted))
              )
              (export "canary:plugin/lifecycle@0.1.0" (instance $lifecycle_export))
            )
        "#;

        let low_fuel = ResourceBudget {
            fuel: 1_000,
            ..ResourceBudget::default()
        };
        let loader = WasmPluginLoader::new(CodecRegistry::new(), low_fuel)
            .expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TRAPPING_WAT)
            .expect("the fixture WAT should parse as a valid component");
        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<trap-warn test fixture>"),
                "trap-warn-test-plugin",
                &HashSet::new(),
            )
            .expect("instantiation itself should succeed -- the loop only runs once called");

        // Observability only: `on_load` still returns `()` (the trap
        // is swallowed, exactly as before) — the regression this
        // guards is the warn event going missing, not the signature.
        let events = captured_events(|| {
            plugin.on_load();
        });
        assert!(
            events
                .iter()
                .any(|(level, fields)| *level == tracing::Level::WARN
                    && fields.contains("on_load")
                    && fields.contains("trap-warn-test-plugin")),
            "a trapping on_load must emit a WARN event naming the entrypoint and plugin, \
             got: {events:?}"
        );
    }

    /// A component whose codec panics on decode: deterministic proof
    /// that a host-side panic never unwinds through Wasmtime frames —
    /// the guarded host body converts it to a pending trap, hands the
    /// guest its benign default, restores the reentrancy tripwire,
    /// and leaves the world untouched.
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct PanicDecode {
        value: f32,
    }

    impl CanaryComponent for PanicDecode {
        const SCHEMA_ID: &'static str = "canary-plugin-api-tests:panic-decode@1";
    }

    impl crate::component_value::ComponentValueCodec for PanicDecode {
        fn to_component_value(&self) -> ComponentValue {
            ComponentValue::Record(vec![("value".to_string(), PrimitiveValue::F32(self.value))])
        }

        fn from_component_value(
            _value: ComponentValue,
        ) -> Result<Self, crate::component_value::ComponentValueError> {
            panic!("deliberate host-side panic: the guard must catch this, not propagate it");
        }
    }

    #[test]
    fn host_panic_is_caught_converted_and_leaves_the_world_untouched() {
        let mut codecs = CodecRegistry::new();
        codecs.register::<PanicDecode>();
        let mut world = World::new();
        world.register_component::<PanicDecode>().unwrap();
        let entity = world.spawn();
        world.insert(entity, PanicDecode { value: 7.0 }).unwrap();

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::WriteEcsWorld);
        let mut slot = WorldSlot {
            world: Some(world),
            codecs: Arc::new(codecs),
            grant: ScopedGrant {
                capabilities,
                budget: ResourceBudget::default(),
            },
            limits: StoreLimitsBuilder::new().build(),
            plugin_name: "panic-test".to_string(),
            depth: 0,
            tick_before: None,
            pending_trap: None,
        };
        let handle = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation(),
        };

        let wrote = canary::plugin::ecs_write::Host::set(
            &mut slot,
            handle,
            PanicDecode::SCHEMA_ID.to_string(),
            WitComponentValue::Record(vec![("value".to_string(), WitPrimitiveValue::F32(1.0))]),
        );

        assert!(
            !wrote,
            "a panicked host body hands the guest its benign default"
        );
        assert_eq!(slot.depth, 0, "the reentrancy tripwire must be restored");
        assert!(
            slot.pending_trap.is_some(),
            "the caught panic must wait as a pending trap for the scoped entry point"
        );
        assert_eq!(
            slot.world
                .as_ref()
                .expect("slot holds the world")
                .get::<PanicDecode>(entity),
            Some(&PanicDecode { value: 7.0 }),
            "the panicked write must not have touched the world"
        );
        // A later host call in the same poisoned invocation
        // short-circuits instead of running user code again.
        assert!(!canary::plugin::ecs_write::Host::set(
            &mut slot,
            handle,
            PanicDecode::SCHEMA_ID.to_string(),
            WitComponentValue::Record(vec![("value".to_string(), WitPrimitiveValue::F32(1.0))]),
        ));
    }

    /// A guest that loops forever, for the fallible-seam tests below:
    /// no imports, so instantiation always succeeds and the trap lands
    /// in the scoped `on_load` instead.
    const SCOPED_TRAP_WAT: &str = r#"
        (component
          (core module $guest
            (func (export "on-load")
              (loop $forever
                br $forever
              )
            )
            (func (export "on-unload"))
            (func (export "last-entity-count") (result i32) (i32.const 0))
          )
          (core instance $guest_instance (instantiate $guest))
          (func $on_load_lifted (canon lift (core func $guest_instance "on-load")))
          (func $on_unload_lifted (canon lift (core func $guest_instance "on-unload")))
          (func $last_entity_count_lifted (result u32)
            (canon lift (core func $guest_instance "last-entity-count")))
          (instance $lifecycle_export
            (export "on-load" (func $on_load_lifted))
            (export "on-unload" (func $on_unload_lifted))
            (export "last-entity-count" (func $last_entity_count_lifted))
          )
          (export "canary:plugin/lifecycle@0.1.0" (instance $lifecycle_export))
        )
    "#;

    /// Compiles WAT to a temp `.wasm` file for `load_scoped`, which
    /// (like `load`) takes a path. Removed by the caller.
    fn write_temp_component(wat: &str, file_name: &str) -> PathBuf {
        let bytes = wat::parse_str(wat).expect("fixture WAT must parse to a valid component");
        let mut path = std::env::temp_dir();
        path.push(format!("canary-r34-{}-{file_name}", std::process::id()));
        std::fs::write(&path, bytes).expect("temp component must be writable");
        path
    }

    /// Required + trap → typed `GuestTrap` with the plugin name and
    /// phase, and the loaned world reclaimed — the runtime aborts
    /// startup on this, it never swallows it.
    #[test]
    fn load_scoped_required_trap_is_a_typed_error_and_reclaims_the_world() {
        let low_fuel = ResourceBudget {
            fuel: 1_000,
            ..ResourceBudget::default()
        };
        let loader =
            WasmPluginLoader::new(CodecRegistry::new(), low_fuel).expect("engine setup failed");
        let path = write_temp_component(SCOPED_TRAP_WAT, "required-trap.wasm");

        let mut world = Some(world_with_entities(4));
        let result = loader.load_scoped(
            &path,
            "required-trap-plugin",
            &HashSet::new(),
            PluginRequirement::Required,
            &mut world,
        );
        let _ = std::fs::remove_file(&path);

        match result {
            Err(PluginError::GuestTrap { plugin, phase, .. }) => {
                assert_eq!(plugin, "required-trap-plugin");
                assert_eq!(phase, PluginPhase::OnLoad);
            }
            Err(other) => panic!("expected GuestTrap, got: {other}"),
            Ok(_) => panic!("a required plugin that traps must fail the load"),
        }
        assert_eq!(
            world
                .as_ref()
                .expect("the loaned world must be reclaimed even on the trap path")
                .entity_count(),
            4
        );
    }

    /// Optional + trap → `SkippedOptional` with the cause retained,
    /// a `warn` emitted, the world reclaimed, and no live instance
    /// handed out — the run continues without the plugin.
    #[test]
    fn load_scoped_optional_trap_warns_skips_and_reclaims_the_world() {
        let low_fuel = ResourceBudget {
            fuel: 1_000,
            ..ResourceBudget::default()
        };
        let loader =
            WasmPluginLoader::new(CodecRegistry::new(), low_fuel).expect("engine setup failed");
        let path = write_temp_component(SCOPED_TRAP_WAT, "optional-trap.wasm");

        let mut world = Some(world_with_entities(4));
        let events = captured_events(|| {
            let (handle, outcome) = loader
                .load_scoped(
                    &path,
                    "optional-trap-plugin",
                    &HashSet::new(),
                    PluginRequirement::Optional,
                    &mut world,
                )
                .expect("an optional plugin that traps must skip, not fail");
            assert!(
                handle.is_none(),
                "a skipped plugin hands out no live instance"
            );
            match outcome {
                PluginOutcome::SkippedOptional { name, cause } => {
                    assert_eq!(name, "optional-trap-plugin");
                    assert!(
                        matches!(cause, PluginError::GuestTrap { .. }),
                        "the skip must retain the typed trap cause, got: {cause}"
                    );
                }
                PluginOutcome::Loaded { .. } => panic!("a trapping plugin must not report Loaded"),
            }
        });
        let _ = std::fs::remove_file(&path);

        assert!(
            events
                .iter()
                .any(|(level, fields)| *level == tracing::Level::WARN
                    && fields.contains("optional-trap-plugin")),
            "an optional skip must emit a WARN naming the plugin, got: {events:?}"
        );
        assert_eq!(
            world
                .as_ref()
                .expect("the loaned world must be reclaimed even on the skip path")
                .entity_count(),
            4
        );
    }

    /// Optional + clean load → a live instance plus `Loaded`: the
    /// seam's happy path, through a real scoped guest call.
    #[test]
    fn load_scoped_success_hands_out_a_live_instance_and_loaded() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup failed");
        let path = write_temp_component(TEST_COMPONENT_WAT, "scoped-ok.wasm");

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);
        let mut world = Some(world_with_entities(6));
        let (handle, outcome) = loader
            .load_scoped(
                &path,
                "scoped-ok-plugin",
                &capabilities,
                PluginRequirement::Optional,
                &mut world,
            )
            .expect("a clean optional load must succeed");
        let _ = std::fs::remove_file(&path);

        match outcome {
            PluginOutcome::Loaded { name } => assert_eq!(name, "scoped-ok-plugin"),
            PluginOutcome::SkippedOptional { .. } => panic!("a clean load must not skip"),
        }
        assert!(
            handle.is_some(),
            "a successful scoped load hands out the live instance"
        );
        assert!(
            world.is_some(),
            "the loaned world must move home after the scoped on_load"
        );
    }
}
