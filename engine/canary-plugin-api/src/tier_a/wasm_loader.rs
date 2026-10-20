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
use std::io::Read as _;
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

/// Maximum bytes the Tier A loader reads from any single plugin
/// file (or accepts from an in-memory byte slice): 16 MiB comfortably
/// covers this slice's kilobyte fixtures and any realistic near-term
/// component (toolchain-compiled components measure in low megabytes
/// even with debug info) while bounding a hostile file's pre-parse
/// allocation before a byte of it is compiled.
///
/// The value is **provisional** pending measured calibration against
/// real plugin content, exactly like [`ResourceBudget::default`]'s 64
/// MiB memory budget and `canary-assets`' `MAX_ASSET_FILE_BYTES`. The
/// parse itself ([`wasmtime::component::Component::new`]) can
/// transiently allocate past the input size while compiling, so this
/// is a coarse input gate, not a total-allocation bound — the
/// fuel/memory budget still bounds what the instantiated plugin can do
/// once running.
pub const MAX_PLUGIN_BYTES: u64 = 16 * 1024 * 1024;

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
    ///
    /// Files past [`MAX_PLUGIN_BYTES`] are refused before parsing (see
    /// [`WasmPluginLoader::instantiate_from_bytes`] for why the parse
    /// itself must never see unbounded input); a missing or unreadable
    /// file reports the underlying I/O failure in the same
    /// [`crate::PluginError::WasmParse`] shape.
    pub fn load(
        &self,
        path: impl AsRef<Path>,
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
        world: World,
    ) -> Result<WasmComponentPlugin, PluginError> {
        let path = path.as_ref();
        let bytes = Self::read_component_bytes(path)?;
        let component = self.parse_bounded_component(&bytes, path)?;
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
    ///
    /// The [`MAX_PLUGIN_BYTES`] size gate applies exactly as in
    /// [`crate::WasmPluginLoader::load`]: an over-limit file is refused
    /// before parsing, reported through the same
    /// [`crate::PluginError::WasmParse`] shape a corrupt file yields.
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
        let bytes = Self::read_component_bytes(path)?;
        let component = self.parse_bounded_component(&bytes, path)?;
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
    ///
    /// The same [`MAX_PLUGIN_BYTES`] gate the file paths enforce
    /// applies here: over-limit input is refused before Wasmtime parses
    /// it, so this path cannot bypass the file paths' size bound.
    pub fn instantiate_from_bytes(
        &self,
        bytes: &[u8],
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
    ) -> Result<WasmComponentPlugin, PluginError> {
        let path = PathBuf::from("<in-memory component>");
        let component = self.parse_bounded_component(bytes, &path)?;
        self.instantiate(component, path, name, capabilities)
    }

    /// Reads the component file at `path` into memory, refusing files
    /// past [`MAX_PLUGIN_BYTES`] instead of handing an unbounded read
    /// to the parser. Mirrors `canary-assets`' `read_file_with_budget`:
    /// a metadata pre-check refuses an over-limit file without reading
    /// a byte (a sparse file reporting gigabytes fails here, cheaply),
    /// and a capped reader takes at most one byte past the limit, so a
    /// path whose metadata understates its contents (a file grown
    /// between the check and the read) still errors instead of
    /// allocating without bound. A missing or unreadable file yields
    /// the same [`PluginError::WasmParse`] shape with the underlying
    /// I/O failure as its source — the open runs before every size
    /// guard, so a missing file can never report a size refusal.
    fn read_component_bytes(path: &Path) -> Result<Vec<u8>, PluginError> {
        if let Ok(metadata) = std::fs::metadata(path) {
            let size = metadata.len();
            if size > MAX_PLUGIN_BYTES {
                return Err(Self::over_limit(path, size));
            }
        }
        let file = std::fs::File::open(path).map_err(|source| PluginError::WasmParse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        let mut capped = file.take(MAX_PLUGIN_BYTES.saturating_add(1));
        let mut bytes = Vec::new();
        capped
            .read_to_end(&mut bytes)
            .map_err(|source| PluginError::WasmParse {
                path: path.to_path_buf(),
                source: Box::new(source),
            })?;
        let actual = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if actual > MAX_PLUGIN_BYTES {
            return Err(Self::over_limit(path, actual));
        }
        Ok(bytes)
    }

    /// Parses already-read `bytes` as a component, refusing input past
    /// [`MAX_PLUGIN_BYTES`] before Wasmtime sees it. Both file loads
    /// (after [`Self::read_component_bytes`], which bounds the read
    /// itself) and direct [`Self::instantiate_from_bytes`] callers funnel
    /// through here, so the in-memory path cannot bypass the gate the
    /// file paths enforce.
    fn parse_bounded_component(&self, bytes: &[u8], path: &Path) -> Result<Component, PluginError> {
        let actual = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if actual > MAX_PLUGIN_BYTES {
            return Err(Self::over_limit(path, actual));
        }
        Component::new(&self.engine, bytes).map_err(|source| PluginError::WasmParse {
            path: path.to_path_buf(),
            source: source.into(),
        })
    }

    /// Builds the size-refusal error: [`PluginError::WasmParse`] naming
    /// `path`, with a source reporting both the observed size and the
    /// limit (mirroring `canary-assets`' `OverBudget` numbers) so the
    /// refusal stays distinguishable from a genuine parse failure
    /// without extending the error taxonomy.
    fn over_limit(path: &Path, actual: u64) -> PluginError {
        PluginError::WasmParse {
            path: path.to_path_buf(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                format!(
                    "refusing Tier A plugin component of {actual} bytes: \
                     over the {MAX_PLUGIN_BYTES}-byte size limit"
                ),
            )),
        }
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

#[cfg(test)]
mod size_gate_tests {
    use super::*;

    // The size gate under test is [`super::MAX_PLUGIN_BYTES`]
    // (brought into scope by the glob below): fixtures are kilobytes
    // and realistic near-term components measure in low megabytes, so
    // anything past that ceiling is hostile-or-broken, never
    // legitimate content.

    /// A sparse file reports `len` bytes from metadata while costing
    /// (almost) nothing on disk: the loader must refuse it from a
    /// metadata pre-check without ever allocating its contents.
    fn sparse_fixture(file_name: &str, len: u64) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "canary-size-gate-{}-{file_name}",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).expect("sparse fixture must be creatable");
        file.set_len(len).expect("sparse resize must succeed");
        drop(file);
        path
    }

    /// The size gate's typed refusal: [`PluginError::WasmParse`] naming
    /// `expected_path`, whose source reports the byte limit —
    /// distinguishable from a genuine Wasmtime parse failure, which
    /// never mentions it.
    #[track_caller]
    fn assert_size_refusal(err: &PluginError, expected_path: &Path) {
        match err {
            PluginError::WasmParse { path, source } => {
                assert_eq!(
                    path.as_path(),
                    expected_path,
                    "the refusal must name the offending file"
                );
                let detail = source.to_string();
                assert!(
                    detail.contains(&MAX_PLUGIN_BYTES.to_string()),
                    "the refusal source must report the \
                     {MAX_PLUGIN_BYTES}-byte limit, got a generic parse \
                     failure instead (no size gate): {detail}"
                );
            }
            other => panic!("an over-limit component must be refused, got: {other:?}"),
        }
    }

    #[test]
    fn load_refuses_an_oversized_sparse_file_before_parsing_it() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let path = sparse_fixture("load-over-limit.wasm", MAX_PLUGIN_BYTES + 1);
        let err = match loader.load(&path, "size-gate-test", &HashSet::new(), World::new()) {
            Err(err) => err,
            Ok(_) => panic!("an over-limit plugin file must be refused"),
        };
        let _ = std::fs::remove_file(&path);
        assert_size_refusal(&err, &path);
    }

    #[test]
    fn load_scoped_refuses_an_oversized_sparse_file_before_parsing_it() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let path = sparse_fixture("load-scoped-over-limit.wasm", MAX_PLUGIN_BYTES + 1);
        let mut world = Some(World::new());
        let err = match loader.load_scoped(
            &path,
            "size-gate-test",
            &HashSet::new(),
            PluginRequirement::Required,
            &mut world,
        ) {
            Err(err) => err,
            Ok(_) => panic!("an over-limit plugin file must be refused"),
        };
        let _ = std::fs::remove_file(&path);
        assert_size_refusal(&err, &path);
        assert!(
            world.is_some(),
            "a refused load must not consume the loaned world"
        );
    }

    #[test]
    fn instantiate_from_bytes_refuses_oversized_input_before_parsing_it() {
        let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
            .expect("engine setup should not fail");
        let oversized =
            vec![0u8; usize::try_from(MAX_PLUGIN_BYTES + 1).expect("the limit must fit in usize")];
        let err = match loader.instantiate_from_bytes(&oversized, "size-gate-test", &HashSet::new())
        {
            Err(err) => err,
            Ok(_) => panic!("over-limit bytes must be refused"),
        };
        match &err {
            PluginError::WasmParse { source, .. } => {
                let detail = source.to_string();
                assert!(
                    detail.contains(&MAX_PLUGIN_BYTES.to_string()),
                    "the refusal source must report the \
                     {MAX_PLUGIN_BYTES}-byte limit, got a generic parse \
                     failure instead (no size gate): {detail}"
                );
            }
            other => panic!("over-limit bytes must be refused, got: {other:?}"),
        }
    }
}
