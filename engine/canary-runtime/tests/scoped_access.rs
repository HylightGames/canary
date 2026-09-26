// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! R-34 acceptance evidence for the runtime composition slice: the
//! builder configuration window, scoped plugin loading with
//! reverse-order cleanup and primary-failure preservation,
//! tick discipline at plugin phases, `RunContext` delivery, and the
//! quiet-path guarantee for read-only scoped calls.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use canary_ecs::World;
use canary_plugin_api::{
    Capability, CodecRegistry, PluginError, PluginPhase, PluginRequirement, ResourceBudget,
    ScopedGrant, WasmPluginLoader,
};
use canary_runtime::{RuntimeBuilder, RuntimeError};
use canary_scheduler::Schedule;
use canary_transform::{register_transform_propagation, GlobalTransform, Transform};

/// Minimal guest: imports `ecs-read`, caches `entity-count` on
/// `on-load`. Proves the loan reaches the active world.
const READER_WAT: &str = r#"
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

/// Guest whose `on-load` traps immediately (`unreachable`), with no
/// imports: instantiation always succeeds, so the failure lands in
/// the scoped call — deterministically, with no fuel tuning.
const TRAP_ON_LOAD_WAT: &str = r#"
    (component
      (core module $guest
        (func (export "on-load") unreachable)
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

/// Guest with a clean `on-load` but a trapping `on-unload`: proves
/// teardown-time cleanup failures are retained as secondary context
/// without replacing the primary startup failure.
const TRAP_ON_UNLOAD_WAT: &str = r#"
    (component
      (core module $guest
        (func (export "on-load"))
        (func (export "on-unload") unreachable)
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

fn write_temp_component(wat: &str, file_name: &str) -> PathBuf {
    let bytes = wat::parse_str(wat).expect("fixture WAT must parse to a valid component");
    let mut path = std::env::temp_dir();
    path.push(format!(
        "canary-runtime-r34-{}-{file_name}",
        std::process::id()
    ));
    std::fs::write(&path, bytes).expect("temp component must be writable");
    path
}

fn world_with_entities(count: usize) -> World {
    let mut world = World::new();
    for _ in 0..count {
        let _ = world.spawn();
    }
    world
}

fn read_grant() -> ScopedGrant {
    let mut capabilities = HashSet::new();
    capabilities.insert(Capability::ReadEcsWorld);
    ScopedGrant {
        capabilities,
        budget: ResourceBudget::default(),
    }
}

fn empty_grant() -> ScopedGrant {
    ScopedGrant {
        capabilities: HashSet::new(),
        budget: ResourceBudget::default(),
    }
}

/// The pre-run configuration window: populators run before ownership
/// transfers, and the world comes back intact for inspection.
#[test]
fn builder_populates_builds_and_reclaims() {
    let runtime = RuntimeBuilder::new()
        .with_world_populator(|world| {
            for _ in 0..3 {
                let _ = world.spawn();
            }
        })
        .build(World::new())
        .expect("build must succeed");

    assert!(
        runtime.run_context().is_none(),
        "no frame has written RunContext yet"
    );
    let world = runtime
        .reclaim_world()
        .expect("no loan is outstanding after build");
    assert_eq!(world.entity_count(), 3);
}

/// Observable lifecycle: loads in registration order, frames advance
/// the context, unloads run in reverse — all against the same owned
/// world the headless path would use.
#[test]
fn run_loads_frames_and_unloads_in_reverse_order() {
    let first = write_temp_component(READER_WAT, "order-first.wasm");
    let second = write_temp_component(READER_WAT, "order-second.wasm");

    let mut runtime = RuntimeBuilder::new()
        .with_plugin(
            "first-reader",
            &first,
            read_grant(),
            PluginRequirement::Required,
        )
        .with_plugin(
            "second-reader",
            &second,
            read_grant(),
            PluginRequirement::Required,
        )
        .build(world_with_entities(5))
        .expect("build must succeed");

    let report = runtime
        .run(3, Duration::from_millis(16))
        .expect("a clean run must succeed");
    let _ = std::fs::remove_file(&first);
    let _ = std::fs::remove_file(&second);

    assert_eq!(report.frames_completed, 3);
    assert_eq!(
        runtime
            .load_records()
            .iter()
            .filter(|record| !record.skipped)
            .count(),
        2
    );
    assert_eq!(
        runtime.unload_order(),
        &["second-reader".to_string(), "first-reader".to_string()],
        "teardown must run in reverse load order"
    );
    let context = runtime
        .run_context()
        .expect("frames must have written RunContext");
    assert_eq!(context.frame_index, 3);
    assert_eq!(
        runtime
            .world()
            .expect("world stays owned after a clean run")
            .entity_count(),
        5
    );
    let world = runtime
        .reclaim_world()
        .expect("no loan is outstanding after a clean run");
    assert_eq!(world.entity_count(), 5);
}

/// The choke point routes by registered grant and refuses unknown
/// plugins before any guest code can run.
#[test]
fn call_plugin_scoped_choke_routes_by_grant_and_rejects_unknown() {
    let reader = write_temp_component(READER_WAT, "choke-reader.wasm");

    let mut runtime = RuntimeBuilder::new()
        .with_plugin(
            "external-reader",
            &reader,
            read_grant(),
            PluginRequirement::Required,
        )
        .build(world_with_entities(4))
        .expect("build must succeed");

    let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
        .expect("loader setup must succeed");
    let component_bytes = std::fs::read(&reader).expect("temp component must be readable");

    let mut capabilities = HashSet::new();
    capabilities.insert(Capability::ReadEcsWorld);
    let mut external = loader
        .instantiate_from_bytes(&component_bytes, "external-reader", &capabilities)
        .expect("external instantiation must succeed");
    runtime
        .call_plugin_scoped(&mut external, PluginPhase::OnLoad)
        .expect("a registered grant must route through the choke point");
    assert_eq!(
        runtime
            .world()
            .expect("the loan must move home")
            .entity_count(),
        4
    );

    let mut ghost = loader
        .instantiate_from_bytes(&component_bytes, "ghost-plugin", &capabilities)
        .expect("ghost instantiation must succeed");
    match runtime.call_plugin_scoped(&mut ghost, PluginPhase::OnLoad) {
        Err(RuntimeError::UnknownPlugin { plugin }) => assert_eq!(plugin, "ghost-plugin"),
        Err(other) => panic!("expected UnknownPlugin, got: {other}"),
        Ok(()) => panic!("an unregistered plugin must be refused"),
    }
    let _ = std::fs::remove_file(&reader);
}

/// Startup abort: the required trap is the typed primary failure,
/// teardown still runs every loaded plugin in reverse, and a
/// teardown-time trap is retained as secondary context — never a
/// replacement for the primary.
#[test]
fn required_trap_aborts_startup_with_primary_preserved_and_cleanup_attempted() {
    let good = write_temp_component(READER_WAT, "abort-good.wasm");
    let fragile = write_temp_component(TRAP_ON_UNLOAD_WAT, "abort-fragile.wasm");
    let bad = write_temp_component(TRAP_ON_LOAD_WAT, "abort-bad.wasm");

    let mut runtime = RuntimeBuilder::new()
        .with_plugin(
            "good-reader",
            &good,
            read_grant(),
            PluginRequirement::Required,
        )
        .with_plugin(
            "fragile-unloader",
            &fragile,
            empty_grant(),
            PluginRequirement::Required,
        )
        .with_plugin(
            "load-trapper",
            &bad,
            empty_grant(),
            PluginRequirement::Required,
        )
        .build(world_with_entities(5))
        .expect("build must succeed");

    match runtime.run(1, Duration::from_millis(16)) {
        Err(RuntimeError::ServiceInit {
            service,
            source,
            cleanup,
        }) => {
            assert_eq!(service, "tier-a-plugin");
            let primary = source
                .downcast_ref::<PluginError>()
                .expect("the primary failure must stay typed");
            assert!(
                matches!(
                    primary,
                    PluginError::GuestTrap { plugin, phase, .. }
                    if plugin == "load-trapper" && *phase == PluginPhase::OnLoad
                ),
                "the primary failure must be the required load trap, got: {primary}"
            );
            assert_eq!(
                cleanup.len(),
                1,
                "the fragile unloader's teardown trap must be retained, got: {cleanup:?}"
            );
            assert!(
                cleanup[0].contains("fragile-unloader"),
                "secondary context must name the failed cleanup, got: {cleanup:?}"
            );
        }
        Err(other) => panic!("expected ServiceInit abort, got: {other}"),
        Ok(_) => panic!("a required load trap must abort startup"),
    }
    assert_eq!(
        runtime.unload_order(),
        &["fragile-unloader".to_string(), "good-reader".to_string()],
        "cleanup must attempt every loaded plugin in reverse order, even after a cleanup failure"
    );
    assert_eq!(
        runtime
            .world()
            .expect("the world must be reclaimed even on the abort path")
            .entity_count(),
        5
    );
    let _ = std::fs::remove_file(&good);
    let _ = std::fs::remove_file(&fragile);
    let _ = std::fs::remove_file(&bad);
}

/// Optional trap: recorded as a skip with its cause, and the run
/// succeeds without the plugin.
#[test]
fn optional_trap_skips_and_the_run_succeeds() {
    let bad = write_temp_component(TRAP_ON_LOAD_WAT, "skip-bad.wasm");

    let mut runtime = RuntimeBuilder::new()
        .with_plugin(
            "optional-trapper",
            &bad,
            empty_grant(),
            PluginRequirement::Optional,
        )
        .build(world_with_entities(2))
        .expect("build must succeed");

    let report = runtime
        .run(1, Duration::from_millis(16))
        .expect("an optional trap must skip, not fail");
    let _ = std::fs::remove_file(&bad);

    assert_eq!(report.frames_completed, 1);
    assert!(runtime.loaded_plugin_names().is_empty());
    let records = runtime.load_records();
    assert_eq!(records.len(), 1);
    assert!(records[0].skipped);
    assert!(
        records[0]
            .cause
            .as_ref()
            .is_some_and(|cause| cause.contains("trapped")),
        "the skip must retain the trap cause, got: {:?}",
        records[0].cause
    );
}

/// Tick discipline: frames alone never move the tick (event-only
/// frames stay tickless while `frame_index` advances); only the
/// explicit per-pass entry does.
#[test]
fn tick_advances_only_through_the_pass_entry() {
    let mut runtime = RuntimeBuilder::new()
        .build(world_with_entities(1))
        .expect("build must succeed");

    let tick_at_build = runtime.world().expect("world is owned").change_tick();
    runtime.begin_frame(Duration::from_millis(16));
    runtime.begin_frame(Duration::from_millis(16));
    let world = runtime.world().expect("world is owned");
    assert_eq!(
        world.change_tick(),
        tick_at_build,
        "event-only frames must not advance the ECS tick"
    );

    let context = runtime.run_context().expect("frames write RunContext");
    assert_eq!(context.frame_index, 2);
    assert_eq!(context.tick, tick_at_build);
    assert_eq!(context.sim_step, Duration::from_millis(16));

    runtime.advance_tick_for_pass();
    assert_ne!(
        runtime.world().expect("world is owned").change_tick(),
        tick_at_build,
        "only the per-pass entry advances the tick"
    );
}

/// Quiet-path preservation: a settled propagation baseline plus a
/// read-only scoped guest call (and `RunContext` overwrites) dirty
/// nothing the transform probe watches.
#[test]
fn read_only_scoped_call_and_context_writes_keep_the_quiet_path_quiet() {
    let reader = write_temp_component(READER_WAT, "quiet-reader.wasm");

    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, Transform::identity())
        .expect("transform insert must succeed");
    let mut schedule = Schedule::new();
    register_transform_propagation(&mut schedule);
    for _ in 0..3 {
        world.advance_tick();
        schedule.run(&mut world);
    }
    let baseline = world.change_tick();
    assert!(
        world
            .query_changed_since::<GlobalTransform>(baseline)
            .next()
            .is_none(),
        "the baseline must be settled-quiet before the scoped call"
    );

    let mut runtime = RuntimeBuilder::new()
        .with_plugin(
            "quiet-reader",
            &reader,
            read_grant(),
            PluginRequirement::Required,
        )
        .build(world)
        .expect("build must succeed");

    let loader = WasmPluginLoader::new(CodecRegistry::new(), ResourceBudget::default())
        .expect("loader setup must succeed");
    let component_bytes = std::fs::read(&reader).expect("temp component must be readable");
    let mut capabilities = HashSet::new();
    capabilities.insert(Capability::ReadEcsWorld);
    let mut external = loader
        .instantiate_from_bytes(&component_bytes, "quiet-reader", &capabilities)
        .expect("external instantiation must succeed");
    runtime
        .call_plugin_scoped(&mut external, PluginPhase::OnLoad)
        .expect("the read-only scoped call must succeed");
    runtime.begin_frame(Duration::from_millis(16));
    let _ = std::fs::remove_file(&reader);

    let world = runtime.world().expect("world stays owned");
    assert_eq!(
        world.change_tick(),
        baseline,
        "neither the scoped call nor the context write may advance the tick"
    );
    assert!(
        world
            .query_changed_since::<GlobalTransform>(baseline)
            .next()
            .is_none(),
        "a read-only guest call plus a RunContext overwrite must leave the quiet path quiet"
    );
}
