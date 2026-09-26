// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine headless boot-harness binary.
//!
//! Proves `canary-core`, `canary-platform`, `canary-ecs`, and
//! `canary-plugin-api` compile, link, and run together. This is **not** a
//! game and not a shipping runtime — it's the smallest possible program
//! that exercises this foundation's whole vertical slice end to end. See
//! `docs/roadmap/v0.0.1-roadmap.md`.

use canary_assets::{AssetStore, Mesh, Texture};
use canary_core::{App, Subsystem};
use canary_ecs::World;
use canary_physics::{register_physics_step, FrameDelta};
use canary_platform::{HeadlessInput, HeadlessWindow, InputSource, Window, WindowDescriptor};
use canary_plugin_api::NativePluginLoader;
use canary_render_ecs::{
    register_mesh_render_bake, register_render_bake, register_textured_render_bake,
};
use canary_scheduler::Schedule;
use canary_transform::register_transform_propagation;

/// A demo component, just to prove `canary-ecs`'s insert/query path works
/// against a real (if trivial) game-shaped type.
#[derive(Debug, Clone, Copy)]
struct Position {
    x: f32,
    y: f32,
}

/// Wraps a `canary-ecs` [`World`] plus its [`Schedule`] as a [`Subsystem`],
/// demonstrating how a real engine subsystem is expected to be registered
/// with [`App`]. See `docs/architecture/core-runtime.md#the-appengine-bootstrap`.
///
/// The schedule owns the per-tick ECS pipeline: physics step first,
/// transform propagation second, soup bake third, mesh bake fourth,
/// textured bake fifth. Registration order is the ordering mechanism —
/// the constructor below registers in exactly that order, and the
/// scheduler's solo-write staging turns the order into separate, ordered
/// stages:
///
/// - Physics MUST precede propagation: the physics system writes
///   `Transform` (stepped 2D poses) and propagation reads it. Reversed,
///   the bakes snapshot one-tick-stale globals every tick — proven by
///   `canary-physics`'s `step_before_propagation_sees_fresh_global` /
///   `reversed_registration_bakes_stale_global` order tests, which pin
///   both directions (mirroring the render-ecs precedent).
/// - Propagation precedes the soup bake (see [`register_render_bake`]'s
///   docs for why that order is load-bearing).
/// - The mesh bake appends file-loaded geometry onto the soup-baked frame
///   (a mesh-empty tick leaves it untouched), so soup-only worlds render
///   exactly as before.
/// - The textured bake runs last (see
///   [`register_textured_render_bake`]'s docs).
///
/// The GPU never enters this schedule: device, target, and pipeline stay
/// in `main()`'s frame scope and the baked frame is drawn explicitly
/// after `tick()`'s `schedule.run()` returns, per
/// `docs/architecture/rendering.md`'s "Extract, don't query" rule.
///
/// [`register_render_bake`]: canary_render_ecs::register_render_bake
/// [`register_textured_render_bake`]: canary_render_ecs::register_textured_render_bake
struct EcsSubsystem {
    world: World,
    schedule: Schedule,
}

impl EcsSubsystem {
    /// Builds the subsystem with the canonical system order:
    /// physics step first, propagation second, soup bake third, mesh
    /// bake fourth, textured bake fifth. Swapping physics after
    /// propagation bakes stale `GlobalTransform`s — proven by
    /// `canary-physics`'s `step_before_propagation_sees_fresh_global`
    /// order test plus its reversed-registration failure proof (which
    /// fails when physics is registered after propagation, mirroring the
    /// render-ecs precedent) — and swapping the soup bake after the mesh
    /// bake would let the soup overwrite the mesh vertices (see
    /// [`register_mesh_render_bake`]'s docs).
    ///
    /// [`register_mesh_render_bake`]: canary_render_ecs::register_mesh_render_bake
    ///
    /// Also ensures the [`AssetStore<Mesh>`] resource exists (inserting an
    /// empty one only when absent — never overwriting a pre-loaded store):
    /// the mesh bake resolves handles against it, and a missing store
    /// would silently skip every mesh entity rather than fail loudly.
    /// Physics needs no constructor seeding: [`register_physics_step`]'s
    /// system inserts its `PhysicsConfig` / `PhysicsClock` /
    /// `SimulationTime` / `RapierBackend` defaults on first tick (never
    /// overwriting), and [`EcsSubsystem::tick`] inserts the per-tick
    /// [`FrameDelta`] below.
    fn new(mut world: World) -> Self {
        if world.resource::<AssetStore<Mesh>>().is_none() {
            world.insert_resource(AssetStore::<Mesh>::new());
        }
        if world.resource::<AssetStore<Texture>>().is_none() {
            world.insert_resource(AssetStore::<Texture>::new());
        }
        let mut schedule = Schedule::new();
        register_physics_step(&mut schedule);
        register_transform_propagation(&mut schedule);
        register_render_bake(&mut schedule);
        register_mesh_render_bake(&mut schedule);
        register_textured_render_bake(&mut schedule);
        Self { world, schedule }
    }
}

impl Subsystem for EcsSubsystem {
    fn name(&self) -> &str {
        "ecs"
    }

