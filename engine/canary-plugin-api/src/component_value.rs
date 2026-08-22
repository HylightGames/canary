// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The Tier A component data ABI: a capped, portable value
//! representation, and the codec machinery bridging it to
//! [`canary_ecs::World`]'s type-erased accessors
//! (`get_erased`/`set_erased`).
//!
//! Genuinely separate from
//! [`canary_ecs::CanaryComponent`]/ADR 0010
//! (`docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md`):
//! that registry answers "which component is this?" (`SCHEMA_ID ->
//! TypeId`). It says nothing about how an arbitrary Rust struct's
//! *fields* cross the Component Model boundary — that's what
//! [`ComponentValue`] and [`ComponentValueCodec`] are for. See
//! `docs/roadmap/v0.0.3-roadmap.md`'s component-data-ABI scope item for
//! why these are deliberately two different problems, and why this one
//! stays in `canary-plugin-api` rather than `canary-ecs`: most
//! components will never need to be WASM-readable, so `canary-ecs`
//! itself has no reason to know about WIT/Component-Model value shapes.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

use canary_ecs::CanaryComponent;

/// One scalar value within a [`ComponentValue`] — split out as its own
/// type because WIT (like the Component Model generally) doesn't
/// support a type that recursively contains itself:
/// `wit/plugin.wit`'s `component-value` can't have a case holding
/// `list<component-value>`, discovered against the real `wit-parser`
/// bundled with `wasmtime` 21.0.2 while building this, not assumed
/// up front. `ComponentValue::List`/`Record` hold these, not
/// `ComponentValue` itself — one level of nesting (a flat record or
/// list of primitives), not arbitrarily deep structures.
#[derive(Debug, Clone, PartialEq)]
pub enum PrimitiveValue {
    /// An unsigned 32-bit integer.
    U32(u32),
    /// A signed 32-bit integer.
    S32(i32),
    /// An unsigned 64-bit integer.
    U64(u64),
    /// A signed 64-bit integer.
    S64(i64),
    /// A 32-bit float.
    F32(f32),
    /// A 64-bit float.
    F64(f64),
    /// A boolean.
    Bool(bool),
    /// A UTF-8 string.
    Str(String),
}

/// A capped, portable representation of a component's value — the
/// value-type set `docs/roadmap/v0.0.3-roadmap.md` scopes the data ABI
/// to (numeric primitives, `bool`, `string`, `list`, `record`), not
/// arbitrary Rust reflection. Mirrors `wit/plugin.wit`'s
/// `component-value` variant exactly (including its two-level split
/// from [`PrimitiveValue`] — see that type's doc comment for why); see
/// `tier_a.rs`'s module docs for the conversion between this type and the
/// `bindgen!`-generated one at the actual call boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentValue {
    /// A single scalar value.
    Primitive(PrimitiveValue),
    /// An ordered list of scalars, all conceptually the same shape
    /// (WIT's `list<primitive-value>`), though this dynamic
    /// representation doesn't itself enforce that — [`ComponentValueCodec`]
    /// impls are responsible for producing/consuming a consistent shape.
    List(Vec<PrimitiveValue>),
    /// Field name/value pairs, in declaration order. WIT's `record`
    /// type has statically named fields; this dynamic stand-in for it
    /// carries names alongside values instead, since
    /// [`ComponentValueCodec`] impls are hand-written per component
    /// type (see its own docs for why), not generated from a `record`
    /// declaration this crate would otherwise need to parse.
    Record(Vec<(String, PrimitiveValue)>),
}

/// Why a [`ComponentValueCodec`] conversion failed — either direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentValueError(pub String);

impl fmt::Display for ComponentValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "component value conversion failed: {}", self.0)
    }
}

impl std::error::Error for ComponentValueError {}

/// Implemented by component types that can be read and written across
/// the Tier A boundary via [`ComponentValue`] — the "host adapter" ADR
/// 0010 named but didn't itself build.
///
/// A separate trait from [`CanaryComponent`] on purpose: not every
/// component with a stable schema identity needs to be WASM-readable
/// (a component might only need identity for, say, a future
/// replication use), so this isn't folded into that trait's
/// requirements. Like [`CanaryComponent`] itself, this is a plain,
/// hand-written trait impl for this first cut, not a derive macro —
/// see ADR 0010's "Resolution" section for the same reasoning applied
/// there.
pub trait ComponentValueCodec: CanaryComponent + Sized {
    /// Converts `self` to its portable representation.
    fn to_component_value(&self) -> ComponentValue;

    /// Converts a portable representation back to `Self`. Returns
    /// [`ComponentValueError`] if `value`'s shape doesn't match what
    /// this type expects (e.g. a `Record` missing a required field, or
    /// a value of the wrong variant entirely) — a real possibility
    /// since a WASM guest, not this host, constructs the value being
    /// converted here.
    fn from_component_value(value: ComponentValue) -> Result<Self, ComponentValueError>;
}

/// One type's worth of type-erased conversion functions, captured over
/// the concrete type `T` at [`CodecRegistry::register`] time (the only
/// point `T` is known) and dispatched by [`std::any::TypeId`]
/// thereafter.
struct Codec {
    to_value: Box<dyn Fn(&(dyn Any + Send + Sync)) -> ComponentValue + Send + Sync>,
    from_value: Box<
        dyn Fn(ComponentValue) -> Result<Box<dyn Any + Send + Sync>, ComponentValueError>
            + Send
            + Sync,
    >,
}

