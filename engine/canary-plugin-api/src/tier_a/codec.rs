// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

use crate::component_value::{ComponentValue, PrimitiveValue};

wasmtime::component::bindgen!({
    path: "wit",
    world: "tier-a-plugin",
});

pub(crate) use canary::plugin::types::{
    ComponentValue as WitComponentValue, EntityHandle as WitEntityHandle,
    PrimitiveValue as WitPrimitiveValue,
};

pub(crate) fn from_wit_entity(handle: WitEntityHandle) -> canary_ecs::Entity {
    canary_ecs::Entity::from_raw_parts(handle.index, handle.generation)
}

pub(crate) fn to_wit_primitive(value: PrimitiveValue) -> WitPrimitiveValue {
    match value {
        PrimitiveValue::U32(v) => WitPrimitiveValue::U32(v),
        PrimitiveValue::S32(v) => WitPrimitiveValue::S32(v),
        PrimitiveValue::U64(v) => WitPrimitiveValue::U64(v),
        PrimitiveValue::S64(v) => WitPrimitiveValue::S64(v),
        PrimitiveValue::F32(v) => WitPrimitiveValue::F32(v),
        PrimitiveValue::F64(v) => WitPrimitiveValue::F64(v),
        PrimitiveValue::Bool(v) => WitPrimitiveValue::Bool(v),
        PrimitiveValue::Str(v) => WitPrimitiveValue::String(v),
    }
}

pub(crate) fn from_wit_primitive(value: WitPrimitiveValue) -> PrimitiveValue {
    match value {
        WitPrimitiveValue::U32(v) => PrimitiveValue::U32(v),
        WitPrimitiveValue::S32(v) => PrimitiveValue::S32(v),
        WitPrimitiveValue::U64(v) => PrimitiveValue::U64(v),
        WitPrimitiveValue::S64(v) => PrimitiveValue::S64(v),
        WitPrimitiveValue::F32(v) => PrimitiveValue::F32(v),
        WitPrimitiveValue::F64(v) => PrimitiveValue::F64(v),
        WitPrimitiveValue::Bool(v) => PrimitiveValue::Bool(v),
        WitPrimitiveValue::String(v) => PrimitiveValue::Str(v),
    }
}

pub(crate) fn to_wit_value(value: ComponentValue) -> WitComponentValue {
    match value {
        ComponentValue::Primitive(v) => WitComponentValue::Primitive(to_wit_primitive(v)),
        ComponentValue::List(items) => {
            WitComponentValue::List(items.into_iter().map(to_wit_primitive).collect())
        }
        ComponentValue::Record(fields) => WitComponentValue::Record(
            fields
                .into_iter()
                .map(|(name, value)| (name, to_wit_primitive(value)))
                .collect(),
        ),
    }
}

pub(crate) fn from_wit_value(value: WitComponentValue) -> ComponentValue {
    match value {
        WitComponentValue::Primitive(v) => ComponentValue::Primitive(from_wit_primitive(v)),
        WitComponentValue::List(items) => {
            ComponentValue::List(items.into_iter().map(from_wit_primitive).collect())
        }
        WitComponentValue::Record(fields) => ComponentValue::Record(
            fields
                .into_iter()
                .map(|(name, value)| (name, from_wit_primitive(value)))
                .collect(),
        ),
    }
}

/// Outcome of one guarded host-function body: either the body's value
/// or a trap error for the guest. Kept behind [`crate::PluginError`] by the
/// caller — no third-party type crosses this crate's public boundary.
pub(crate) fn guard_host_call<T>(
    plugin: &str,
    op: &'static str,
    body: impl FnOnce() -> T + std::panic::UnwindSafe,
) -> Result<T, wasmtime::Error> {
    std::panic::catch_unwind(body).map_err(|payload| {
        let detail = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|value| (*value).to_string())
            })
            .unwrap_or_else(|| "non-string panic payload".to_string());
        wasmtime::Error::msg(format!(
            "host function `{op}` for plugin `{plugin}` panicked: {detail}"
        ))
    })
}
