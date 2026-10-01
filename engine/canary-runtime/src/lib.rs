// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Reusable consumer runtime composition: the R-34 slice.
//!
//! The runtime owns the one active game [`World`],
//! the run context, phase order, and teardown for the duration of a
//! run. Tier A plugins touch the active world only through granted
//! host interfaces at explicit exclusive boundaries — see
//! [`Runtime::call_plugin_scoped`].
//!
//! **Configuration window.** The consumer assembles everything the
//! run needs **before** `start`: populate the `World` (spawn, insert,
//! register components, insert resources including [`RunContext`]'s
//! initial value), declare each plugin's [`ScopedGrant`] +
//! [`PluginRequirement`], and select codecs and budgets.
//! [`RuntimeBuilder::build`] **moves** the world in; after `build`
//! returns, the runtime is the world's single owner. During the run,
//! consumer code touches the world only through phase callbacks the
//! runtime invokes (scoped plugin calls, frame boundaries). After
//! `run` returns (or unwinds through cleanup),
//! [`Runtime::reclaim_world`] hands the world back for
//! inspection/testing.
//!
//! What this slice does **not** yet include (later design work, not
//! oversights): the full Initialize→Pace phase order with platform,
//! input, audio, UI, and render backends; a general fixed-step game
//! runner; pause/resume, restart-in-place, or hot reload. `run`
//! executes plugin loading, outer frames, and reverse-order unload —
//! the lifecycle boundary the first scoped-access proof needs. The
//! existing headless binary stays as-is until the backend-composition
//! design lands; migrating it onto this library is tracked follow-up
//! work, not part of R-34.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use canary_ecs::{Tick, World};
use canary_plugin_api::{
    CodecRegistry, Plugin, PluginError, PluginOutcome, PluginPhase, PluginRequirement,
    ResourceBudget, ScopedGrant, WasmComponentPlugin, WasmPluginLoader,
};
use thiserror::Error;

mod authored_spawn;
mod collab_session;
mod frame_driver;
mod input_frame;
mod simulation_snapshot;

pub use authored_spawn::{AuthoredSpawner, SpawnDecoder, SpawnReport, StagedInsert};
pub use collab_session::{
    serve_single_frame, split_tagged_body, tag_edit_body, CollabHostError, CollabServeError,
    CollabSessionHost, ServeOutcome, TAG_EDIT, TAG_SYNC,
};
pub use frame_driver::{DrivenFrame, FrameDriver, FrameParams};
pub use input_frame::{drive_input_frame, InputFrame, InputFrameOutput};
pub use simulation_snapshot::{
    decode_entity_ref, encode_entity_ref, ComponentBinding, ResourceBinding, RestoreReport,
    SimClock, SimComponent, SimResource, Simulation, SnapshotRegistry, StepReport,
};

/// Owned and advanced by the runtime; consumed (read) by
/// schedules/systems and scoped plugin host calls. Written at frame open
/// by [`Runtime::begin_frame`] and re-stamped by
/// [`Runtime::begin_sim_pass`] when the frame runs a simulation pass —
/// see both methods. Nothing but the runtime writes it; systems read it
/// via [`World::resource`].
///
/// Delivered as a resource rather than thread-local state (hidden
/// per-thread globals, untestable without running the loop) or a
/// schedule-run argument (churns every system signature for v0.0.13).
/// Per-frame overwrites stamp only this resource's own change tick —
/// no component probe (including the transform quiet-tick probe)
/// watches resource ticks — so event/presentation-only frames stay
/// quiet while `frame_index` advances monotonically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunContext {
    /// Unique per [`Runtime::run`] execution.
    pub run_id: u64,
    /// Outer-loop iterations, including event/presentation-only frames.
    pub frame_index: u64,
    /// ECS tick of the current (or most recent) scheduled pass.
    pub tick: Tick,
    /// Wall-clock outer-frame time; **not** simulation dt.
    pub frame_dt: Duration,
    /// Accumulated simulation time.
    pub sim_time: Duration,
    /// Simulation-step duration where a runner provides one.
    pub sim_step: Duration,
}

