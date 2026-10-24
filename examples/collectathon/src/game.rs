//! The `collectathon` simulation: velocity-driven movement on mapped
//! actions, touch collection, extended-range collection on the collect
//! edge, room reset on the reset edge or HUD button, and UI intents.
//!
//! The player is a physics body, never a hand-integrated pose: the input
//! system writes [`Velocity`](canary_physics::Velocity) and the solver
//! integrates it into [`Transform`](canary_transform::Transform). The
//! build pass (`hud`) and the intents stage below agree on the reset
//! button id, never on a live `World` borrow — a widget callback never
//! touches the world; the intent lands here, in the pass.
//!
//! Registration order is load-bearing and owned by the binaries (see
//! [`register_gameplay`](crate::game::register_gameplay)): physics steps first, transform propagation
//! recomputes globals from just-stepped locals, gameplay writers run
//! next, and the audio trigger actuates last so it never plays
//! one-tick-stale intents.

use canary_assets::{AssetHandle, Mesh, Sound, Texture};
use canary_audio::{AudioListener, AudioSource, SourceState};
use canary_ecs::{EcsError, World};
use canary_input::{ActionId, SimulationInput};
use canary_physics::{Collider, GravityScale, RigidBody, Velocity};
use canary_render_ecs::{MeshRenderable, TexturedRenderable};
use canary_runtime::SpawnReport;
use canary_scheduler::{Schedule, SystemAccess};
use canary_transform::{GlobalTransform, Transform};
use canary_ui_core::{UiIntent, UiIntents};

use crate::hud::RESET_BUTTON;
use crate::state::GameStats;
use crate::{Actions, Pickup, Player, Score};

/// Fixed simulation step in milliseconds. The windowed game accumulates
/// measured wall time into whole steps (bounded per presentation frame);
/// the headless twin runs one explicit step per frame, deterministically.
pub const STEP_MS: u64 = 16;
/// Player speed in logical pixels per second, written as a velocity — the
/// solver integrates it, so diagonal input is axis-composed, not
/// normalized.
pub const SPEED_PX_PER_SEC: f32 = 120.0;
/// Half-extent of the room in logical pixels; the pose sync clamps here so
/// the state the HUD shows is the state the screen shows.
pub const ARENA_HALF_PX: f32 = 140.0;
/// Touch-collection radius in logical pixels.
pub const COLLECT_RADIUS_PX: f32 = 12.0;
/// Collect-edge radius in logical pixels (edge-triggered, wider).
pub const COLLECT_EDGE_RADIUS_PX: f32 = 48.0;
/// Player collider half-extent in logical pixels: a cuboid the capsule
/// overlap test orbits around.
pub const PLAYER_HALF_PX: f32 = 6.0;

/// Registers the gameplay stages: movement (velocity writes), pose sync,
/// touch collection, collect-on-edge, reset-on-edge, and UI intents.
///
/// Does NOT register the engine-owned ends of the frame: the binary
/// registers [`register_physics_step`](canary_physics::register_physics_step)
/// FIRST and
/// [`register_transform_propagation`](canary_transform::register_transform_propagation)
/// second, then calls this, then registers
/// [`register_audio_trigger`](canary_audio::register_audio_trigger) LAST —
/// after the gameplay writers whose [`AudioSource`] transitions it
/// actuates, and after propagation so positional voices attenuate from
/// fresh globals. Reordering any of those three bakes stale state.
pub fn register_gameplay(schedule: &mut Schedule, actions: &Actions) {
    register_move_player(schedule, actions);
    register_sync_pose(schedule);
    register_collect_on_touch(schedule);
    register_collect_on_edge(schedule, actions.collect);
    register_reset_on_edge(schedule, actions.reset);
    register_apply_ui_intents(schedule);
}

/// Drives the player body through its velocity: held input writes
/// `direction * SPEED` every pass, released input writes zero — the else
/// branch is the load-bearing half (a dynamic body keeps its last
/// velocity until told otherwise, so skipping the write would glide).
/// The solver owns integration; this system never touches a pose.
pub fn register_move_player(schedule: &mut Schedule, actions: &Actions) {
    let (up, down, left, right) = (actions.up, actions.down, actions.left, actions.right);
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .reads::<Player>()
            .writes::<Velocity>(),
        move |world: &mut World| {
            let snapshot = world
                .resource::<SimulationInput>()
                .expect("driver publishes the snapshot before the schedule runs");
            let (dx, dy) = (
                f32::from(snapshot.is_down(right)) - f32::from(snapshot.is_down(left)),
                f32::from(snapshot.is_down(down)) - f32::from(snapshot.is_down(up)),
            );
            let velocity = Velocity {
                linvel: [dx * SPEED_PX_PER_SEC, dy * SPEED_PX_PER_SEC],
                angvel: 0.0,
            };
            let entities: Vec<_> = world.query::<Player>().map(|(entity, _)| entity).collect();
            for entity in entities {
                if let Some(slot) = world.get_mut::<Velocity>(entity) {
                    slot.linvel = velocity.linvel;
                    slot.angvel = 0.0;
                } else {
                    let _ = world.insert(entity, velocity);
                }
            }
        },
    );
}

