// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Deterministic gameplay input: from normalized platform events to the
//! per-simulation-pass [`SimulationInput`] snapshot.
//!
//! Implements the proposed contract in ADR 0025
//! (`docs/decisions/architecture-decision-records/0025-deterministic-input-actions-and-ui-capture.md`)
//! for the first `.13` slice: game-declared digital action identities
//! ([`ActionSchema`]), physical-to-action bindings ([`Binding`]), a
//! backend-neutral routed event stream with per-event UI-capture flags
//! ([`RoutedEvent`]), and an [`InputMapper`] that aggregates multiple
//! bindings before deriving edge state and stamps the outer
//! [`SimulationInput::frame_index`] at route time. The runtime stamps
//! [`SimulationInput::tick`] immediately before the scheduled pass; the
//! mapper never does.
//!
//! # Target design vs this slice
//!
//! Target: the full `RawInput → InputMapping → InputAction → PlayerInput →
//! SimulationInput` path with analog axes, remapping UI, persisted profiles,
//! multi-player assignment, and zero-or-multiple simulation passes per outer
//! frame. This slice is deliberately smaller: one local player
//! ([`PlayerSlot::LOCAL`]), digital actions only, single simulation pass per
//! outer frame. What is here is real, not a placeholder: every public item
//! is implemented and pinned by tests.
//!
//! # Dependency direction
//!
//! This is a leaf crate: it depends on `canary-ecs` (resource delivery),
//! `canary-platform` (normalized event conversion only), and `thiserror`
//! (typed errors). It knows nothing about rendering, UI backends, the
//! runtime, or `winit`/`egui`/scancode types — none of those appear in any
//! public signature.
//!
//! # Delivery contract
//!
//! [`SimulationInput`] travels as a per-frame-overwritten ECS resource (see
//! [`SimulationInput::publish`]), never as component data, so writing it
//! never trips the quiet-tick change-detection probe the way component
//! writes would. It carries player identity, frame/tick identity, and
//! deterministically ordered action states — never timestamps, raw events,
//! or UI capture state.

mod action;
mod capture;
mod mapping;
mod player;
mod snapshot;

pub use action::{ActionId, ActionSchema, SchemaError};
pub use capture::{all_captured, all_open, RoutedEvent};
pub use mapping::{
    drain_platform_events, raw_event_from_platform, Binding, InputMapper, KeyCode, MappingError,
    PhysicalControl, PointerButton, RawInputEvent,
};
pub use player::{PlayerSlot, LOCAL_PLAYER};
pub use snapshot::{ActionSnapshot, ActionState, SimulationInput};