/// Typed runtime failures with primary-vs-cleanup accounting: cleanup
/// failures are retained as secondary context and never replace the
/// original failure.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// A required-service initialization failure aborts startup. The
    /// runtime shuts down everything that initialized successfully in
    /// reverse order first; those cleanup failures land in `cleanup`.
    #[error("required service `{service}` failed to initialize: {source}")]
    ServiceInit {
        /// Which service failed (`"wasm-plugin-loader"` or
        /// `"tier-a-plugin"` in this slice).
        service: &'static str,
        /// The original typed failure.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
        /// One message per failed cleanup step, in the order
        /// attempted. Secondary context only.
        cleanup: Vec<String>,
    },
    /// An unhandled fatal phase failure stops the loop, runs
    /// teardown, and is returned after cleanup with the same
    /// primary-vs-secondary split.
    #[error("fatal error in {phase}: {source}")]
    FatalPhase {
        /// Which phase failed (`"plugin-unload"` in this slice).
        phase: &'static str,
        /// The original typed failure.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
        /// One message per further failed cleanup step. Secondary
        /// context only.
        cleanup: Vec<String>,
    },
    /// One scoped plugin invocation failed with a typed trap. The
    /// loaned world was already reclaimed through the normal cleanup
    /// path before this was constructed.
    #[error("plugin `{plugin}` failed during `{phase:?}`: {source}")]
    PluginFailed {
        /// The loader-supplied plugin name.
        plugin: String,
        /// Which lifecycle callback failed.
        phase: PluginPhase,
        /// The typed trap.
        #[source]
        source: PluginError,
    },
    /// No scoped grant was ever registered for `plugin`, so there is
    /// nothing to invoke it with. Returned before any guest code can
    /// run.
    #[error("no scoped grant registered for plugin `{plugin}`")]
    UnknownPlugin {
        /// The name the caller asked for.
        plugin: String,
    },
}

/// One pre-run world-population step, staged by
/// [`RuntimeBuilder::with_world_populator`] and consumed by
/// [`RuntimeBuilder::build`] before ownership transfers.
type WorldPopulator = Box<dyn FnOnce(&mut World)>;

/// One plugin registration: where its bytes live, what it may touch,
/// and whether its failure aborts the run.
#[derive(Debug, Clone)]
pub struct PluginRegistration {
    /// Loader-supplied name, used for grant lookup and diagnostics.
    pub name: String,
    /// Path to the Tier A component artifact.
    pub path: PathBuf,
    /// The capability subset linked for this instance.
    pub grant: ScopedGrant,
    /// Whether failure aborts the run or degrades to a logged skip.
    pub requirement: PluginRequirement,
}

/// Assembles a [`Runtime`] during the pre-run configuration window.
/// See the crate-level docs for what must happen before `build`.
pub struct RuntimeBuilder {
    populators: Vec<WorldPopulator>,
    registrations: Vec<PluginRegistration>,
    codecs: CodecRegistry,
    budget: ResourceBudget,
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self {
            populators: Vec::new(),
            registrations: Vec::new(),
            codecs: CodecRegistry::new(),
            budget: ResourceBudget::default(),
        }
    }
}

impl RuntimeBuilder {
    /// Starts an empty configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-run only: staged into the world before the runtime takes
    /// ownership. Use for spawning, inserting, registering
    /// components, and inserting resources.
    pub fn with_world_populator(mut self, populate: impl FnOnce(&mut World) + 'static) -> Self {
        self.populators.push(Box::new(populate));
        self
    }

    /// Pre-run only: declares one Tier A plugin's bytes, scoped
    /// grant, and failure policy.
    pub fn with_plugin(
        mut self,
        name: impl Into<String>,
        path: impl AsRef<Path>,
        grant: ScopedGrant,
        requirement: PluginRequirement,
    ) -> Self {
        self.registrations.push(PluginRegistration {
            name: name.into(),
            path: path.as_ref().to_path_buf(),
            grant,
            requirement,
        });
        self
    }

    /// Pre-run only: the fixed component-type codecs Tier A plugins
    /// may access. Register everything before `build`; there is no
    /// post-build registration.
    pub fn with_codecs(mut self, codecs: CodecRegistry) -> Self {
        self.codecs = codecs;
        self
    }

    /// Pre-run only: the fuel + memory budget applied to every
    /// instance this runtime loads.
    pub fn with_budget(mut self, budget: ResourceBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Moves the world in. After this call the runtime owns it;
    /// populators run first, then ownership transfers.
    pub fn build(self, mut world: World) -> Result<Runtime, RuntimeError> {
        for populate in self.populators {
            populate(&mut world);
        }
        let loader = WasmPluginLoader::new(self.codecs, self.budget).map_err(|source| {
            RuntimeError::ServiceInit {
                service: "wasm-plugin-loader",
                source: Box::new(source),
                cleanup: Vec::new(),
            }
        })?;
        Ok(Runtime {
            world: Some(world),
            loader,
            registrations: self.registrations,
            loaded: Vec::new(),
            load_records: Vec::new(),
            unload_order: Vec::new(),
            run_id: 0,
            frame_index: 0,
            sim_time: Duration::ZERO,
        })
    }
}

/// One loaded instance held by the runtime across the run.
struct LoadedPlugin {
    name: String,
    plugin: WasmComponentPlugin,
    grant: ScopedGrant,
}

/// What one registration resolved to during
/// [`Runtime::load_all_plugins`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRecord {
    /// The loader-supplied plugin name.
    pub name: String,
    /// True when an optional plugin was skipped instead of loading.
    pub skipped: bool,
    /// The retained failure for a skip; `None` for a clean load.
    pub cause: Option<String>,
}