/// Copies the solver-owned pose back onto the game-owned [`Player`]
/// position, clamped to the arena — and clamps the solver pose to the
/// same bound so the body cannot drift invisibly offscreen.
///
/// Runs after physics and propagation (see [`register_gameplay`]), so the
/// collection systems below read this frame's stepped position through
/// the ordinary [`Player`] component, never a second pose source.
pub fn register_sync_pose(schedule: &mut Schedule) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads::<Transform>()
            .writes::<Transform>()
            .writes::<Player>(),
        |world: &mut World| {
            let entities: Vec<_> = world.query::<Player>().map(|(entity, _)| entity).collect();
            for entity in entities {
                let Some(pose) = world.get::<Transform>(entity).copied() else {
                    continue;
                };
                let (x, y) = (
                    pose.translation.x.clamp(-ARENA_HALF_PX, ARENA_HALF_PX),
                    pose.translation.y.clamp(-ARENA_HALF_PX, ARENA_HALF_PX),
                );
                if let Some(pose_mut) = world.get_mut::<Transform>(entity) {
                    pose_mut.translation.x = x;
                    pose_mut.translation.y = y;
                }
                if let Some(player) = world.get_mut::<Player>(entity) {
                    player.x = x;
                    player.y = y;
                }
            }
        },
    );
}

/// Collects pickups the player touches: within [`COLLECT_RADIUS_PX`] of a
/// player, an uncollected pickup flips to collected, its voice flips to
/// playing, and the score banks one point.
pub fn register_collect_on_touch(schedule: &mut Schedule) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads::<Player>()
            .writes::<Pickup>()
            .writes::<MeshRenderable>()
            .writes::<TexturedRenderable>()
            .writes::<AudioSource>()
            .writes::<Score>()
            .writes_resource::<GameStats>(),
        |world: &mut World| {
            collect_within(world, COLLECT_RADIUS_PX, true);
        },
    );
}

/// Collects once per collect press edge, however long the control stays
/// held: pickups within [`COLLECT_EDGE_RADIUS_PX`] of any player collect
/// on the edge pass only.
pub fn register_collect_on_edge(schedule: &mut Schedule, collect: ActionId) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .reads::<Player>()
            .writes::<Pickup>()
            .writes::<MeshRenderable>()
            .writes::<TexturedRenderable>()
            .writes::<AudioSource>()
            .writes::<Score>()
            .writes_resource::<GameStats>(),
        move |world: &mut World| {
            if !world
                .resource::<SimulationInput>()
                .expect("driver publishes the snapshot before the schedule runs")
                .was_pressed(collect)
            {
                return;
            }
            collect_within(world, COLLECT_EDGE_RADIUS_PX, true);
        },
    );
}

/// Shared touch/edge collector: marks pickups within `radius` taken,
/// hides their drawable (keeping the entity and its voice alive), flips
/// their voices to playing, and banks one point plus one stat per pickup.
/// Already-taken pickups never re-fire, so holding the player inside a
/// collected shard banks nothing further.
fn collect_within(world: &mut World, radius: f32, play_sound: bool) {
    let players: Vec<(f32, f32)> = world
        .query::<Player>()
        .map(|(_, player)| (player.x, player.y))
        .collect();
    if players.is_empty() {
        return;
    }
    if world.resource::<GameStats>().is_none() {
        world.insert_resource(GameStats::default());
    }
    let radius_sq = radius * radius;
    let mut newly_collected = 0u32;
    let pickups: Vec<_> = world.query::<Pickup>().map(|(entity, _)| entity).collect();
    for entity in pickups {
        let touched = match world.get::<Pickup>(entity) {
            Some(pickup) if !pickup.collected => players.iter().any(|(px, py)| {
                let (dx, dy) = (pickup.x - px, pickup.y - py);
                dx * dx + dy * dy <= radius_sq
            }),
            _ => continue,
        };
        if !touched {
            continue;
        }
        if let Some(pickup) = world.get_mut::<Pickup>(entity) {
            pickup.collected = true;
            newly_collected += 1;
        }
        if play_sound {
            if let Some(source) = world.get_mut::<AudioSource>(entity) {
                source.state = SourceState::Playing;
            }
        }
        // Hide the collected pickup by dropping both drawables, never by
        // despawning: the same entity's AudioSource must survive to play
        // the collection chime, and reset_room re-inserts both renderables
        // from PickupVisual. The bridge extracts whatever carries a
        // renderable, so this is the visibility filter it has no
        // concept of.
        let _ = world.remove::<MeshRenderable>(entity);
        let _ = world.remove::<TexturedRenderable>(entity);
    }
    if newly_collected == 0 {
        return;
    }
    let scores: Vec<_> = world.query::<Score>().map(|(entity, _)| entity).collect();
    for entity in scores {
        if let Some(score) = world.get_mut::<Score>(entity) {
            score.points = score.points.saturating_add(newly_collected);
        }
    }
    if let Some(stats) = world.resource_mut::<GameStats>() {
        stats.collected = stats.collected.saturating_add(newly_collected);
    }
}

/// Resets the room once per reset press edge, however long R stays held.
pub fn register_reset_on_edge(schedule: &mut Schedule, reset: ActionId) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .reads::<PickupVisual>()
            .writes::<Pickup>()
            .writes::<MeshRenderable>()
            .writes::<TexturedRenderable>()
            .writes::<AudioSource>()
            .writes::<Score>()
            .writes_resource::<GameStats>(),
        move |world: &mut World| {
            if !world
                .resource::<SimulationInput>()
                .expect("driver publishes the snapshot before the schedule runs")
                .was_pressed(reset)
            {
                return;
            }
            reset_room(world);
        },
    );
}