/// Maps a [`TypeId`] to the [`ComponentValueCodec`] registered for it,
/// so a caller that only has a runtime `TypeId` (resolved via
/// [`canary_ecs::World::type_id_for_schema`]) can still convert to and
/// from [`ComponentValue`] — the type-erased half of the data ABI that
/// [`canary_ecs::World::get_erased`]/[`canary_ecs::World::set_erased`]
/// don't themselves provide (they erase *storage* access; this erases
/// *conversion*).
#[derive(Default)]
pub struct CodecRegistry {
    codecs: HashMap<TypeId, Codec>,
}

impl CodecRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `T`'s codec. Idempotent when called more than once for
    /// the same `T` (later registrations simply overwrite the same
    /// entry, since there's nothing to conflict on — unlike
    /// [`canary_ecs::World::register_component`], there's no separate
    /// stable identity here for two different types to collide over).
    pub fn register<T: ComponentValueCodec + Send + Sync + 'static>(&mut self) {
        let type_id = TypeId::of::<T>();
        self.codecs.insert(
            type_id,
            Codec {
                to_value: Box::new(|any| {
                    let concrete = any.downcast_ref::<T>().expect(
                        "CodecRegistry: internal invariant violated (TypeId/codec mismatch)",
                    );
                    concrete.to_component_value()
                }),
                from_value: Box::new(|value| {
                    let concrete = T::from_component_value(value)?;
                    Ok(Box::new(concrete) as Box<dyn Any + Send + Sync>)
                }),
            },
        );
    }

    /// Converts a type-erased reference (from
    /// [`canary_ecs::World::get_erased`]) to [`ComponentValue`], if
    /// `type_id` has a registered codec.
    pub fn to_value(
        &self,
        type_id: TypeId,
        value: &(dyn Any + Send + Sync),
    ) -> Option<ComponentValue> {
        self.codecs
            .get(&type_id)
            .map(|codec| (codec.to_value)(value))
    }

    /// Converts a [`ComponentValue`] to a type-erased, boxed value
    /// suitable for [`canary_ecs::World::set_erased`], if `type_id` has
    /// a registered codec.
    pub fn from_value(
        &self,
        type_id: TypeId,
        value: ComponentValue,
    ) -> Option<Result<Box<dyn Any + Send + Sync>, ComponentValueError>> {
        self.codecs
            .get(&type_id)
            .map(|codec| (codec.from_value)(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Health {
        value: f32,
    }

    impl CanaryComponent for Health {
        const SCHEMA_ID: &'static str = "canary-plugin-api-tests:health@1";
    }

    impl ComponentValueCodec for Health {
        fn to_component_value(&self) -> ComponentValue {
            ComponentValue::Record(vec![("value".to_string(), PrimitiveValue::F32(self.value))])
        }

        fn from_component_value(value: ComponentValue) -> Result<Self, ComponentValueError> {
            let ComponentValue::Record(fields) = value else {
                return Err(ComponentValueError("Health expects a record".to_string()));
            };
            let value = fields
                .into_iter()
                .find_map(|(name, v)| (name == "value").then_some(v))
                .ok_or_else(|| {
                    ComponentValueError("Health record missing field `value`".to_string())
                })?;
            let PrimitiveValue::F32(value) = value else {
                return Err(ComponentValueError(
                    "Health.value expects an f32".to_string(),
                ));
            };
            Ok(Health { value })
        }
    }

    #[test]
    fn registered_codec_round_trips_a_real_component_through_component_value() {
        let mut registry = CodecRegistry::new();
        registry.register::<Health>();
        let type_id = TypeId::of::<Health>();

        let original = Health { value: 42.5 };
        let boxed: Box<dyn Any + Send + Sync> = Box::new(original);

        let value = registry
            .to_value(type_id, boxed.as_ref())
            .expect("Health should have a registered codec");
        assert_eq!(
            value,
            ComponentValue::Record(vec![("value".to_string(), PrimitiveValue::F32(42.5))])
        );

        let round_tripped = registry
            .from_value(type_id, value)
            .expect("Health should have a registered codec")
            .expect("a well-formed value should convert back successfully");
        let round_tripped = round_tripped
            .downcast_ref::<Health>()
            .expect("from_value should produce a boxed Health");
        assert_eq!(*round_tripped, original);
    }

    #[test]
    fn from_value_reports_a_shape_mismatch_rather_than_panicking() {
        let mut registry = CodecRegistry::new();
        registry.register::<Health>();

        let result = registry
            .from_value(
                TypeId::of::<Health>(),
                ComponentValue::Primitive(PrimitiveValue::U32(7)),
            )
            .expect("Health should have a registered codec");

        assert!(
            result.is_err(),
            "a Primitive(U32) is not a valid Health representation"
        );
    }

    #[test]
    fn an_unregistered_type_id_yields_none_rather_than_a_default_conversion() {
        let registry = CodecRegistry::new();
        // Health was never registered on this instance.
        let health = Health { value: 1.0 };
        assert!(registry
            .to_value(TypeId::of::<Health>(), &health as &(dyn Any + Send + Sync))
            .is_none());
    }
}
