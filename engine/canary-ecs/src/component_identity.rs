// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

/// Implemented by component types that need a stable identity across the
/// plugin, replication, or marketplace boundary -- see
/// `docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md`
/// (ADR 0010).
///
/// `std::any::TypeId` is a Rust-compiler-internal concept with no
/// meaning to a Tier A (WASM Component Model) plugin, a replication
/// wire format, or a marketplace tool, and isn't even guaranteed stable
/// across two separate Rust compilations. [`World::insert`], `get`,
/// `query`, and friends still only require `T: Send + Sync + 'static`
/// and use `TypeId` internally as an efficient host-only fast-path
/// lookup key -- that doesn't change. What `CanaryComponent` adds is a
/// second, *stable* identity for the types that need one: a namespaced
/// string plus a schema version, registered against the host's
/// `TypeId` via [`World::register_component`] so a plugin or wire
/// format can name a schema without ever observing `TypeId` itself.
///
/// Purely internal, host-only components (never replicated, never
/// plugin-visible) have no need to implement this at all.
///
/// This first cut deliberately implements ADR 0010's "or an explicit
/// trait impl" alternative rather than a `#[derive(CanaryComponent)]`
/// macro: the ADR names the choice between the two as one of the open
/// questions this prototyping pass exists to settle, and a manual impl
/// was enough to validate the identity-and-registry direction without
/// committing to macro/proc-macro infrastructure before there was a
/// second real consumer to design it against. That consumer now exists
/// -- the Tier A loader (`v0.0.3`, `engine/canary-plugin-api/src/tier_a.rs`)
/// uses this exact trait and registry, unchanged, to resolve a WASM
/// guest's `schema-id` string back to a Rust type. A derive macro
/// remains a fully backward-compatible future addition -- it would just
/// generate the same trait impl written here by hand.
///
/// [`World::insert`]: crate::World::insert
/// [`World::register_component`]: crate::World::register_component
pub trait CanaryComponent: Send + Sync + 'static {
    /// A stable, namespaced schema identity, e.g.
    /// `"canary:transform/position@1"`. See ADR 0010 for the format
    /// (analogous to WIT interface/world versioning and to how
    /// Protocol Buffers/Cap'n Proto assign wire identities independent
    /// of any single host language's in-memory representation) and why
    /// it's a string+version pair rather than relying on `TypeId`.
    const SCHEMA_ID: &'static str;
}
