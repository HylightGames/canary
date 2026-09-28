// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The `ui-game` simulation: movement on mapped actions, shots on the fire
//! edge, and shots on the HUD fire button's intent.
//!
//! Deliberately the same demo contract as the headless harness
//! (`canary-runtime`'s binary): same schema, same bindings, same systems,
//! same `"fire"` button id. The headless run proves the simulation; this
//! sample proves the simulation reaches a real window with a real HUD.

use canary_ecs::World;
use canary_input::{
    ActionId, ActionSchema, Binding, InputMapper, KeyCode, PhysicalControl,
    PointerButton as InputPointerButton, SimulationInput,
};
use canary_platform::Key as PlatformKey;
use canary_scheduler::{Schedule, SystemAccess};
use canary_ui_core::UiId;

/// Fixed simulation step: one pass per outer frame, no fixed-step runner
/// (deferred past `.13` by the roadmap).
pub const STEP_MS: u64 = 16;
/// Player speed in logical pixels per second.
pub const SPEED_PX_PER_SEC: f32 = 120.0;
/// The HUD fire button's id: the build pass (`hud`) and the intents stage
/// below agree on this value, never on a live `World` borrow.
pub const FIRE_BUTTON: UiId = UiId::new("fire");

/// Player position in logical pixels, relative to the scene center.
/// Clamped to the scene's half-range by the movement system, so the state
/// the HUD shows is the state the screen shows — never drift past the rim.
#[derive(Debug, Clone, Copy)]
pub struct Position {
    /// Horizontal offset; the scene maps this to NDC (clamped on screen).
    pub x: f32,
    /// Vertical offset, positive downward; the scene negates into NDC.
    pub y: f32,
}

/// Shot counter: edge-triggered, one per fire press or button activation.
#[derive(Debug, Clone, Copy)]
pub struct Shots(pub u32);

/// The game's digital actions, in schema-declaration order.
pub struct Actions {
    /// Move up.
    pub up: ActionId,
    /// Move down.
    pub down: ActionId,
    /// Move left.
    pub left: ActionId,
    /// Move right.
    pub right: ActionId,
    /// Fire (edge-triggered).
    pub fire: ActionId,
}

/// Declares the game schema and binds WASD plus arrows to movement
/// (multiple bindings per action) and Space plus pointer-primary to fire.
pub fn demo_input() -> (InputMapper, Actions) {
    let (schema, ids) =
        ActionSchema::declare(["up", "down", "left", "right", "fire"]).expect("schema declares");
    let mut mapper = InputMapper::new(schema);
    let mut bind = |key: PlatformKey, action: ActionId| {
        mapper
            .add_binding(Binding::gameplay(
                PhysicalControl::Key(KeyCode::from_platform_key(key)),
                action,
            ))
            .expect("movement binding registers");
    };
    bind(PlatformKey::W, ids[0]);
    bind(PlatformKey::ArrowUp, ids[0]);
    bind(PlatformKey::S, ids[1]);
    bind(PlatformKey::ArrowDown, ids[1]);
    bind(PlatformKey::A, ids[2]);
    bind(PlatformKey::ArrowLeft, ids[2]);
    bind(PlatformKey::D, ids[3]);
    bind(PlatformKey::ArrowRight, ids[3]);
    bind(PlatformKey::Space, ids[4]);
    mapper
        .add_binding(Binding::gameplay(
            PhysicalControl::Pointer(InputPointerButton::Primary),
            ids[4],
        ))
        .expect("pointer fire binding registers");
    (
        mapper,
        Actions {
            up: ids[0],
            down: ids[1],
            left: ids[2],
            right: ids[3],
            fire: ids[4],
        },
    )
}

