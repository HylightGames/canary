// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Player identity for input snapshots: a small `Copy` slot id with `0`
//! reserved for the local player (ADR 0025, decision 6). Per-slot profile
//! instances (split-screen) and direct snapshot injection ride on this same
//! type later without reshaping it; multiplayer assignment is deferred.

/// The local player slot id: `0`.
pub const LOCAL_PLAYER: PlayerSlot = PlayerSlot::LOCAL;

/// Which player a [`crate::SimulationInput`] snapshot belongs to.
///
/// A small `Copy` slot id. Slot `0` is the local player; the `.13` slice
/// only ever produces that slot. Higher slots are reserved for future
/// per-slot profile instances, not assigned by anything in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayerSlot(u16);

impl PlayerSlot {
    /// The local player slot (`0`).
    pub const LOCAL: Self = Self(0);

    /// Creates the player slot `slot`.
    pub fn new(slot: u16) -> Self {
        Self(slot)
    }

    /// The raw slot number (`0` for the local player).
    pub fn index(self) -> u16 {
        self.0
    }

    /// Whether this is the local player slot (`0`).
    pub fn is_local(self) -> bool {
        self.0 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_player_is_slot_zero() {
        assert_eq!(PlayerSlot::LOCAL.index(), 0);
        assert!(PlayerSlot::LOCAL.is_local());
        assert_eq!(LOCAL_PLAYER, PlayerSlot::LOCAL);
    }

    #[test]
    fn nonzero_slots_are_not_local() {
        assert!(!PlayerSlot::new(1).is_local());
        assert_eq!(PlayerSlot::new(3).index(), 3);
    }
}
