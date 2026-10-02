//! `collectathon`: the `v0.1.0` integration-proof sample — a one-room 2D
//! collectathon played against Canary's documented public APIs only.
//!
//! A physics-driven player moves on mapped digital actions, collects
//! authored pickups, and shows score/goal status in a `CanaryUI` HUD. One
//! HUD button sends game intent through the documented simulation boundary;
//! pickup state triggers a sound loaded from disk. The room uses authored
//! stable IDs and reloads through [`AuthoredSpawner`]; a simulation snapshot
//! separately captures the deterministic boundary. Separate binaries prove
//! the `.15` replication path, the `.16` shared-authored-edit path, and the
//! Tier A plugin guest against the same game data.
//!
//! Skeleton status (WP1): components, input schema, systems, HUD, asset
//! seams, snapshot bindings, replication mapping, collab host seam, and the
//! plugin codec are real and compile. Windowed presentation, audio-device
//! bring-up, and the multi-process harnesses land in later work packages.
//!
//! [`AuthoredSpawner`]: canary_runtime::AuthoredSpawner

/// Asset file names and room loading.
pub mod assets;
/// Authoritative collaboration session seam.
pub mod collab;
/// Simulation systems: movement, touch collection, pulse, UI intents.
pub mod game;
/// HUD build pass: score readout plus the bonus button.
pub mod hud;
/// Shared replication-session harness for the server/client binaries.
pub mod net_session;
/// Tier A plugin seam: the guest-visible `Score` codec.
pub mod plugin;
/// Networking seam: replicated schemas, wire codecs, opt-in marking.
pub mod replication;
/// Offscreen scene: player quad plus pickup diamonds through the RHI.
pub mod scene;
/// One shared WGSL → SPIR-V recipe for the scene and UI paint shaders.
pub mod shader;
/// Project-state seam: authored decoding plus the snapshot boundary.
pub mod state;

use canary_ecs::CanaryComponent;
use canary_input::{
    ActionId, ActionSchema, Binding, InputMapper, KeyCode, PhysicalControl,
    PointerButton as InputPointerButton,
};
use canary_platform::Key as PlatformKey;

/// Player position in logical pixels, relative to the room center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Player {
    /// Horizontal offset; the scene maps this to NDC.
    pub x: f32,
    /// Vertical offset, positive downward; the scene negates into NDC.
    pub y: f32,
}

impl CanaryComponent for Player {
    const SCHEMA_ID: &'static str = "collectathon.player@1";
}

/// One collectible: a shard, or the room goal when [`Pickup::is_goal`].
#[derive(Debug, Clone, PartialEq)]
pub struct Pickup {
    /// Horizontal offset in logical pixels.
    pub x: f32,
    /// Vertical offset in logical pixels.
    pub y: f32,
    /// Set the first frame the player touches or pulses this pickup.
    pub collected: bool,
    /// True for the room goal, false for an ordinary shard.
    pub is_goal: bool,
}

impl CanaryComponent for Pickup {
    const SCHEMA_ID: &'static str = "collectathon.pickup@1";
}

/// Score counter: one point per collected pickup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Score {
    /// Points banked so far.
    pub points: u32,
}

impl CanaryComponent for Score {
    const SCHEMA_ID: &'static str = "collectathon.score@1";
}

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
    /// Collect (edge-triggered): gather nearby pickups at extended range.
    pub collect: ActionId,
    /// Reset (edge-triggered): restore every pickup and zero the score.
    pub reset: ActionId,
}

/// Declares the game schema and binds WASD plus arrows to movement
/// (multiple bindings per action), Space plus pointer-primary to collect,
/// and R to reset.
///
/// Mirrors the `ui-game` input contract shape so the headless and windowed
/// consumers share one simulation boundary.
pub fn declare_input() -> (InputMapper, Actions) {
    let (schema, ids) = ActionSchema::declare(["up", "down", "left", "right", "collect", "reset"])
        .expect("schema declares");
    fn bind(mapper: &mut InputMapper, key: PlatformKey, action: ActionId) {
        mapper
            .add_binding(Binding::gameplay(
                PhysicalControl::Key(KeyCode::from_platform_key(key)),
                action,
            ))
            .expect("movement binding registers");
    }
    let mut mapper = InputMapper::new(schema);
    bind(&mut mapper, PlatformKey::W, ids[0]);
    bind(&mut mapper, PlatformKey::ArrowUp, ids[0]);
    bind(&mut mapper, PlatformKey::S, ids[1]);
    bind(&mut mapper, PlatformKey::ArrowDown, ids[1]);
    bind(&mut mapper, PlatformKey::A, ids[2]);
    bind(&mut mapper, PlatformKey::ArrowLeft, ids[2]);
    bind(&mut mapper, PlatformKey::D, ids[3]);
    bind(&mut mapper, PlatformKey::ArrowRight, ids[3]);
    bind(&mut mapper, PlatformKey::Space, ids[4]);
    mapper
        .add_binding(Binding::gameplay(
            PhysicalControl::Pointer(InputPointerButton::Primary),
            ids[4],
        ))
        .expect("pointer collect binding registers");
    bind(&mut mapper, PlatformKey::R, ids[5]);
    (
        mapper,
        Actions {
            up: ids[0],
            down: ids[1],
            left: ids[2],
            right: ids[3],
            collect: ids[4],
            reset: ids[5],
        },
    )
}