/// Applies UI-originated intents at the declared simulation boundary: a
/// widget callback never touches the world; the intent lands here, in the
/// pass. Each reset-button activation restores the room (see
/// [`reset_room`]).
pub fn register_apply_ui_intents(schedule: &mut Schedule) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<UiIntents>()
            .reads::<PickupVisual>()
            .writes::<Pickup>()
            .writes::<MeshRenderable>()
            .writes::<TexturedRenderable>()
            .writes::<AudioSource>()
            .writes::<Score>()
            .writes_resource::<GameStats>(),
        |world: &mut World| {
            let reset = world
                .resource::<UiIntents>()
                .expect("driver publishes intents before the schedule runs")
                .intents
                .contains(&UiIntent::ButtonPressed(RESET_BUTTON));
            if !reset {
                return;
            }
            reset_room(world);
        },
    );
}

/// Restores the room: every pickup un-taken, every pickup drawable
/// re-inserted from its [`PickupVisual`], every pickup voice stopped,
/// every score zeroed, collected stats zeroed. Totals
/// ([`GameStats::goal`]) are room content, not run state, so they
/// survive the reset.
pub fn reset_room(world: &mut World) {
    let pickups: Vec<_> = world.query::<Pickup>().map(|(entity, _)| entity).collect();
    for entity in pickups {
        if let Some(pickup) = world.get_mut::<Pickup>(entity) {
            pickup.collected = false;
        }
        if let Some(source) = world.get_mut::<AudioSource>(entity) {
            source.state = SourceState::Stopped;
        }
        if let Some(visual) = world.get::<PickupVisual>(entity).copied() {
            let _ = world.insert(entity, MeshRenderable::new(visual.mesh, visual.color));
            let _ = world.insert(
                entity,
                TexturedRenderable::new(visual.sprite_mesh, visual.sprite),
            );
        }
    }
    let scores: Vec<_> = world.query::<Score>().map(|(entity, _)| entity).collect();
    for entity in scores {
        if let Some(score) = world.get_mut::<Score>(entity) {
            score.points = 0;
        }
    }
    if let Some(stats) = world.resource_mut::<GameStats>() {
        stats.collected = 0;
    }
}

/// Registers the game-owned component schemas so the authored spawner can
/// validate the room document against the world before the first spawn.
pub fn register_game_components(world: &mut World) -> Result<(), EcsError> {
    world.register_component::<Player>()?;
    world.register_component::<Pickup>()?;
    world.register_component::<Score>()?;
    Ok(())
}

/// Simulation-owned attachments the authored room does not name: the
/// pickup voice handles.
#[derive(Debug, Clone, Copy)]
pub struct SimHandles {
    /// Sound every pickup voice plays on collection.
    pub pickup_sound: AssetHandle<Sound>,
}

/// What an uncollected pickup draws: the mesh plus its flat tint, and
/// the sprite mesh plus texture drawn over it.
///
/// The render attachment stores this alongside [`MeshRenderable`] and
/// [`TexturedRenderable`] so collection can remove both drawables
/// without losing the data [`reset_room`] needs to restore them. Kept
/// in the sample, not the engine: `extract_mesh_scene` has no
/// visibility concept and
/// [`RenderItem`](canary_render_ecs::RenderItem) carries no entity id to
/// filter by, so hiding a collected pickup is the scene's decision.
/// Both drawables must go: the windowed bridge draws the textured
/// scene whenever the camera resolves, so dropping only the flat quad
/// leaves the sprite on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickupVisual {
    /// Mesh the pickup draws.
    pub mesh: AssetHandle<Mesh>,
    /// Flat RGB tint of the pickup's triangles.
    pub color: [f32; 3],
    /// Sprite mesh drawn over the flat quad.
    pub sprite_mesh: AssetHandle<Mesh>,
    /// Sprite texture drawn over the flat quad.
    pub sprite: AssetHandle<Texture>,
}

/// Attaches the simulation-owned components the spawner cannot decode:
/// the player becomes a gravity-free dynamic cuboid body with a seeded
/// velocity, a solver pose copied from its authored position, and the
/// scene's [`AudioListener`]; every spawned pickup gains a stopped,
/// non-positional voice on `handles.pickup_sound` (pickup chimes are
/// non-diegetic — distance must not quiet them); and the run stats zero
/// out over the spawned pickup count.
///
/// `report` is the spawner's placement map (`"player"`, `"goal"`,
/// `"pickup_a"` …). A missing `"player"` is a content error reported as
/// [`EcsError::StaleOrUnknownEntity`]; missing pickups are tolerated and
/// simply lower the goal total.
pub fn attach_simulation_components(
    world: &mut World,
    report: &SpawnReport,
    handles: &SimHandles,
) -> Result<GameStats, EcsError> {
    let player = report
        .entity("player")
        .ok_or(EcsError::StaleOrUnknownEntity)?;
    let (x, y) = world
        .get::<Player>(player)
        .map(|player| (player.x, player.y))
        .ok_or(EcsError::StaleOrUnknownEntity)?;
    let pose = Transform::from_translation(glam::Vec3::new(x, y, 0.0));
    world.insert(player, RigidBody::dynamic())?;
    world.insert(player, Collider::cuboid([PLAYER_HALF_PX, PLAYER_HALF_PX]))?;
    world.insert(player, Velocity::zero())?;
    world.insert(player, GravityScale::none())?;
    world.insert(player, pose)?;
    world.insert(player, GlobalTransform::from_matrix(pose.to_matrix()))?;
    // The player's propagated pose is the scene's ear: the trigger reads
    // the first listener entity's GlobalTransform (see
    // `canary_audio::audio_trigger_system`), falling back to the origin
    // when none exists.
    world.insert(player, AudioListener)?;

    let mut total = 0u32;
    for local in ["goal", "pickup_a", "pickup_b", "pickup_c"] {
        let Some(entity) = report.entity(local) else {
            continue;
        };
        if world.get::<Pickup>(entity).is_none() {
            continue;
        }
        world.insert(
            entity,
            AudioSource::non_positional(handles.pickup_sound).with_volume(1.0),
        )?;
        total += 1;
    }
    let stats = GameStats {
        collected: 0,
        goal: total,
    };
    world.insert_resource(stats);
    Ok(stats)
}

