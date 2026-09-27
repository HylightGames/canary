// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The immutable per-simulation-pass logical input snapshot (ADR 0025,
//! decision 2).
//!
//! [`SimulationInput`] carries player identity, the simulation frame/tick
//! identity, and deterministically ordered digital action states. It never
//! carries timestamps, raw device events, or UI capture state — replay and
//! networking record this layer, not those. The mapper stamps
//! `frame_index` at route time; the runtime stamps `tick` immediately
//! before the scheduled pass via [`SimulationInput::stamp_tick`], so tests
//! assert the observed pair, never an assumed one.

use canary_ecs::{Tick, World};

use crate::{ActionId, PlayerSlot};

/// Digital state of one logical action within a simulation pass.
///
/// `pressed`/`released` are aggregate-transition edges: they are set only on
/// a pass where the action's combined bindings transitioned up-to-down
/// (`pressed`) or down-to-up (`released`), never on steady-held passes.
/// When both transitions happen inside one pass, both edges are set and
/// `down` reports the end-of-pass hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionState {
    /// Whether any bound physical control is held at end of pass.
    pub down: bool,
    /// Whether the aggregate transitioned up-to-down during this pass.
    pub pressed: bool,
    /// Whether the aggregate transitioned down-to-up during this pass.
    pub released: bool,
}

impl ActionState {
    /// An action with nothing held and no edges.
    pub const UP: Self = Self {
        down: false,
        pressed: false,
        released: false,
    };

    /// A held action with no edges this pass (steady hold).
    pub const HELD: Self = Self {
        down: true,
        pressed: false,
        released: false,
    };
}

/// One action's state inside a [`SimulationInput`], in schema declaration
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSnapshot {
    /// Which declared action this entry reports.
    pub id: ActionId,
    /// The action's digital state for this pass.
    pub state: ActionState,
}

/// The immutable logical input crossing into one deterministic simulation
/// pass.
///
/// Stamped with the outer `frame_index` by the mapper at route time and
/// with `tick` by the runtime immediately before the scheduled pass.
/// Travels as a per-frame-overwritten ECS resource (see
/// [`SimulationInput::publish`]) — like `RunContext`, read by consumers
/// and written only by the runtime/input phase — so it never trips the
/// quiet-tick probe the way component data would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationInput {
    /// Which player this snapshot belongs to (`.13`: always the local player).
    pub player: PlayerSlot,
    /// The outer presentation frame this snapshot was routed in, stamped by
    /// the mapper at route time (not the simulation tick).
    pub frame_index: u64,
    /// The simulation tick, stamped by the runtime immediately before the
    /// scheduled pass. `None` until then: a freshly routed snapshot has a
    /// frame but no tick yet.
    pub tick: Option<Tick>,
    /// One entry per declared action, in schema declaration order.
    pub actions: Vec<ActionSnapshot>,
}

impl SimulationInput {
    /// The digital state of `id` in this snapshot, or `None` when `id`
    /// belongs to a different schema.
    pub fn state(&self, id: ActionId) -> Option<ActionState> {
        self.actions
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.state)
    }

    /// Whether `id` is held at end of pass (`false` for unknown ids).
    pub fn is_down(&self, id: ActionId) -> bool {
        self.state(id).is_some_and(|state| state.down)
    }

    /// Whether `id` pressed this pass (`false` for unknown ids).
    pub fn was_pressed(&self, id: ActionId) -> bool {
        self.state(id).is_some_and(|state| state.pressed)
    }

    /// Whether `id` released this pass (`false` for unknown ids).
    pub fn was_released(&self, id: ActionId) -> bool {
        self.state(id).is_some_and(|state| state.released)
    }

    /// Stamps the simulation tick. Called by the runtime immediately before
    /// the scheduled pass — never by the mapper, which stamps only
    /// `frame_index` at route time.
    pub fn stamp_tick(&mut self, tick: Tick) {
        self.tick = Some(tick);
    }

    /// Publishes this snapshot as a per-frame-overwritten ECS resource.
    ///
    /// Overwrites any previous [`SimulationInput`] resource: there is at
    /// most one value per type in a [`World`], so each frame's snapshot
    /// replaces the last. A resource write stamps no component change
    /// ticks, which is exactly why input delivery uses a resource rather
    /// than component data.
    pub fn publish(self, world: &mut World) {
        world.insert_resource(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ActionSchema;

    fn snapshot_fixture() -> (ActionId, ActionId, SimulationInput) {
        let (_, ids) = ActionSchema::declare(["jump", "fire"]).unwrap();
        let (jump, fire) = (ids[0], ids[1]);
        let snapshot = SimulationInput {
            player: PlayerSlot::LOCAL,
            frame_index: 7,
            tick: None,
            actions: vec![
                ActionSnapshot {
                    id: jump,
                    state: ActionState {
                        down: true,
                        pressed: true,
                        released: false,
                    },
                },
                ActionSnapshot {
                    id: fire,
                    state: ActionState::UP,
                },
            ],
        };
        (jump, fire, snapshot)
    }

    #[test]
    fn accessors_report_per_action_state() {
        let (jump, fire, snapshot) = snapshot_fixture();
        assert!(snapshot.is_down(jump));
        assert!(snapshot.was_pressed(jump));
        assert!(!snapshot.was_released(jump));
        assert!(!snapshot.is_down(fire));
        assert_eq!(snapshot.frame_index, 7);
        assert_eq!(snapshot.tick, None);
    }

    #[test]
    fn unknown_ids_read_as_inactive() {
        let (_, _, snapshot) = snapshot_fixture();
        let (_, foreign) = ActionSchema::declare(["other"]).unwrap();
        assert_eq!(snapshot.state(foreign[0]), None);
        assert!(!snapshot.is_down(foreign[0]));
        assert!(!snapshot.was_pressed(foreign[0]));
        assert!(!snapshot.was_released(foreign[0]));
    }

    #[test]
    fn stamp_tick_sets_the_runtime_tick() {
        let (_, _, mut snapshot) = snapshot_fixture();
        let mut world = World::new();
        world.advance_tick();
        let tick = world.change_tick();
        snapshot.stamp_tick(tick);
        assert_eq!(snapshot.tick, Some(tick));
    }

    #[test]
    fn publish_overwrites_the_previous_frame_resource() {
        let (jump, _, first) = snapshot_fixture();
        let mut world = World::new();
        first.publish(&mut world);

        let second = SimulationInput {
            player: PlayerSlot::LOCAL,
            frame_index: 8,
            tick: None,
            actions: vec![ActionSnapshot {
                id: jump,
                state: ActionState::UP,
            }],
        };
        second.publish(&mut world);

        let stored = world.resource::<SimulationInput>().unwrap();
        assert_eq!(stored.frame_index, 8);
        assert!(!stored.is_down(jump));
    }
}
