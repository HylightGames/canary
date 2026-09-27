// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Game-declared action identities and their canonical declaration order
//! (ADR 0025, decision 2).
//!
//! The game declares its logical action names once, up front, via
//! [`ActionSchema::declare`]. Declaration order is the canonical order for
//! snapshots, replay, and wire encodings: [`crate::SimulationInput`]
//! always lists actions in that order regardless of the order physical
//! events arrived in. Identities are stable within the declaring schema;
//! replay/network encodings must be versioned explicitly (ADR 0022
//! Clarification 6), never inferred from memory layout.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

/// Source of per-schema tags: every [`ActionSchema::declare`] call claims
/// the next tag, so identities from different schemas never compare equal
/// even when they share a declaration index. Wraps on overflow, which is
/// unreachable in practice (2^64 declarations).
static NEXT_SCHEMA_TAG: AtomicU64 = AtomicU64::new(0);

/// A stable game-owned action identity within one [`ActionSchema`].
///
/// The concrete representation is intentionally opaque: games name actions by
/// string at declaration time and keep the returned [`ActionId`]s. An id
/// pairs its schema's unique tag with its declaration index, so ids stay
/// compact and `Copy`, remain stable within their schema, and never alias
/// an id from another schema. Ids are never serialized — replay/network
/// encodings use schema-ordered positions under an explicit version.
/// Tag values depend on process-wide declaration order, which snapshots
/// never expose: ordering always comes from the schema, never the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionId {
    /// Which schema declaration issued this id.
    tag: u64,
    /// Declaration index within that schema.
    index: u32,
}

/// Failure to declare an [`ActionSchema`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchemaError {
    /// No action names were supplied; a schema with no actions cannot order
    /// a snapshot.
    #[error("action schema declares no actions")]
    EmptySchema,
    /// The same action name was declared twice.
    #[error("action schema declares {name:?} more than once")]
    DuplicateAction {
        /// The repeated action name.
        name: String,
    },
    /// More actions were declared than fit in an [`ActionId`].
    #[error("action schema declares more actions than fit in an ActionId")]
    TooManyActions,
}

/// The game's declared logical actions, in canonical order.
///
/// Returned alongside the per-name [`ActionId`]s by [`ActionSchema::declare`].
/// Snapshots iterate [`ActionSchema::action_ids`] order; event arrival order
/// never affects it.
#[derive(Debug, Clone)]
pub struct ActionSchema {
    /// Unique tag claimed at declaration; part of every issued [`ActionId`].
    tag: u64,
    /// Action names in declaration order.
    names: Vec<String>,
    /// Issued identities in declaration order (the canonical order).
    order: Vec<ActionId>,
    /// Reverse map from identity to declaration position.
    positions: HashMap<ActionId, usize>,
}

impl ActionSchema {
    /// Declares the game's logical actions.
    ///
    /// Returns the schema plus one [`ActionId`] per name, in the same order
    /// as `names`. That order is the canonical snapshot/replay order.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError::EmptySchema`] when `names` is empty,
    /// [`SchemaError::DuplicateAction`] when a name repeats, and
    /// [`SchemaError::TooManyActions`] when the declaration count exceeds
    /// the [`ActionId`] range.
    pub fn declare<I, S>(names: I) -> Result<(Self, Vec<ActionId>), SchemaError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let names: Vec<String> = names.into_iter().map(Into::into).collect();
        if names.is_empty() {
            return Err(SchemaError::EmptySchema);
        }
        let mut seen: HashMap<&str, ()> = HashMap::with_capacity(names.len());
        for name in &names {
            if seen.insert(name.as_str(), ()).is_some() {
                return Err(SchemaError::DuplicateAction { name: name.clone() });
            }
        }
        let tag = NEXT_SCHEMA_TAG.fetch_add(1, Ordering::Relaxed);
        let mut ids = Vec::with_capacity(names.len());
        let mut positions = HashMap::with_capacity(names.len());
        for position in 0..names.len() {
            let id = ActionId {
                tag,
                index: u32::try_from(position).map_err(|_| SchemaError::TooManyActions)?,
            };
            ids.push(id);
            positions.insert(id, position);
        }
        let order = ids.clone();
        Ok((
            Self {
                tag,
                names,
                order,
                positions,
            },
            ids,
        ))
    }

    /// The number of declared actions.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the schema declares no actions (unreachable via
    /// [`ActionSchema::declare`], which rejects empties, but part of the
    /// collection contract).
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Every declared [`ActionId`], in canonical declaration order.
    pub fn action_ids(&self) -> Vec<ActionId> {
        self.order.clone()
    }

    /// The declaration position of `id`, or `None` when `id` belongs to a
    /// different schema.
    pub fn position(&self, id: ActionId) -> Option<usize> {
        if id.tag != self.tag {
            return None;
        }
        self.positions.get(&id).copied()
    }

    /// The declared name behind `id`, or `None` when `id` belongs to a
    /// different schema.
    pub fn name(&self, id: ActionId) -> Option<&str> {
        self.position(id)
            .and_then(|position| self.names.get(position).map(String::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declare_returns_ids_in_declaration_order() {
        let (schema, ids) = ActionSchema::declare(["jump", "fire", "crouch"]).unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(schema.action_ids(), ids);
        assert_eq!(schema.name(ids[0]), Some("jump"));
        assert_eq!(schema.name(ids[1]), Some("fire"));
        assert_eq!(schema.name(ids[2]), Some("crouch"));
        assert_eq!(schema.position(ids[2]), Some(2));
    }

    #[test]
    fn declare_rejects_an_empty_schema() {
        let empty: Vec<String> = Vec::new();
        assert_eq!(
            ActionSchema::declare(empty).map(|_| ()),
            Err(SchemaError::EmptySchema)
        );
    }

    #[test]
    fn declare_rejects_a_duplicate_name() {
        assert_eq!(
            ActionSchema::declare(["jump", "fire", "jump"]).map(|_| ()),
            Err(SchemaError::DuplicateAction {
                name: String::from("jump"),
            })
        );
    }

    #[test]
    fn foreign_ids_resolve_to_none() {
        let (schema, _) = ActionSchema::declare(["jump"]).unwrap();
        let (other, other_ids) = ActionSchema::declare(["fire"]).unwrap();
        assert_eq!(schema.position(other_ids[0]), None);
        assert_eq!(schema.name(other_ids[0]), None);
        assert_eq!(other.len(), 1);
        assert!(!other.is_empty());
    }
}
