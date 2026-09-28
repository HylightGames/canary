// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! One loaded, instantiated Tier A plugin: the [`WasmComponentPlugin`]
//! handle, its [`crate::Plugin`] lifecycle impl, and the scoped-call
//! choke point.

use canary_ecs::World;
use wasmtime::Store;

use crate::error::PluginError;
use crate::plugin::{Plugin, PluginPhase};

use super::codec::TierAPlugin;
use super::slot::{ScopedGrant, WorldSlot};

/// A loaded, instantiated Tier A plugin.
pub struct WasmComponentPlugin {
    pub(crate) name: String,
    pub(crate) store: Store<WorldSlot>,
    pub(crate) bindings: TierAPlugin,
    /// The per-entry execution budget, re-armed by [`crate::WasmComponentPlugin::refuel`]
    /// before every guest entry point. Wasmtime fuel is consumed
    /// permanently (never self-replenishing), so without re-arming, a
    /// long-lived plugin would trap forever once its first budget ran
    /// out — every later call, including `on_unload`, would fail.
    pub(crate) fuel: u64,
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
    /// failure to [`crate::PluginError::GuestTrap`]. Never swallows: no
    /// logging here — the caller decides warn vs abort by
    /// [`crate::PluginRequirement`].
    ///
    /// Exclusive with schedule execution by construction: the caller
    /// holds the runtime `&mut` while this runs, and the schedule is
    /// not re-entered. No world borrow or handle escapes: host
    /// functions hand out only owned values (counts, bools, owned
    /// [`crate::ComponentValue`]s), never references.
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
    /// capability. Not part of the [`crate::Plugin`] trait surface — specific
    /// to this slice's illustrative `ecs-read` interface, not anything
    /// the trait itself commits to.
    #[cfg(test)]
    pub(crate) fn last_entity_count(&mut self) -> u32 {
        self.bindings
            .canary_plugin_lifecycle()
            .call_last_entity_count(&mut self.store)
            .expect("last-entity-count should not trap for a well-formed test fixture")
    }
}
