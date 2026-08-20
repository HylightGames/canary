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
/// An illustrative, non-exhaustive set. Enforcement varies by tier:
/// Tier B has no sandboxing by design (see
/// `docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi`), so
/// nothing in this crate enforces that a Tier B plugin only does what it
/// declared here — it's advisory only for that tier. Tier A enforces
/// [`Capability::ReadEcsWorld`] **structurally**, as of `v0.0.3`'s first
/// slice: see [`crate::WasmPluginLoader::load`]'s doc comment. The
/// remaining three variants don't have a corresponding Tier A interface
/// yet (`wit/plugin.wit` only defines one), so they remain advisory for
/// both tiers until that's built out — see
/// `docs/roadmap/v0.0.3-roadmap.md`.
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