    fn tick(&mut self, dt: std::time::Duration) {
        // The runner owns the logical ECS tick. Advance once before the
        // simulation schedule so every write in this run receives the
        // same new tick, including FrameDelta and system outputs.
        self.world.advance_tick();

        // Frame time reaches the fixed-step accumulator as an ordinary
        // resource: inserting unconditionally overwrites last tick's
        // delta (resources hold one value per type), so no stale dt can
        // survive across simulation runs.
        self.world.insert_resource(FrameDelta::new(dt));
        self.schedule.run(&mut self.world);
        tracing::debug!(
            entities = self.world.entity_count(),
            dt_ms = dt.as_secs_f64() * 1000.0,
            "ecs tick"
        );
    }

    fn shutdown(&mut self) {
        tracing::info!(
            entities = self.world.entity_count(),
            "ecs subsystem shutting down"
        );
    }
}

fn main() -> anyhow::Result<()> {
    canary_core::init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Canary Engine booting");

    // --- Platform abstraction: prove the trait boundary compiles and runs
    // headless (see docs/architecture/platform-abstraction.md). This
    // harness stays headless on purpose -- real `winit`-backed
    // windowing exists behind `canary-platform`'s `winit-backend`
    // feature (see v0.0.4), it just isn't what a deterministic boot
    // harness and CI want.
    let mut window = HeadlessWindow::new(WindowDescriptor::default());
    let mut input = HeadlessInput::new();
    window.poll_events();
    let _ = input.poll();
    tracing::info!(
        title = %window.descriptor().title,
        "platform layer initialized (headless by harness choice -- see canary-platform's winit-backend feature for real windowing)"
    );

    // --- ECS: spawn a few demo entities and prove both the insert and the
    // query path (see canary_ecs::World::query).
    let mut world = World::new();
    for i in 0..3 {
        let entity = world.spawn();
        world.insert(
            entity,
            Position {
                x: i as f32,
                y: 0.0,
            },
        )?;
    }
    tracing::info!(entities = world.entity_count(), "spawned demo entities");
    for (entity, position) in world.query::<Position>() {
        tracing::debug!(%entity, x = position.x, y = position.y, "demo entity position");
    }

    // --- App bootstrap: register the ECS as a subsystem and run a few
    // fixed-dt ticks, proving canary-core's init/tick/shutdown lifecycle
    // deterministically (see `App::run_for`'s own docs for why a boot
    // harness -- and CI -- want fixed, not real, dt).
    let mut app = App::new();
    app.add_subsystem(EcsSubsystem::new(world));
    app.add_plugin_dir("plugins");
    app.run_for(3, std::time::Duration::from_millis(16))?;

    // --- A second, separate App proves the real-timed loop added in
    // v0.0.9 (App::run) actually elapses real wall-clock time between
    // ticks, not just that it type-checks -- run for a short, fixed
    // real-world duration rather than an unbounded loop, so this boot
    // harness still terminates on its own.
    let mut timed_app = App::new();
    timed_app.add_subsystem(EcsSubsystem::new(World::new()));
    let start = std::time::Instant::now();
    timed_app.run(|| start.elapsed() < std::time::Duration::from_millis(50))?;

    // --- Plugin loader: no plugins ship with this foundation, but prove
    // the native (Tier B) loader is constructible and that checking a
    // plugin directory doesn't panic. See
    // docs/architecture/plugin-system.md.
    let loader = NativePluginLoader::new();
    let plugin_dir = std::path::Path::new("plugins");
    let has_plugins = plugin_dir.is_dir()
        && plugin_dir
            .read_dir()
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
    if has_plugins {
        tracing::warn!(
            "found files under `plugins/`, but automatic directory loading isn't wired up yet \
             (see docs/roadmap/v0.0.1-roadmap.md) -- load them explicitly via NativePluginLoader"
        );
    } else {
        tracing::info!(
            dir = %plugin_dir.display(),
            "no plugins found (none ship with this foundation; the loader itself is exercised by canary-plugin-api's own tests)"
        );
    }
    let _ = loader;

    tracing::info!("Canary Engine shutting down cleanly");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_ecs::Tick;
    use canary_transform::{GlobalTransform, Transform};

    #[test]
    fn runner_advances_once_per_tick_and_allows_quiet_propagation_to_settle() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::identity())
            .expect("transform insert must succeed");
        let mut subsystem = EcsSubsystem::new(world);
        let dt = std::time::Duration::from_millis(16);

        subsystem.tick(dt);
        let first_tick = subsystem.world.change_tick();
        assert_ne!(first_tick, Tick::default());
        assert!(subsystem.world.get::<GlobalTransform>(entity).is_some());

        // The first follow-up run settles the propagation baseline.
        subsystem.tick(dt);
        let settled_tick = subsystem.world.change_tick();
        assert_ne!(settled_tick, first_tick);

        // With no transform or hierarchy changes, the next run advances
        // time but does not rewrite GlobalTransform.
        subsystem.tick(dt);
        assert_ne!(subsystem.world.change_tick(), settled_tick);
        assert!(subsystem
            .world
            .query_changed_since::<GlobalTransform>(settled_tick)
            .next()
            .is_none());
    }
}
