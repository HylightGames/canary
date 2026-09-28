// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Loading Tier A components: the [`crate::WasmPluginLoader`], its structural
//! capability linking, and the fallible scoped-load seam.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use canary_ecs::World;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimitsBuilder};

use crate::capability::Capability;
use crate::component_value::CodecRegistry;
use crate::error::PluginError;
use crate::plugin::{PluginPhase, PluginRequirement};

use super::budget::ResourceBudget;
use super::codec::canary::plugin::{ecs_read, ecs_write};
use super::codec::TierAPlugin;
use super::instance::WasmComponentPlugin;
use super::slot::{ScopedGrant, WorldSlot};

/// Loader-level reporting for one plugin registration: the fallible
/// seam the runtime's plugin-loading service uses, so required
/// plugins fail startup with a typed error while optional ones
/// degrade to `warn` + continue. [`crate::Plugin::on_load`]/[`crate::Plugin::on_unload`]
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
    // `engine` is `pub(crate)` for the test module's AOT round-trip
    // (`Component::new`/`deserialize` directly); all other assembly
    // goes through the constructor and methods.
    pub(crate) engine: Engine,
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
    /// [`wasmtime::component::Linker`] when that capability is present in `capabilities`. A
    /// component whose WIT world imports `ecs-read` but wasn't granted
    /// [`crate::Capability::ReadEcsWorld`] (or `ecs-write` without
    /// [`crate::Capability::WriteEcsWorld`]) has no way to even *reach* that
    /// import — instantiation itself fails with
    /// [`crate::PluginError::WasmInstantiate`] (an unsatisfied-import error),
    /// before any of the component's own code runs, rather than
    /// succeeding and merely having a runtime call rejected.
    ///
    /// `name` is supplied by the caller rather than read from the
    /// component itself — see `wit/plugin.wit`'s module-level comment
    /// for why. `world` is moved into the resulting plugin's host
    /// slot as its starting loan state: this legacy owned-world path
    /// keeps single-owner tests and tooling working without a
    /// runtime. The active-game-world path loans per invocation
    /// through [`crate::WasmComponentPlugin::call_scoped`] instead — never
    /// mix the two on one instance (a scoped call on an instance
    /// still holding its owned world is refused with
    /// [`crate::PluginError::WorldUnavailable`]).
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
    /// [`crate::WasmComponentPlugin::call_scoped`], and the world is
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
    /// in [`crate::WasmPluginLoader::load`]. The returned plugin handle
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

    /// Maps one loader-level failure by [`crate::PluginRequirement`]:
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
    /// driven outside [`crate::WasmPluginLoader::load_scoped`] (tests,
    /// tooling, and [`crate::WasmComponentPlugin::call_scoped`] callers that
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

    /// Shared instantiation core for [`crate::WasmPluginLoader::load`] (which
    /// parses `component` from a file) and this module's tests (which
    /// parse one from an inline WAT fixture instead, so this slice's
    /// tests need nothing beyond `cargo test` — no external
    /// `cargo-component`/`wit-bindgen` toolchain, per
    /// `docs/roadmap/v0.0.3-roadmap.md`'s documented fallback).
    ///
    /// Starts with no loaned world in the slot: scoped access arrives
    /// per invocation via [`crate::WasmComponentPlugin::call_scoped`].
    pub(crate) fn instantiate(
        &self,
        component: Component,
        path_for_errors: PathBuf,
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
    ) -> Result<WasmComponentPlugin, PluginError> {
        let name = name.into();
        let mut linker = Linker::new(&self.engine);
        if capabilities.contains(&Capability::ReadEcsWorld) {
            ecs_read::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut WorldSlot| state)
                .map_err(|source| PluginError::WasmEngineSetup {
                    source: source.into(),
                })?;
        }
        if capabilities.contains(&Capability::WriteEcsWorld) {
            ecs_write::add_to_linker::<_, HasSelf<_>>(&mut linker, |state: &mut WorldSlot| state)
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