/// What one [`Runtime::run`] execution completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunReport {
    /// The run identity assigned to this execution.
    pub run_id: u64,
    /// Outer frames completed (all of them — unload runs after).
    pub frames_completed: u64,
}

/// The composition owner: one active world, schedule-adjacent phase
/// boundaries, and teardown, for the duration of a run.
///
/// Single-threaded at phase boundaries: scoped plugin calls take
/// `&mut self` and loan `world` (leaving `None`), so a call can
/// never overlap schedule execution or another plugin call.
pub struct Runtime {
    world: Option<World>,
    loader: WasmPluginLoader,
    registrations: Vec<PluginRegistration>,
    loaded: Vec<LoadedPlugin>,
    load_records: Vec<LoadRecord>,
    unload_order: Vec<String>,
    run_id: u64,
    frame_index: u64,
    sim_time: Duration,
}

impl Runtime {
    /// Loads every registered plugin in registration order through
    /// the fallible loader seam. A required failure unloads what
    /// already loaded (reverse order) and returns the typed primary
    /// failure with cleanup failures as secondary context; an
    /// optional failure is recorded as a skip and loading continues.
    pub fn load_all_plugins(&mut self) -> Result<(), RuntimeError> {
        for index in 0..self.registrations.len() {
            let registration = self.registrations[index].clone();
            let (handle, outcome) = match self.loader.load_scoped(
                &registration.path,
                &registration.name,
                &registration.grant.capabilities,
                registration.requirement,
                &mut self.world,
            ) {
                Ok(pair) => pair,
                Err(error) => {
                    let cleanup = self.unload_loaded_plugins();
                    return Err(RuntimeError::ServiceInit {
                        service: "tier-a-plugin",
                        source: Box::new(error),
                        cleanup,
                    });
                }
            };
            match outcome {
                PluginOutcome::Loaded { name } => {
                    self.load_records.push(LoadRecord {
                        name,
                        skipped: false,
                        cause: None,
                    });
                }
                PluginOutcome::SkippedOptional { name, cause } => {
                    self.load_records.push(LoadRecord {
                        name,
                        skipped: true,
                        cause: Some(cause.to_string()),
                    });
                }
            }
            if let Some(plugin) = handle {
                self.loaded.push(LoadedPlugin {
                    name: registration.name,
                    plugin,
                    grant: registration.grant,
                });
            }
        }
        Ok(())
    }

    /// Invoke one plugin lifecycle callback with scoped active-world
    /// access: the single choke point, exclusive with schedule
    /// execution by construction (`&mut self`, loan leaves `None`,
    /// world moves home before return on every path). Failure maps
    /// to [`RuntimeError::PluginFailed`] with the typed trap; the
    /// grant comes from this runtime's registrations, looked up by
    /// the plugin's own name.
    pub fn call_plugin_scoped(
        &mut self,
        plugin: &mut WasmComponentPlugin,
        phase: PluginPhase,
    ) -> Result<(), RuntimeError> {
        let name = plugin.name();
        let grant = self
            .loaded
            .iter()
            .find(|loaded| loaded.name == name)
            .map(|loaded| loaded.grant.clone())
            .or_else(|| {
                self.registrations
                    .iter()
                    .find(|registration| registration.name == name)
                    .map(|registration| registration.grant.clone())
            });
        let Some(grant) = grant else {
            return Err(RuntimeError::UnknownPlugin { plugin: name });
        };
        plugin
            .call_scoped(&mut self.world, &grant, phase)
            .map_err(|source| RuntimeError::PluginFailed {
                plugin: name,
                phase,
                source,
            })
    }

    /// Unloads every loaded plugin in reverse load order through
    /// scoped `on_unload` calls. Every unload is attempted even after
    /// a failure; the first failure becomes the typed primary with
    /// the rest as secondary context.
    pub fn unload_all_plugins(&mut self) -> Result<(), RuntimeError> {
        let mut failures = self.unload_loaded_plugins();
        if failures.is_empty() {
            Ok(())
        } else {
            let primary = failures.remove(0);
            Err(RuntimeError::FatalPhase {
                phase: "plugin-unload",
                source: Box::new(std::io::Error::other(primary)),
                cleanup: failures,
            })
        }
    }

