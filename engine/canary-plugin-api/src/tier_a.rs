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
//! **This module is a first slice of `v0.0.3`'s scope, not the whole of
//! it.** What it proves, end to end, against a real Wasmtime engine:
//!
//! - Loading and instantiating a real WASM Component Model artifact.
//! - The [`Plugin`] lifecycle (`on_load`/`on_unload`) working through a
//!   component, not just a native dylib.
//! - **Structural** capability enforcement: [`WasmPluginLoader::load`]
//!   only links a capability's host functions into the instance's
//!   [`Linker`] when that capability was actually granted. A component
//!   whose world imports an ungranted capability's interface has
//!   nothing to link against and fails at *instantiation* — before any
//!   of its own code runs — not merely a call that gets rejected. See
//!   the tests in this module for both directions proven directly.
//!
//! What it does **not** yet cover — real, separately scoped work, not
//! oversights:
//!
//! - **The full ECS read/write data ABI.** `wit/plugin.wit`'s `ecs-read`
//!   interface exposes exactly one method (`entity-count`) as a proof of
//!   the mechanism. The `SCHEMA_ID`-addressed, capped-value-type `get`/
//!   `set`/`has-component`/`is-valid-entity` surface
//!   `docs/roadmap/v0.0.3-roadmap.md` actually scopes is a distinct,
//!   larger piece of work: it needs a real component *data* ABI (how an
//!   arbitrary Rust component's fields cross the boundary), not just
//!   the identity resolution [`canary_ecs::World::type_id_for_schema`]
//!   already provides.
//! - **A resource budget** (memory limit, fuel/epoch execution budget).
//!   Every instance created here runs with Wasmtime's defaults, which
//!   are not a deliberate sandboxing decision.
//! - **AOT compilation.** Every load here goes through Wasmtime's
//!   default (JIT) compilation path.
//! - **A real, running [`canary_ecs::World`]'s scoped access.**
//!   `HostState` owns a `World` by value rather than borrowing an
//!   already-in-use one — seem its doc comment for why that's a
//!   deliberate simplification, not a finished design.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use canary_ecs::World;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

use crate::capability::Capability;
use crate::component_value::{CodecRegistry, ComponentValue, PrimitiveValue};
use crate::error::PluginError;
use crate::plugin::Plugin;

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

/// Host-side state for one loaded Tier A instance.
///
/// Owning `world: World` by value, rather than lending a reference into
/// an already-running one, is a deliberate simplification for this
/// first slice. Safely giving a WASM host call temporary, scoped access
/// to a `World` some *other* code is concurrently using is a real
/// design question of its own — one that belongs with whatever the
/// eventual scheduler/system-access design (the parallel job-stealing
/// scheduler, still unbuilt — see
/// `docs/architecture/core-runtime.md#threading--the-job-system`)
/// settles for *every* system's `World` access, not something to invent
/// ad hoc for Tier A specifically. This module proves the capability
/// mechanism works against a real `World`'s real data; it does not yet
/// prove that `World` can be safely shared with whatever else is
/// running at the same time.
struct HostState {
    world: World,
    codecs: Arc<CodecRegistry>,
}

impl canary::plugin::ecs_read::Host for HostState {
    fn entity_count(&mut self) -> u32 {
        // `World::entity_count` returns `usize`; ECS entity counts are
        // never going to approach `u32::MAX` in practice, and the WIT
        // interface committing to `u32` (rather than `u64`) here is a
        // deliberate choice for this proof, not a load-bearing one --
        // revisit if it ever matters.
        self.world.entity_count() as u32
    }

    fn is_valid_entity(&mut self, entity: WitEntityHandle) -> bool {
        self.world.is_alive(from_wit_entity(entity))
    }

    fn has_component(&mut self, entity: WitEntityHandle, schema_id: String) -> bool {
        let entity = from_wit_entity(entity);
        let Some(type_id) = self.world.type_id_for_schema(&schema_id) else {
            return false;
        };
        self.world.has_component_erased(entity, type_id)
    }

    fn get(&mut self, entity: WitEntityHandle, schema_id: String) -> Option<WitComponentValue> {
        let entity = from_wit_entity(entity);
        let type_id = self.world.type_id_for_schema(&schema_id)?;
        let erased = self.world.get_erased(entity, type_id)?;
        let value = self.codecs.to_value(type_id, erased)?;
        Some(to_wit_value(value))
    }
}

