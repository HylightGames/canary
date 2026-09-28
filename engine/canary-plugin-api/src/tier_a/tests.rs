use super::*;
use std::collections::HashSet;
use std::path::PathBuf;

use canary_ecs::World;
use wasmtime::component::Component;
use wasmtime::StoreLimitsBuilder;

use crate::capability::Capability;
use crate::component_value::{CodecRegistry, ComponentValue, PrimitiveValue};
use crate::error::PluginError;
use crate::plugin::{Plugin, PluginPhase, PluginRequirement};
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
