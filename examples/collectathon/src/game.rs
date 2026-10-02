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
//! [`register_gameplay`]): physics steps first, transform propagation
//! recomputes globals from just-stepped locals, gameplay writers run
//! next, and the audio trigger actuates last so it never plays
//! one-tick-stale intents.

use canary_assets::{AssetHandle, Sound};
use canary_audio::{AudioSource, SourceState};
use canary_ecs::{EcsError, World};
use canary_input::{ActionId, SimulationInput};
use canary_physics::{Collider, GravityScale, RigidBody, Velocity};
use canary_runtime::SpawnReport;
use canary_scheduler::{Schedule, SystemAccess};
use canary_transform::{GlobalTransform, Transform};
use canary_ui_core::{UiIntent, UiIntents};

use crate::hud::RESET_BUTTON;
use crate::state::GameStats;
use crate::{Actions, Pickup, Player, Score};

/// Fixed simulation step: one pass per outer frame, no fixed-step runner
/// (deferred past `.13` by the roadmap).
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

/// Shared touch/edge collector: marks pickups within `radius` taken, flips
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
            .writes::<Pickup>()
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
            .writes::<Pickup>()
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

/// Restores the room: every pickup un-taken, every pickup voice stopped,
/// every score zeroed, collected stats zeroed. Totals ([`GameStats::goal`])
/// are room content, not run state, so they survive the reset.
pub fn reset_room(world: &mut World) {
    let pickups: Vec<_> = world.query::<Pickup>().map(|(entity, _)| entity).collect();
    for entity in pickups {
        if let Some(pickup) = world.get_mut::<Pickup>(entity) {
            pickup.collected = false;
        }
        if let Some(source) = world.get_mut::<AudioSource>(entity) {
            source.state = SourceState::Stopped;
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

/// Attaches the simulation-owned components the spawner cannot decode:
/// the player becomes a gravity-free dynamic cuboid body with a seeded
/// velocity and a solver pose copied from its authored position, every
/// spawned pickup gains a stopped voice on `handles.pickup_sound`, and
/// the run stats zero out over the spawned pickup count.
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

    let mut total = 0u32;
    for local in ["goal", "pickup_a", "pickup_b", "pickup_c"] {
        let Some(entity) = report.entity(local) else {
            continue;
        };
        if world.get::<Pickup>(entity).is_none() {
            continue;
        }
        world.insert(entity, AudioSource::new(handles.pickup_sound))?;
        total += 1;
    }
    let stats = GameStats {
        collected: 0,
        goal: total,
    };
    world.insert_resource(stats);
    Ok(stats)
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
}