/// World-space scale of the player's file-loaded quad: the fixtures are
/// unit quads (half-extent `0.5`), while the arena half-extent is
/// [`ARENA_HALF_PX`] — so the ortho bake maps this to an NDC half-height
/// of `0.5 * 16 / 140 ≈ 0.057`, the legacy player quad's `0.06` read
/// through the bridge.
pub const PLAYER_MESH_SCALE: f32 = 16.0;

/// World-space scale of each pickup's file-loaded mesh: bakes to an NDC
/// half-height of `0.5 * 10 / 140 ≈ 0.036`, the legacy pickup diamond's
/// `0.035` read through the bridge.
pub const PICKUP_MESH_SCALE: f32 = 10.0;

/// Disk handles the windowed sample binds to renderables: the same
/// [`load_game_assets`](crate::assets::load_game_assets) outputs the
/// simulation seam already consumes, repackaged so the render attachment
/// names exactly which bytes each entity draws.
///
/// Kept separate from [`SimHandles`] on purpose: the headless twin, the
/// replication server, and the collab harnesses construct [`SimHandles`]
/// literally, and none of them draws — growing that struct would break
/// every non-render consumer for pixels only the windowed game shows.
#[derive(Debug, Clone, Copy)]
pub struct RenderHandles {
    /// Player and shard flat underlay geometry
    /// ([`QUAD_MESH`](crate::assets::QUAD_MESH)).
    pub player_mesh: AssetHandle<Mesh>,
    /// Goal marker geometry ([`BOX_MESH`](crate::assets::BOX_MESH)).
    pub goal_mesh: AssetHandle<Mesh>,
    /// Player sprite geometry
    /// ([`PLAYER_QUAD_MESH`](crate::assets::PLAYER_QUAD_MESH)).
    pub player_sprite_mesh: AssetHandle<Mesh>,
    /// Shard sprite geometry
    /// ([`SHARD_QUAD_MESH`](crate::assets::SHARD_QUAD_MESH)).
    pub shard_sprite_mesh: AssetHandle<Mesh>,
    /// Goal sprite geometry
    /// ([`GOAL_QUAD_MESH`](crate::assets::GOAL_QUAD_MESH)).
    pub goal_sprite_mesh: AssetHandle<Mesh>,
    /// Sprite atlas
    /// ([`SPRITE_ATLAS_PNG`](crate::assets::SPRITE_ATLAS_PNG)) every
    /// textured quad samples through its own tile UVs.
    pub sprite: AssetHandle<Texture>,
}