/// Integrates held movement actions at the pass step. Held input moves
/// every pass; nothing latches inside the system.
pub fn register_move_player(schedule: &mut Schedule, actions: &Actions) {
    let (up, down, left, right) = (actions.up, actions.down, actions.left, actions.right);
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .reads_resource::<canary_runtime::RunContext>()
            .writes::<Position>(),
        move |world: &mut World| {
            let snapshot = world
                .resource::<SimulationInput>()
                .expect("driver publishes the snapshot before the schedule runs");
            let (dx, dy) = (
                f32::from(snapshot.is_down(right)) - f32::from(snapshot.is_down(left)),
                f32::from(snapshot.is_down(down)) - f32::from(snapshot.is_down(up)),
            );
            if dx == 0.0 && dy == 0.0 {
                return;
            }
            let context = world
                .resource::<canary_runtime::RunContext>()
                .expect("driver stamps RunContext before the schedule runs");
            let step = SPEED_PX_PER_SEC * context.sim_step.as_secs_f32();
            let entities: Vec<_> = world
                .query::<Position>()
                .map(|(entity, _)| entity)
                .collect();
            for entity in entities {
                if let Some(position) = world.get_mut::<Position>(entity) {
                    // Park at the rim: the scene renders this same bound,
                    // so holding into the edge stops the triangle instead
                    // of drifting the state invisibly offscreen.
                    position.x = (position.x + dx * step)
                        .clamp(-crate::scene::HALF_RANGE_PX, crate::scene::HALF_RANGE_PX);
                    position.y = (position.y + dy * step)
                        .clamp(-crate::scene::HALF_RANGE_PX, crate::scene::HALF_RANGE_PX);
                }
            }
        },
    );
}

/// Fires once per fire press edge, however long the control stays held.
pub fn register_fire_on_edge(schedule: &mut Schedule, fire: ActionId) {
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<SimulationInput>()
            .writes::<Shots>(),
        move |world: &mut World| {
            if !world
                .resource::<SimulationInput>()
                .expect("driver publishes the snapshot before the schedule runs")
                .was_pressed(fire)
            {
                return;
            }
            let entities: Vec<_> = world.query::<Shots>().map(|(entity, _)| entity).collect();
            for entity in entities {
                if let Some(shots) = world.get_mut::<Shots>(entity) {
                    shots.0 += 1;
                }
            }
        },
    );
}

/// Applies UI-originated intents at the declared simulation boundary: a
/// widget callback never touches the world; the intent lands here, in the
/// pass. This is the half the click-to-intent adapter proof doesn't cover
/// (that proof ends at the intent); the test below locks this half, and
/// the frame driver owns the plumbing between.
pub fn register_apply_ui_intents(schedule: &mut Schedule) {
    use canary_ui_core::{UiIntent, UiIntents};
    schedule.add_write_system(
        SystemAccess::new()
            .reads_resource::<UiIntents>()
            .writes::<Shots>(),
        |world: &mut World| {
            let fired = world
                .resource::<UiIntents>()
                .expect("driver publishes intents before the schedule runs")
                .intents
                .contains(&UiIntent::ButtonPressed(FIRE_BUTTON));
            if !fired {
                return;
            }
            let entities: Vec<_> = world.query::<Shots>().map(|(entity, _)| entity).collect();
            for entity in entities {
                if let Some(shots) = world.get_mut::<Shots>(entity) {
                    shots.0 += 1;
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_ui_core::{UiIntent, UiIntents};

    /// Runs only the intents stage against a hand-built resource: no
    /// backend, no clicks, no coordinates — the click-to-intent half is
    /// `canary-ui-egui`'s own proof, the plumbing half is the frame
    /// driver's; this test locks the game's application half.
    fn run_intents_stage(intents: UiIntents) -> u32 {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Shots(0))
            .expect("test entity takes Shots");
        world.insert_resource(intents);
        let mut schedule = Schedule::new();
        register_apply_ui_intents(&mut schedule);
        schedule.run(&mut world);
        let shots = world
            .query::<Shots>()
            .next()
            .map(|(_, shots)| shots.0)
            .expect("shots exist");
        shots
    }

    #[test]
    fn fire_button_intent_applies_one_shot_at_the_boundary() {
        let shots = run_intents_stage(UiIntents {
            intents: vec![UiIntent::ButtonPressed(FIRE_BUTTON)],
        });
        assert_eq!(shots, 1, "one button activation fires exactly one shot");
    }

    #[test]
    fn empty_intents_leave_shots_untouched() {
        let shots = run_intents_stage(UiIntents::default());
        assert_eq!(shots, 0, "a quiet UI frame fires nothing");
    }
}