impl canary::plugin::ecs_write::Host for HostState {
    fn set(
        &mut self,
        entity: WitEntityHandle,
        schema_id: String,
        value: WitComponentValue,
    ) -> bool {
        let entity = from_wit_entity(entity);
        let Some(type_id) = self.world.type_id_for_schema(&schema_id) else {
            return false;
        };
        let Some(Ok(boxed)) = self.codecs.from_value(type_id, from_wit_value(value)) else {
            return false;
        };
        self.world.set_erased(entity, type_id, boxed)
    }
}

/// A loaded, instantiated Tier A plugin.
pub struct WasmComponentPlugin {
    name: String,
    store: Store<HostState>,
    bindings: TierAPlugin,
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
        // `crate::abi::PluginVTable`) — so a trap is silently ignored
        // rather than propagated, consistent with, not a new gap
        // relative to, the existing trait's shape.
        let _ = self
            .bindings
            .canary_plugin_lifecycle()
            .call_on_load(&mut self.store);
    }

    fn on_unload(&mut self) {
        // See `on_load` above for why a trap here is swallowed, not
        // propagated.
        let _ = self
            .bindings
            .canary_plugin_lifecycle()
            .call_on_unload(&mut self.store);
    }
}

impl WasmComponentPlugin {
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

/// Loads Tier A (sandboxed WASM Component Model) plugins. See the
/// module-level docs above for exactly what this does and does not yet
/// cover.
pub struct WasmPluginLoader {
    engine: Engine,
    codecs: Arc<CodecRegistry>,
}

impl WasmPluginLoader {
    /// Creates a loader with the Component Model enabled and `codecs`
    /// as the fixed set of component types Tier A plugins can access
    /// via `ecs-read`/`ecs-write` (if granted). Registration happens
    /// before construction, not after: `codecs` is shared cheaply
    /// (`Arc`) across every plugin this loader goes on to load, rather
    /// than cloned per instance, so there's no `&mut self` registration
    /// method to call once instances exist — register everything this
    /// loader will ever need up front.
    pub fn new(codecs: CodecRegistry) -> Result<Self, PluginError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|source| PluginError::WasmEngineSetup {
            source: source.into(),
        })?;
        Ok(Self {
            engine,
            codecs: Arc::new(codecs),
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
    /// state; see `HostState`'s doc comment for the real design
    /// question that sidesteps for now.
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
        self.instantiate(component, path.to_path_buf(), name, capabilities, world)
    }

    /// Shared instantiation core for [`WasmPluginLoader::load`] (which
    /// parses `component` from a file) and this module's tests (which
    /// parse one from an inline WAT fixture instead, so this slice's
    /// tests need nothing beyond `cargo test` — no external
    /// `cargo-component`/`wit-bindgen` toolchain, per
    /// `docs/roadmap/v0.0.3-roadmap.md`'s documented fallback).
    fn instantiate(
        &self,
        component: Component,
        path_for_errors: PathBuf,
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
        world: World,
    ) -> Result<WasmComponentPlugin, PluginError> {
        let mut linker = Linker::new(&self.engine);
        if capabilities.contains(&Capability::ReadEcsWorld) {
            canary::plugin::ecs_read::add_to_linker(&mut linker, |state: &mut HostState| state)
                .map_err(|source| PluginError::WasmEngineSetup {
                    source: source.into(),
                })?;
        }
        if capabilities.contains(&Capability::WriteEcsWorld) {
            canary::plugin::ecs_write::add_to_linker(&mut linker, |state: &mut HostState| state)
                .map_err(|source| PluginError::WasmEngineSetup {
                    source: source.into(),
                })?;
        }
        // `Filesystem`/`Network` have no corresponding Tier A interface
        // yet (see `wit/plugin.wit`'s module docs), so there is nothing
        // further to conditionally link for them at this stage.

        let mut store = Store::new(
            &self.engine,
            HostState {
                world,
                codecs: Arc::clone(&self.codecs),
            },
        );
        let (bindings, _instance) = TierAPlugin::instantiate(&mut store, &component, &linker)
            .map_err(|source| PluginError::WasmInstantiate {
                path: path_for_errors,
                source: source.into(),
            })?;

        Ok(WasmComponentPlugin {
            name: name.into(),
            store,
            bindings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_ecs::CanaryComponent;

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

    fn host_state_with_a_health_entity(value: f32) -> (HostState, canary_ecs::Entity) {
        let mut codecs = CodecRegistry::new();
        codecs.register::<Health>();

        let mut world = World::new();
        world.register_component::<Health>().unwrap();
        let entity = world.spawn();
        world.insert(entity, Health { value }).unwrap();

        (
            HostState {
                world,
                codecs: Arc::new(codecs),
            },
            entity,
        )
    }

    #[test]
    fn host_state_get_set_and_has_component_work_against_a_real_world() {
        let (mut host, entity) = host_state_with_a_health_entity(42.0);
        let handle = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation(),
        };
        let schema_id = Health::SCHEMA_ID.to_string();

        assert!(canary::plugin::ecs_read::Host::is_valid_entity(
            &mut host,
            handle.clone()
        ));
        assert!(canary::plugin::ecs_read::Host::has_component(
            &mut host,
            handle.clone(),
            schema_id.clone()
        ));

        let value =
            canary::plugin::ecs_read::Host::get(&mut host, handle.clone(), schema_id.clone())
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
        let wrote = canary::plugin::ecs_write::Host::set(&mut host, handle, schema_id, new_value);
        assert!(wrote);
        assert_eq!(
            host.world.get::<Health>(entity),
            Some(&Health { value: 99.0 })
        );
    }

    #[test]
    fn host_state_get_returns_none_for_an_unknown_schema_id() {
        let (mut host, entity) = host_state_with_a_health_entity(1.0);
        let handle = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation(),
        };

        let value =
            canary::plugin::ecs_read::Host::get(&mut host, handle, "no-such-schema".to_string())
                .map(from_wit_value);
        assert_eq!(value, None);
    }

    #[test]
    fn host_state_set_returns_false_and_changes_nothing_for_a_stale_entity() {
        let (mut host, entity) = host_state_with_a_health_entity(1.0);
        // A different generation for the same slot -- a stale handle,
        // per Entity::from_raw_parts's documented safety property.
        let stale = WitEntityHandle {
            index: entity.index(),
            generation: entity.generation() + 1,
        };

        let wrote = canary::plugin::ecs_write::Host::set(
            &mut host,
            stale,
            Health::SCHEMA_ID.to_string(),
            WitComponentValue::Record(vec![("value".to_string(), WitPrimitiveValue::F32(999.0))]),
        );

        assert!(!wrote);
        assert_eq!(
            host.world.get::<Health>(entity),
            Some(&Health { value: 1.0 }),
            "the real entity's data must be untouched by a set() targeting a stale handle"
        );
    }

    // -- WASM-boundary tests: the capability *linking* mechanism itself --

    #[test]
    fn granting_read_ecs_world_lets_the_component_read_real_world_state() {
        let loader =
            WasmPluginLoader::new(CodecRegistry::new()).expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let mut capabilities = HashSet::new();
        capabilities.insert(Capability::ReadEcsWorld);

        let mut plugin = loader
            .instantiate(
                component,
                PathBuf::from("<test fixture>"),
                "test-plugin",
                &capabilities,
                world_with_entities(5),
            )
            .expect("instantiation with the granted capability should succeed");

        assert_eq!(plugin.name(), "test-plugin");
        plugin.on_load();
        assert_eq!(
            plugin.last_entity_count(),
            5,
            "the component should have read the real World's entity count through \
             the ecs-read capability, not a fake or stale value"
        );
    }

    /// The point of this module: proves capability denial is
    /// *structural*. If this regressed to a runtime-checked-and-denied
    /// call instead, instantiation below would succeed and the failure
    /// (if any) would show up when calling `on_load`, not here.
    #[test]
    fn denying_read_ecs_world_makes_the_import_structurally_unreachable() {
        let loader =
            WasmPluginLoader::new(CodecRegistry::new()).expect("engine setup should not fail");
        let component = Component::new(&loader.engine, TEST_COMPONENT_WAT)
            .expect("the fixture WAT should parse as a valid component");

        let capabilities = HashSet::new(); // Nothing granted.

        let result = loader.instantiate(
            component,
            PathBuf::from("<test fixture>"),
            "test-plugin",
            &capabilities,
            world_with_entities(5),
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

        let loader =
            WasmPluginLoader::new(CodecRegistry::new()).expect("engine setup should not fail");
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
            world_with_entities(1),
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
}
