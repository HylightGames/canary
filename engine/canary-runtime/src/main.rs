//! Canary Engine headless boot-harness binary.
//!
//! Proves `canary-core`, `canary-platform`, `canary-ecs`, and
//! `canary-plugin-api` compile, link, and run together. This is **not** a
//! game and not a shipping runtime — it's the smallest possible program
//! that exercises this foundation's whole vertical slice end to end. See
//! `docs/roadmap/v0.0.1-roadmap.md`.

use canary_core::{App, Subsystem};
use canary_ecs::World;
use canary_platform::{HeadlessInput, HeadlessWindow, InputSource, Window, WindowDescriptor};
use canary_plugin_api::NativePluginLoader;

/// A demo component, just to prove `canary-ecs`'s insert/query path works
/// against a real (if trivial) game-shaped type.
#[derive(Debug, Clone, Copy)]
struct Position {
    x: f32,
    y: f32,
}

/// Wraps a `canary-ecs` [`World`] as a [`Subsystem`], demonstrating how a
/// real engine subsystem is expected to be registered with [`App`]. See
/// `docs/architecture/core-runtime.md#the-appengine-bootstrap`.
struct EcsSubsystem {
    world: World,
}

impl Subsystem for EcsSubsystem {
    fn name(&self) -> &str {
        "ecs"
    }

    fn tick(&mut self) {
        tracing::debug!(entities = self.world.entity_count(), "ecs tick");
    }

    fn shutdown(&mut self) {
        tracing::info!(entities = self.world.entity_count(), "ecs subsystem shutting down");
    }
}

fn main() -> anyhow::Result<()> {
    canary_core::init_logging();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Canary Engine booting");

    // --- Platform abstraction: prove the trait boundary compiles and runs
    // headless (see docs/architecture/platform-abstraction.md). No real
    // window exists yet, by design.
    let mut window = HeadlessWindow::new(WindowDescriptor::default());
    let mut input = HeadlessInput::new();
    window.poll_events();
    let _ = input.poll();
    tracing::info!(
        title = %window.descriptor().title,
        "platform layer initialized (headless — no real window backend yet)"
    );

    // --- ECS: spawn a few demo entities and prove both the insert and the
    // query path (see canary_ecs::World::query).
    let mut world = World::new();
    for i in 0..3 {
        let entity = world.spawn();
        world.insert(entity, Position { x: i as f32, y: 0.0 })?;
    }
    tracing::info!(entities = world.entity_count(), "spawned demo entities");
    for (entity, position) in world.query::<Position>() {
        tracing::debug!(%entity, x = position.x, y = position.y, "demo entity position");
    }

    // --- App bootstrap: register the ECS as a subsystem and run a few
    // ticks, proving canary-core's init/tick/shutdown lifecycle.
    let mut app = App::new();
    app.add_subsystem(EcsSubsystem { world });
    app.add_plugin_dir("plugins");
    app.run_for(3)?;

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
    drop(loader);

    tracing::info!("Canary Engine shutting down cleanly");
    Ok(())
}