/// Attaches the file-loaded renderables the spawner cannot decode.
///
/// Every sprite entity carries a flat [`MeshRenderable`] and a
/// [`TexturedRenderable`] over it: the textured draw covers the flat quad
/// when the atlas resolves, and the flat quad carries the entity when it
/// does not. The player draws its quad in amber, each shard its quad in
/// cyan, and the goal its box marker in gold.
///
/// Pickups also gain the static pose the bridge extracts through: a
/// [`Transform`] copied from their authored position plus the matching
/// [`GlobalTransform`]. They carry no [`RigidBody`], so the physics step
/// never touches them and propagation simply refreshes their globals
/// every tick. The player's solver pose is widened to
/// [`PLAYER_MESH_SCALE`] (physics preserves scale across steps), with `z`
/// scale pinned to `1.0` so the goal box's depth keeps straddling the
/// room plane instead of stretching out of the camera's `[near, far]`.
///
/// `report` is the same spawner placement map
/// [`attach_simulation_components`] takes. A missing `"player"` (or a
/// player without a solver pose) is a content error reported as
/// [`EcsError::StaleOrUnknownEntity`]; missing pickups are tolerated and
/// simply draw nothing, mirroring the simulation seam.
pub fn attach_render_components(
    world: &mut World,
    report: &SpawnReport,
    handles: &RenderHandles,
) -> Result<(), EcsError> {
    let player = report
        .entity("player")
        .ok_or(EcsError::StaleOrUnknownEntity)?;
    // Widen the solver pose so the unit quad reads at the legacy quad's
    // size: physics preserves scale across steps, so this survives the
    // simulation untouched. `z` scale stays `1.0` so pickup box depth
    // keeps straddling the room plane (see below).
    let mut pose = world
        .get::<Transform>(player)
        .copied()
        .ok_or(EcsError::StaleOrUnknownEntity)?;
    pose.scale = glam::Vec3::new(PLAYER_MESH_SCALE, PLAYER_MESH_SCALE, 1.0);
    world.insert(player, pose)?;
    // Amber flat quad beneath the sprite: the textured draw covers it when
    // the sprite resolves, and it carries the player when it does not.
    world.insert(
        player,
        MeshRenderable::new(handles.player_mesh, [1.0, 0.6, 0.0]),
    )?;
    world.insert(
        player,
        TexturedRenderable::new(handles.player_sprite_mesh, handles.sprite),
    )?;

    for (local, mesh, sprite_mesh, color) in [
        (
            "goal",
            handles.goal_mesh,
            handles.goal_sprite_mesh,
            [1.0, 0.85, 0.2],
        ),
        (
            "pickup_a",
            handles.player_mesh,
            handles.shard_sprite_mesh,
            [0.2, 0.9, 0.9],
        ),
        (
            "pickup_b",
            handles.player_mesh,
            handles.shard_sprite_mesh,
            [0.2, 0.9, 0.9],
        ),
        (
            "pickup_c",
            handles.player_mesh,
            handles.shard_sprite_mesh,
            [0.2, 0.9, 0.9],
        ),
    ] {
        let Some(entity) = report.entity(local) else {
            continue;
        };
        let Some(pickup) = world.get::<Pickup>(entity) else {
            continue;
        };
        let (x, y) = (pickup.x, pickup.y);
        // Static pose from the authored position: no rigid body, so the
        // physics step never touches it and propagation simply refreshes
        // the global every tick.
        let pose = Transform {
            translation: glam::Vec3::new(x, y, 0.0),
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::new(PICKUP_MESH_SCALE, PICKUP_MESH_SCALE, 1.0),
        };
        world.insert(entity, pose)?;
        world.insert(entity, GlobalTransform::from_matrix(pose.to_matrix()))?;
        let visual = PickupVisual {
            mesh,
            color,
            sprite_mesh,
            sprite: handles.sprite,
        };
        world.insert(entity, visual)?;
        world.insert(entity, MeshRenderable::new(visual.mesh, visual.color))?;
        world.insert(entity, TexturedRenderable::new(sprite_mesh, handles.sprite))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_input::{ActionSnapshot, ActionState, PlayerSlot};
    use canary_runtime::RunContext;

    /// Builds a snapshot holding `held` down and `pressed` edge-triggered.
    fn snapshot_for(actions: &Actions, held: &[ActionId], pressed: &[ActionId]) -> SimulationInput {
        let states = |id: ActionId| ActionSnapshot {
            id,
            state: ActionState {
                down: held.contains(&id),
                pressed: pressed.contains(&id),
                released: false,
            },
        };
        SimulationInput {
            player: PlayerSlot::LOCAL,
            frame_index: 0,
            tick: None,
            actions: vec![
                states(actions.up),
                states(actions.down),
                states(actions.left),
                states(actions.right),
                states(actions.collect),
                states(actions.reset),
            ],
        }
    }

    /// Spawns a player with a seeded velocity; the movement system must
    /// overwrite it, never integrate from it.
    fn spawn_player(world: &mut World) {
        let player = world.spawn();
        world
            .insert(player, Player { x: 0.0, y: 0.0 })
            .expect("test player takes Player");
        world
            .insert(
                player,
                Velocity {
                    linvel: [999.0, 999.0],
                    angvel: 5.0,
                },
            )
            .expect("test player takes Velocity");
    }

    /// Runs only the movement stage against a hand-built snapshot.
    fn run_move_stage(actions: &Actions, snapshot: SimulationInput) -> [f32; 2] {
        let mut world = World::new();
        spawn_player(&mut world);
        snapshot.publish(&mut world);
        world.insert_resource(RunContext {
            run_id: 0,
            frame_index: 0,
            tick: world.change_tick(),
            frame_dt: std::time::Duration::from_millis(STEP_MS),
            sim_time: std::time::Duration::ZERO,
            sim_step: std::time::Duration::from_millis(STEP_MS),
        });
        let mut schedule = Schedule::new();
        register_move_player(&mut schedule, actions);
        schedule.run(&mut world);
        let linvel = world
            .query::<Velocity>()
            .next()
            .map(|(_, velocity)| velocity.linvel)
            .expect("velocity exists");
        linvel
    }

    #[test]
    fn held_right_writes_full_speed_velocity() {
        let (_, actions) = crate::declare_input();
        let snapshot = snapshot_for(&actions, &[actions.right], &[]);
        assert_eq!(run_move_stage(&actions, snapshot), [SPEED_PX_PER_SEC, 0.0]);
    }

    #[test]
    fn released_input_writes_zero_not_the_seeded_drift() {
        let (_, actions) = crate::declare_input();
        let snapshot = snapshot_for(&actions, &[], &[]);
        assert_eq!(run_move_stage(&actions, snapshot), [0.0, 0.0]);
    }

    #[test]
    fn opposing_inputs_cancel_on_both_axes() {
        let (_, actions) = crate::declare_input();
        let snapshot = snapshot_for(
            &actions,
            &[actions.up, actions.down, actions.left, actions.right],
            &[],
        );
        assert_eq!(run_move_stage(&actions, snapshot), [0.0, 0.0]);
    }

    /// Spawns a player at the origin, one shard in touch range, and a
    /// score bank; returns the pickup entity.
    fn spawn_touch_room(world: &mut World) -> canary_ecs::Entity {
        let player = world.spawn();
        world
            .insert(player, Player { x: 0.0, y: 0.0 })
            .expect("test player takes Player");
        let pickup = world.spawn();
        world
            .insert(
                pickup,
                Pickup {
                    x: COLLECT_RADIUS_PX - 1.0,
                    y: 0.0,
                    collected: false,
                    is_goal: false,
                },
            )
            .expect("test pickup takes Pickup");
        world
            .insert(
                pickup,
                AudioSource::new(AssetHandle::<Sound>::from_raw_parts(0, 0)),
            )
            .expect("test pickup takes AudioSource");
        let score = world.spawn();
        world
            .insert(score, Score { points: 0 })
            .expect("test score takes Score");
        world.insert_resource(GameStats::default());
        pickup
    }

    #[test]
    fn touch_collects_marks_taken_banks_one_and_plays() {
        let mut world = World::new();
        let pickup = spawn_touch_room(&mut world);
        let mut schedule = Schedule::new();
        register_collect_on_touch(&mut schedule);
        schedule.run(&mut world);

        assert!(
            world
                .get::<Pickup>(pickup)
                .expect("pickup exists")
                .collected,
            "touch must mark the pickup taken"
        );
        assert_eq!(
            world
                .get::<AudioSource>(pickup)
                .expect("voice exists")
                .state,
            SourceState::Playing,
            "collection must arm the pickup voice"
        );
        assert_eq!(
            world
                .query::<Score>()
                .next()
                .expect("score exists")
                .1
                .points,
            1,
            "collection must bank exactly one point"
        );
        assert_eq!(
            world
                .resource::<GameStats>()
                .expect("stats exist")
                .collected,
            1
        );
    }

    #[test]
    fn touch_collect_hides_both_drawables_and_reset_restores_them() {
        // The windowed bridge draws the textured scene whenever the
        // camera resolves, so a collected pickup must lose *both*
        // drawables — dropping only the flat quad leaves the sprite
        // on screen (the live bug: chime played, shard stayed).
        let mut world = World::new();
        let pickup = spawn_touch_room(&mut world);
        let mesh = AssetHandle::<Mesh>::from_raw_parts(0, 0);
        let sprite_mesh = AssetHandle::<Mesh>::from_raw_parts(0, 1);
        let sprite = AssetHandle::<Texture>::from_raw_parts(0, 2);
        world
            .insert(
                pickup,
                PickupVisual {
                    mesh,
                    color: [0.2, 0.9, 0.9],
                    sprite_mesh,
                    sprite,
                },
            )
            .expect("test pickup takes PickupVisual");
        world
            .insert(pickup, MeshRenderable::new(mesh, [0.2, 0.9, 0.9]))
            .expect("test pickup takes MeshRenderable");
        world
            .insert(pickup, TexturedRenderable::new(sprite_mesh, sprite))
            .expect("test pickup takes TexturedRenderable");
        let mut schedule = Schedule::new();
        register_collect_on_touch(&mut schedule);
        schedule.run(&mut world);

        assert!(
            world.get::<MeshRenderable>(pickup).is_none(),
            "collection must drop the flat drawable"
        );
        assert!(
            world.get::<TexturedRenderable>(pickup).is_none(),
            "collection must drop the textured drawable, or the bridge keeps drawing the shard"
        );

        reset_room(&mut world);
        assert!(
            world.get::<MeshRenderable>(pickup).is_some(),
            "reset must restore the flat drawable"
        );
        assert!(
            world.get::<TexturedRenderable>(pickup).is_some(),
            "reset must restore the textured drawable"
        );
    }

    #[test]
    fn collected_pickup_never_collects_twice() {
        let mut world = World::new();
        let pickup = spawn_touch_room(&mut world);
        let mut schedule = Schedule::new();
        register_collect_on_touch(&mut schedule);
        schedule.run(&mut world);
        schedule.run(&mut world);

        assert!(
            world
                .get::<Pickup>(pickup)
                .expect("pickup exists")
                .collected
        );
        assert_eq!(
            world
                .query::<Score>()
                .next()
                .expect("score exists")
                .1
                .points,
            1,
            "a second pass inside the taken shard must bank nothing"
        );
    }

    #[test]
    fn collect_edge_reaches_extended_range_once() {
        let (_, actions) = crate::declare_input();
        let mut world = World::new();
        let player = world.spawn();
        world
            .insert(player, Player { x: 0.0, y: 0.0 })
            .expect("test player takes Player");
        let pickup = world.spawn();
        world
            .insert(
                pickup,
                Pickup {
                    x: COLLECT_EDGE_RADIUS_PX - 1.0,
                    y: 0.0,
                    collected: false,
                    is_goal: false,
                },
            )
            .expect("test pickup takes Pickup");
        world
            .insert(
                pickup,
                AudioSource::new(AssetHandle::<Sound>::from_raw_parts(0, 0)),
            )
            .expect("test pickup takes AudioSource");
        let score = world.spawn();
        world
            .insert(score, Score { points: 0 })
            .expect("test score takes Score");
        world.insert_resource(GameStats::default());

        let mut schedule = Schedule::new();
        register_collect_on_edge(&mut schedule, actions.collect);
        snapshot_for(&actions, &[actions.collect], &[actions.collect]).publish(&mut world);
        schedule.run(&mut world);
        assert!(
            world
                .get::<Pickup>(pickup)
                .expect("pickup exists")
                .collected,
            "the edge pass must collect at extended range"
        );

        // Held, no edge: nothing further happens.
        snapshot_for(&actions, &[actions.collect], &[]).publish(&mut world);
        schedule.run(&mut world);
        assert_eq!(
            world
                .query::<Score>()
                .next()
                .expect("score exists")
                .1
                .points,
            1,
            "steady hold past the edge must not re-collect"
        );
    }

    #[test]
    fn reset_intent_restores_room_and_silences_voices() {
        let mut world = World::new();
        let pickup = spawn_touch_room(&mut world);
        let mut schedule = Schedule::new();
        register_collect_on_touch(&mut schedule);
        register_apply_ui_intents(&mut schedule);
        world.insert_resource(UiIntents::default());
        schedule.run(&mut world);
        assert!(
            world
                .get::<Pickup>(pickup)
                .expect("pickup exists")
                .collected
        );

        world.insert_resource(UiIntents {
            intents: vec![UiIntent::ButtonPressed(RESET_BUTTON)],
        });
        schedule.run(&mut world);

        assert!(
            !world
                .get::<Pickup>(pickup)
                .expect("pickup exists")
                .collected,
            "reset must un-take the pickup"
        );
        assert_eq!(
            world
                .get::<AudioSource>(pickup)
                .expect("voice exists")
                .state,
            SourceState::Stopped,
            "reset must silence the voice"
        );
        assert_eq!(
            world
                .query::<Score>()
                .next()
                .expect("score exists")
                .1
                .points,
            0,
            "reset must zero the score"
        );
    }

    #[test]
    fn quiet_ui_frame_resets_nothing() {
        let mut world = World::new();
        spawn_touch_room(&mut world);
        let mut schedule = Schedule::new();
        register_collect_on_touch(&mut schedule);
        register_apply_ui_intents(&mut schedule);
        world.insert_resource(UiIntents::default());
        schedule.run(&mut world);
        assert_eq!(
            world
                .query::<Score>()
                .next()
                .expect("score exists")
                .1
                .points,
            1,
            "touch collection must survive a quiet UI frame"
        );
    }

    /// Builds the live room world through the real binary seams: game
    /// schemas, disk assets, authored room spawn, simulation attachments,
    /// asset stores as resources, then the render attachment under test.
    fn live_room_world() -> World {
        let mut world = World::new();
        register_game_components(&mut world).expect("game schemas register");
        let game_assets = crate::assets::load_game_assets().expect("game assets load from disk");
        let room =
            crate::assets::load_room(&crate::assets::asset_dir().join(crate::assets::ROOM_FILE))
                .expect("room document loads");
        let decoder = crate::state::CollectathonDecoder;
        let spawner = canary_runtime::AuthoredSpawner::new(&decoder, &crate::assets::resolve_asset);
        let report = spawner.spawn(&mut world, &room).expect("room spawns");
        attach_simulation_components(
            &mut world,
            &report,
            &SimHandles {
                pickup_sound: game_assets.pickup_sound,
            },
        )
        .expect("simulation attaches");
        let render = RenderHandles {
            player_mesh: game_assets.player_mesh,
            goal_mesh: game_assets.goal_mesh,
            player_sprite_mesh: game_assets.player_sprite_mesh,
            shard_sprite_mesh: game_assets.shard_sprite_mesh,
            goal_sprite_mesh: game_assets.goal_sprite_mesh,
            sprite: game_assets.sprite,
        };
        world.insert_resource(game_assets.meshes);
        world.insert_resource(game_assets.textures);
        world.insert_resource(game_assets.sounds);
        attach_render_components(&mut world, &report, &render).expect("renderables attach");
        world
    }

    /// Window aspect used by the windowed sample (960x600).
    const WINDOW_ASPECT: f32 = 960.0 / 600.0;

    #[test]
    fn live_room_bakes_through_the_bridge_at_window_aspect_not_legacy() {
        let mut world = live_room_world();
        // Fresh globals, exactly like the windowed schedule order:
        // propagation before any bake.
        let mut schedule = Schedule::new();
        canary_transform::register_transform_propagation(&mut schedule);
        schedule.run(&mut world);

        let view = crate::scene::collectathon_view(WINDOW_ASPECT).expect("window aspect resolves");
        let mesh_items = canary_render_ecs::extract_mesh_scene(&world);
        assert_eq!(
            mesh_items.len(),
            5,
            "player plus goal plus three shards must all extract through live disk handles"
        );
        let ortho =
            canary_render_ecs::bake_scene_to_vertices_ortho(&mesh_items, view, WINDOW_ASPECT);
        assert!(
            !ortho.is_empty(),
            "the live mesh scene must bake to real vertices"
        );
        let legacy =
            canary_render_ecs::bake_scene_to_vertices_with_aspect(&mesh_items, WINDOW_ASPECT);
        assert_ne!(
            ortho, legacy,
            "the live bake must ride the ortho camera, not the legacy perspective fallback"
        );
        let textured_items = canary_render_ecs::extract_textured_scene(&world);
        assert_eq!(
            textured_items.len(),
            5,
            "the player, the goal and all three shards must extract through their live \
             mesh-plus-atlas handles"
        );
        let textured = canary_render_ecs::bake_textured_scene_to_vertices_ortho(
            &textured_items,
            view,
            WINDOW_ASPECT,
        );
        assert!(
            !textured.is_empty(),
            "the live textured scene must bake to real vertices"
        );
    }

    #[test]
    fn collected_pickup_leaves_the_bridge_scene_and_reset_restores_it() {
        let mut world = live_room_world();
        let player = world
            .query::<Player>()
            .map(|(entity, _)| entity)
            .next()
            .expect("room spawns a player");
        let shard = world
            .query::<Pickup>()
            .find(|(_, pickup)| !pickup.is_goal)
            .map(|(entity, pickup)| (entity, pickup.x, pickup.y))
            .expect("room authors a shard");
        let mesh_count = |world: &World| canary_render_ecs::extract_mesh_scene(world).len();

        let pickups: Vec<_> = world.query::<Pickup>().map(|(entity, _)| entity).collect();
        assert_eq!(
            pickups.len(),
            4,
            "the room authors the goal plus three shards"
        );
        for entity in pickups {
            assert!(
                !world
                    .get::<AudioSource>(entity)
                    .expect("every pickup carries a voice")
                    .positional,
                "pickup chimes must be non-positional so distance cannot silence them"
            );
        }

        assert_eq!(
            mesh_count(&world),
            5,
            "player plus goal plus three shards must extract from a fresh room"
        );

        // Stand the player on the shard and collect through the real
        // touch path.
        {
            let player = world.get_mut::<Player>(player).expect("player exists");
            player.x = shard.1;
            player.y = shard.2;
        }
        collect_within(&mut world, COLLECT_RADIUS_PX, true);

        assert!(
            world
                .get::<Pickup>(shard.0)
                .expect("shard exists")
                .collected,
            "touch collection must mark the shard taken"
        );
        assert!(
            world.get::<MeshRenderable>(shard.0).is_none(),
            "the collected shard must lose its drawable"
        );
        assert_eq!(
            mesh_count(&world),
            4,
            "the collected shard must leave the bridge scene"
        );
        assert_eq!(
            world
                .get::<AudioSource>(shard.0)
                .expect("voice survives collection")
                .state,
            SourceState::Playing,
            "the chime must still be armed on the surviving entity"
        );
        assert!(
            world.get::<Transform>(shard.0).is_some()
                && world.get::<GlobalTransform>(shard.0).is_some(),
            "collection must keep the pickup's transform and global"
        );

        reset_room(&mut world);

        assert!(
            !world
                .get::<Pickup>(shard.0)
                .expect("shard exists")
                .collected,
            "reset must un-take the shard"
        );
        assert!(
            world.get::<MeshRenderable>(shard.0).is_some(),
            "reset must re-insert the shard's drawable from PickupVisual"
        );
        assert_eq!(
            mesh_count(&world),
            5,
            "every pickup must draw again after reset"
        );

        // Collect the restored shard again: reset restores room state, so
        // the hide path must keep working across cycles.
        collect_within(&mut world, COLLECT_RADIUS_PX, true);
        assert!(
            world
                .get::<Pickup>(shard.0)
                .expect("shard exists")
                .collected,
            "the second collect must mark the restored shard taken"
        );
        assert!(
            world.get::<MeshRenderable>(shard.0).is_none(),
            "the second collect must hide the restored drawable"
        );
        assert_eq!(
            mesh_count(&world),
            4,
            "the re-collected shard must leave the bridge scene again"
        );
    }

    #[test]
    fn moved_player_transform_rebakes_through_propagation() {
        let mut world = live_room_world();
        let view = crate::scene::collectathon_view(WINDOW_ASPECT).expect("window aspect resolves");
        let bake_live = |world: &World| {
            let items = canary_render_ecs::extract_mesh_scene(world);
            canary_render_ecs::bake_scene_to_vertices_ortho(&items, view, WINDOW_ASPECT)
        };
        let mut schedule = Schedule::new();
        canary_transform::register_transform_propagation(&mut schedule);
        schedule.run(&mut world);
        let before = bake_live(&world);
        assert!(
            !before.is_empty(),
            "the live scene must bake before the move"
        );

        // Move the player's local pose 10px in x; the cached global stays
        // stale until propagation runs, exactly like a tick boundary.
        let player = world
            .query::<crate::Player>()
            .map(|(entity, _)| entity)
            .next()
            .expect("player exists");
        world
            .get_mut::<canary_transform::Transform>(player)
            .expect("player carries a Transform")
            .translation
            .x += 10.0;
        schedule.run(&mut world);
        let after = bake_live(&world);
        assert_ne!(
            before, after,
            "a moved Transform must rebake to new vertices once propagation refreshes the global"
        );
        // A rigid +10px x-step shifts NDC x by exactly
        // `10 / half_height / aspect` through the ortho camera; the
        // player's soup vertices must carry that shift, proving the bake
        // follows the propagated pose instead of echoing stale floats.
        let step_ndc = 10.0 / ARENA_HALF_PX / WINDOW_ASPECT;
        let shifted = before
            .iter()
            .step_by(5)
            .zip(after.iter().step_by(5))
            .any(|(a, b)| (b - a - step_ndc).abs() < 1e-5);
        assert!(
            shifted,
            "some baked soup vertex must shift by one +10px x-step in NDC ({step_ndc})"
        );
    }
}
