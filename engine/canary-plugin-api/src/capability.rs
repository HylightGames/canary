// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

/// A capability a plugin declares it needs.
///
/// v0.0.1-pre1: an illustrative, non-exhaustive set, and — because only
/// the trusted native (Tier B) loader exists so far — **advisory only**.
/// Tier B has no sandboxing by design (see
/// `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`), so
/// nothing in this crate currently *enforces* that a plugin only does what
/// it declared here. This type exists now so the declaration shape is
/// settled; it becomes load-bearing — actually checked and enforced by the
/// runtime — once the sandboxed WASM (Tier A) loader lands. See
/// `docs/architecture/plugin-system.md#the-plugin-trait-surface-canary-plugin-api`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Read access to ECS component/resource data the plugin declares.
    ReadEcsWorld,
    /// Write access to ECS component/resource data the plugin declares.
    WriteEcsWorld,
    /// Filesystem access.
    Filesystem,
    /// Network access.
    Network,
}