    /// Attempts every loaded plugin's scoped `on_unload` in reverse
    /// load order, recording the order attempted. Returns one message
    /// per failed unload, in attempt order.
    fn unload_loaded_plugins(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        while let Some(mut loaded) = self.loaded.pop() {
            self.unload_order.push(loaded.name.clone());
            if let Err(error) =
                loaded
                    .plugin
                    .call_scoped(&mut self.world, &loaded.grant, PluginPhase::OnUnload)
            {
                failures.push(format!("{}: {error}", loaded.name));
            }
        }
        failures
    }

    /// Opens one outer frame: advances `frame_index` and overwrites
    /// the [`RunContext`] resource with the current counters —
    /// **before** any tick advance. Never advances the tick or
    /// `sim_time` itself (R-38: simulation time accumulates only in
    /// [`Runtime::begin_sim_pass`]); event/presentation-only frames call
    /// exactly this and nothing else, keeping `frame_index` monotonic
    /// while the tick stands still and stamping `sim_step` as zero.
    pub fn begin_frame(&mut self, frame_dt: Duration) {
        self.frame_index = self.frame_index.saturating_add(1);
        if let Some(world) = self.world.as_mut() {
            world.insert_resource(RunContext {
                run_id: self.run_id,
                frame_index: self.frame_index,
                tick: world.change_tick(),
                frame_dt,
                sim_time: self.sim_time,
                sim_step: Duration::ZERO,
            });
        }
    }

    /// Opens one simulation pass: folds `sim_step` into `sim_time`,
    /// advances the ECS tick exactly once (via
    /// [`Runtime::advance_tick_for_pass`]), and re-stamps the
    /// [`RunContext`] resource with the new tick. Call after
    /// [`Runtime::begin_frame`] and immediately before the schedule
    /// runs — never for event/presentation-only frames. The
    /// frame-open [`RunContext::frame_dt`] carries over; call after
    /// [`Runtime::begin_frame`] so it reflects the current frame.
    pub fn begin_sim_pass(&mut self, sim_step: Duration) {
        self.sim_time = self.sim_time.checked_add(sim_step).unwrap_or(Duration::MAX);
        self.advance_tick_for_pass();
        if let Some(world) = self.world.as_mut() {
            let frame_dt = world
                .resource::<RunContext>()
                .map(|context| context.frame_dt)
                .unwrap_or_default();
            let tick = world.change_tick();
            world.insert_resource(RunContext {
                run_id: self.run_id,
                frame_index: self.frame_index,
                tick,
                frame_dt,
                sim_time: self.sim_time,
                sim_step,
            });
        }
    }

    /// Advances the ECS tick exactly once for one scheduled world
    /// pass. Called by the pass driver immediately before the
    /// schedule runs — never at plugin phases, never per frame.
    pub fn advance_tick_for_pass(&mut self) {
        if let Some(world) = self.world.as_mut() {
            world.advance_tick();
        }
    }

    /// Runs the R-34 lifecycle: load all plugins, execute `frames`
    /// outer frames, then unload in reverse order. Returns the run
    /// report; on failure the world stays owned (inspectable via
    /// [`Runtime::world`], reclaimable via
    /// [`Runtime::reclaim_world`]).
    pub fn run(&mut self, frames: u64, frame_dt: Duration) -> Result<RunReport, RuntimeError> {
        static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);
        self.run_id = NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed);
        self.load_all_plugins()?;
        let mut frames_completed = 0;
        for _ in 0..frames {
            self.begin_frame(frame_dt);
            frames_completed += 1;
        }
        self.unload_all_plugins()?;
        Ok(RunReport {
            run_id: self.run_id,
            frames_completed,
        })
    }

    /// Hands the world back after the run (or after a failed build
    /// cleanup) for inspection/testing. `None` only if a loan is
    /// outstanding, which the scoped APIs make impossible to hold
    /// across calls.
    pub fn reclaim_world(self) -> Option<World> {
        self.world
    }

    /// The active world while owned, `None` only mid-loan.
    pub fn world(&self) -> Option<&World> {
        self.world.as_ref()
    }

    /// The current [`RunContext`], once a frame has written it.
    pub fn run_context(&self) -> Option<RunContext> {
        self.world.as_ref()?.resource::<RunContext>().copied()
    }

    /// Names of the currently loaded plugins, in load order.
    pub fn loaded_plugin_names(&self) -> Vec<String> {
        self.loaded
            .iter()
            .map(|loaded| loaded.name.clone())
            .collect()
    }

    /// What every registration resolved to, in registration order.
    pub fn load_records(&self) -> &[LoadRecord] {
        &self.load_records
    }

    /// Every unload attempted, in attempt (reverse-load) order.
    pub fn unload_order(&self) -> &[String] {
        &self.unload_order
    }
}
