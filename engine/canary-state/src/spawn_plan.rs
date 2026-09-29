//! Ordered spawn plans: the `SchemaId`-to-fields vocabulary at the simulation boundary.
//!
//! [`SpawnPlan::from_document`] translates an [`AuthoredDocument`](crate::authored::AuthoredDocument)
//! into an ordered, fully validated plan without touching any live world: it
//! scans `entity.<local>` sections, bakes any `prefab` reference (one level:
//! prefab base fields first, then the instance's own fields win per field),
//! resolves every `{ "$asset": "<id>" }` marker through the caller-supplied
//! resolver, converts each component body into canonical
//! [`SnapshotValue`](crate::value::SnapshotValue) fields, and validates every
//! value. A non-object section, an unknown prefab, an unresolvable asset, or
//! a non-canonical value fails here — before any caller spawns.
//!
//! Baking reads the document but never rewrites it: the authored
//! instance/override representation stays intact (each [`PlannedEntity`]
//! records which prefab it baked, if any), and the plan carries the merged
//! result. Decoding fields into components and placing them with typed
//! `World::insert` is the composition owner's job (see `canary-runtime`'s
//! staged spawner, which validates the whole plan against the world registry
//! and decodes everything before the first spawn). This crate stays a leaf:
//! `SchemaId` and field maps cross the seam, never `Any`, `TypeId`, or a
//! `World`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::authored::AuthoredDocument;
use crate::error::StateError;
use crate::schema::SchemaId;
use crate::value::SnapshotValue;

/// Prefix marking an authored section as a spawnable entity. The rest of
/// the section name is the entity's local (authored) name, stable across
/// runs no matter which runtime IDs the world hands out.
const ENTITY_PREFIX: &str = "entity.";

/// Section-level key naming the prefab an entity instance bakes: the value
/// must be a string prefab name from the document's prefab table. Sibling
/// keys are component schemas whose fields override the baked prefab fields
/// per field. Prefab tables never nest instances: a `prefab` key inside a
/// prefab's own overrides is rejected.
const PREFAB_KEY: &str = "prefab";

/// Single-key object marking an asset reference inside component data:
/// `{ "$asset": "<logical-id>" }`. Planning resolves every marker before
/// decoding, so decoders only ever see concrete values.
const ASSET_MARKER: &str = "$asset";

/// One component instance awaiting decode: its schema plus canonical fields.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedComponent {
    /// Which component schema these fields belong to.
    pub schema: SchemaId,
    /// Canonical field values, in key order.
    pub fields: BTreeMap<String, SnapshotValue>,
}

/// One entity awaiting spawn: its authored-local name plus components.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedEntity {
    /// Authored-local name (the section name after `entity.`).
    pub local: String,
    /// Prefab this instance baked, if the section named one. The authored
    /// document is never rewritten by baking; this records provenance.
    pub prefab: Option<String>,
    /// Components in section order.
    pub components: Vec<PlannedComponent>,
}

/// The validated spawn order for one document: entities in deterministic
/// (section-name) order, each with its components in section order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpawnPlan {
    /// Entities to spawn, in order.
    pub entities: Vec<PlannedEntity>,
}

impl SpawnPlan {
    /// Translates `doc` into spawn order. Non-entity sections (settings,
    /// future tables) are ignored; `entity.<local>` sections bake their
    /// `prefab` reference when present (prefab base fields first, then the
    /// instance's own fields win per field). Asset markers resolve through
    /// `assets`; every field value is validated. No live world is touched —
    /// registry agreement and typed decode are the spawner's later phases.
    /// The document itself is never mutated: baking merges into the plan.
    pub fn from_document(
        doc: &AuthoredDocument,
        assets: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, StateError> {
        let mut entities = Vec::new();
        for (name, section) in &doc.sections {
            let Some(local) = name.strip_prefix(ENTITY_PREFIX) else {
                continue;
            };
            let components = section.as_object().ok_or_else(|| StateError::File {
                path: Path::new("<document>").to_path_buf(),
                reason: format!("section '{name}' is not an object"),
            })?;
            let (prefab, baked) = bake_prefab(doc, name, components)?;
            let mut planned = PlannedEntity {
                local: local.to_owned(),
                prefab,
                components: Vec::new(),
            };
            for (schema_name, data) in &baked {
                let resolved = resolve_assets(data, assets)?;
                let fields = component_fields(&resolved, name, schema_name)?;
                fields.values().try_for_each(SnapshotValue::validate)?;
                planned.components.push(PlannedComponent {
                    schema: SchemaId::new(schema_name),
                    fields,
                });
            }
            entities.push(planned);
        }
        Ok(Self { entities })
    }
}

/// Bakes one entity section: when the section names a `prefab`, the
/// prefab's resolved field map seeds the component table and the section's
/// own component entries override it per field (instance wins); otherwise
/// the section stands alone. Returns the baked prefab name (if any) plus
/// the merged schema-to-fields table in deterministic key order.
///
/// Merge rules: schemas only in the prefab are inherited whole; schemas in
/// both merge per field with the instance winning; schemas only in the
/// instance are appended. Both sides must hold objects per schema — a
/// scalar component body is a typed error either way.
fn bake_prefab(
    doc: &AuthoredDocument,
    section: &str,
    components: &serde_json::Map<String, serde_json::Value>,
) -> Result<(Option<String>, BTreeMap<String, serde_json::Value>), StateError> {
    let file_error = |reason: String| StateError::File {
        path: Path::new("<document>").to_path_buf(),
        reason,
    };
    let prefab_name = match components.get(PREFAB_KEY) {
        None => {
            return Ok((
                None,
                components
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ))
        }
        Some(serde_json::Value::String(name)) => name.clone(),
        Some(other) => {
            return Err(file_error(format!(
                "section '{section}' names a prefab with a non-string '{PREFAB_KEY}' key: {other}"
            )));
        }
    };
    let baked_base = doc.resolve_prefab(&prefab_name).map_err(|error| {
        file_error(format!(
            "section '{section}' bakes unknown or chained prefab '{prefab_name}': {error}"
        ))
    })?;
    let mut merged = BTreeMap::new();
    for (key, value) in &baked_base {
        if key == PREFAB_KEY {
            return Err(file_error(format!(
                "prefab '{prefab_name}' nests an instance key '{PREFAB_KEY}': prefabs never nest instances"
            )));
        }
        if !value.is_object() {
            return Err(file_error(format!(
                "prefab '{prefab_name}' holds a non-object schema '{key}'"
            )));
        }
        merged.insert(key.clone(), value.clone());
    }
    for (key, value) in components {
        if key == PREFAB_KEY {
            continue;
        }
        match (merged.get_mut(key), value) {
            (
                Some(serde_json::Value::Object(base_fields)),
                serde_json::Value::Object(override_fields),
            ) => {
                for (field, item) in override_fields {
                    base_fields.insert(field.clone(), item.clone());
                }
            }
            (_, serde_json::Value::Object(_)) => {
                merged.insert(key.clone(), value.clone());
            }
            _ => {
                return Err(file_error(format!(
                    "component '{key}' in section '{section}' is not an object"
                )));
            }
        }
    }
    Ok((Some(prefab_name), merged))
}

/// Replaces every `{ "$asset": "<id>" }` marker with the resolved string,
/// recursing through objects and arrays. Decoders never see markers.
fn resolve_assets(
    value: &serde_json::Value,
    assets: &dyn Fn(&str) -> Option<String>,
) -> Result<serde_json::Value, StateError> {
    match value {
        serde_json::Value::Object(entries)
            if entries.len() == 1 && entries.contains_key(ASSET_MARKER) =>
        {
            let id = entries[ASSET_MARKER]
                .as_str()
                .ok_or_else(|| StateError::File {
                    path: Path::new("<document>").to_path_buf(),
                    reason: "asset marker value is not a string".to_owned(),
                })?;
            let resolved =
                assets(id).ok_or_else(|| StateError::AssetUnresolved { id: id.to_owned() })?;
            Ok(serde_json::Value::String(resolved))
        }
        serde_json::Value::Object(entries) => entries
            .iter()
            .map(|(key, item)| resolve_assets(item, assets).map(|item| (key.clone(), item)))
            .collect::<Result<serde_json::Map<String, serde_json::Value>, _>>()
            .map(serde_json::Value::Object),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| resolve_assets(item, assets))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        scalar => Ok(scalar.clone()),
    }
}

/// Converts one component's JSON object into canonical snapshot fields.
fn component_fields(
    data: &serde_json::Value,
    section: &str,
    schema: &str,
) -> Result<BTreeMap<String, SnapshotValue>, StateError> {
    let object = data.as_object().ok_or_else(|| StateError::File {
        path: Path::new("<document>").to_path_buf(),
        reason: format!("component '{schema}' in section '{section}' is not an object"),
    })?;
    Ok(object
        .iter()
        .map(|(key, item)| (key.clone(), SnapshotValue::from_json(item)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ProjectId;

    fn doc() -> AuthoredDocument {
        AuthoredDocument::new(ProjectId::generate().expect("os randomness"))
    }

    fn resolve(id: &str) -> Option<String> {
        Some(format!("resolved/{id}"))
    }

    #[test]
    fn plan_lists_entities_in_section_order_with_fields() {
        let mut document = doc();
        document.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({ "test.health": { "hp": 50.0 } }),
        );
        document.sections.insert(
            "entity.sidekick".to_owned(),
            serde_json::json!({ "test.health": { "hp": 30.0 } }),
        );
        document.sections.insert(
            "settings".to_owned(),
            serde_json::json!({ "gravity": 9.81 }),
        );

        let plan = SpawnPlan::from_document(&document, &resolve).expect("plan");

        assert_eq!(plan.entities.len(), 2);
        assert_eq!(plan.entities[0].local, "hero");
        assert_eq!(plan.entities[1].local, "sidekick");
        assert_eq!(
            plan.entities[0].components[0].schema,
            SchemaId::new("test.health")
        );
        assert_eq!(
            plan.entities[0].components[0].fields.get("hp"),
            Some(&SnapshotValue::F64(50.0))
        );
    }

    #[test]
    fn asset_markers_resolve_before_fields_are_built() {
        let mut document = doc();
        document.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({ "test.sprite": { "tex": { "$asset": "hero.png" } } }),
        );

        let plan = SpawnPlan::from_document(&document, &resolve).expect("plan");

        assert_eq!(
            plan.entities[0].components[0].fields.get("tex"),
            Some(&SnapshotValue::Str("resolved/hero.png".to_owned()))
        );
    }

    #[test]
    fn unresolved_asset_aborts_planning() {
        let mut document = doc();
        document.sections.insert(
            "entity.ghost".to_owned(),
            serde_json::json!({ "test.sprite": { "tex": { "$asset": "missing.png" } } }),
        );

        let err = SpawnPlan::from_document(&document, &|_| None).expect_err("missing asset fails");
        assert!(matches!(err, StateError::AssetUnresolved { .. }));
    }

    #[test]
    fn non_object_section_is_a_typed_error() {
        let mut document = doc();
        document
            .sections
            .insert("entity.broken".to_owned(), serde_json::json!([1, 2, 3]));

        let err = SpawnPlan::from_document(&document, &resolve).expect_err("array fails");
        assert!(matches!(err, StateError::File { .. }));
    }

    #[test]
    fn non_object_component_is_a_typed_error() {
        let mut document = doc();
        document.sections.insert(
            "entity.broken".to_owned(),
            serde_json::json!({ "test.health": 42 }),
        );

        let err = SpawnPlan::from_document(&document, &resolve).expect_err("scalar fails");
        assert!(matches!(err, StateError::File { .. }));
    }

    fn prefab_doc() -> AuthoredDocument {
        use crate::authored::Prefab;
        let mut document = doc();
        document.prefabs.insert(
            "goblin".to_owned(),
            Prefab {
                base: None,
                overrides: BTreeMap::from([
                    (
                        "test.health".to_owned(),
                        serde_json::json!({"hp": 10.0, "armor": 2.0}),
                    ),
                    (
                        "test.name".to_owned(),
                        serde_json::json!({"label": "goblin"}),
                    ),
                ]),
            },
        );
        document
    }

    #[test]
    fn prefab_bake_merges_base_then_instance_overrides() {
        let mut document = prefab_doc();
        document.sections.insert(
            "entity.chief".to_owned(),
            serde_json::json!({
                "prefab": "goblin",
                "test.health": { "hp": 99.0 },
            }),
        );

        let plan = SpawnPlan::from_document(&document, &resolve).expect("plan");

        assert_eq!(plan.entities.len(), 1);
        assert_eq!(plan.entities[0].prefab.as_deref(), Some("goblin"));
        let schemas: Vec<&str> = plan.entities[0]
            .components
            .iter()
            .map(|component| component.schema.as_str())
            .collect();
        assert_eq!(schemas, vec!["test.health", "test.name"]);
        let health = &plan.entities[0].components[0].fields;
        assert_eq!(health.get("hp"), Some(&SnapshotValue::F64(99.0)));
        assert_eq!(
            health.get("armor"),
            Some(&SnapshotValue::F64(2.0)),
            "unoverridden base field survives"
        );
    }

    #[test]
    fn prefab_bake_output_equals_the_hand_written_equivalent() {
        let mut baked_doc = prefab_doc();
        baked_doc.sections.insert(
            "entity.chief".to_owned(),
            serde_json::json!({
                "prefab": "goblin",
                "test.health": { "hp": 99.0 },
            }),
        );
        let mut hand_doc = doc();
        hand_doc.sections.insert(
            "entity.chief".to_owned(),
            serde_json::json!({
                "test.health": { "hp": 99.0, "armor": 2.0 },
                "test.name": { "label": "goblin" },
            }),
        );

        let baked = SpawnPlan::from_document(&baked_doc, &resolve).expect("baked plan");
        let mut hand = SpawnPlan::from_document(&hand_doc, &resolve).expect("hand plan");
        // Provenance is the only difference: the component payloads match.
        for entity in &mut hand.entities {
            entity.prefab = Some("goblin".to_owned());
        }
        assert_eq!(baked, hand);
    }

    #[test]
    fn baking_leaves_the_authored_document_unchanged() {
        let mut document = prefab_doc();
        document.sections.insert(
            "entity.chief".to_owned(),
            serde_json::json!({
                "prefab": "goblin",
                "test.health": { "hp": 99.0 },
            }),
        );
        let before = document.clone();

        SpawnPlan::from_document(&document, &resolve).expect("plan");

        assert_eq!(document, before, "bake must not rewrite authored state");
    }

    #[test]
    fn unknown_prefab_is_a_typed_error() {
        let mut document = doc();
        document.sections.insert(
            "entity.lost".to_owned(),
            serde_json::json!({ "prefab": "no-such-prefab" }),
        );

        let err = SpawnPlan::from_document(&document, &resolve).expect_err("unknown prefab");
        assert!(matches!(err, StateError::File { .. }));
    }

    #[test]
    fn non_string_prefab_key_is_a_typed_error() {
        let mut document = doc();
        document.sections.insert(
            "entity.lost".to_owned(),
            serde_json::json!({ "prefab": {"base": "goblin"} }),
        );

        let err = SpawnPlan::from_document(&document, &resolve).expect_err("object prefab");
        assert!(matches!(err, StateError::File { .. }));
    }

    #[test]
    fn chained_prefab_base_fails_the_bake() {
        use crate::authored::Prefab;
        let mut document = doc();
        // `leaf` names `mid`, and `mid` itself names `root`: two levels of
        // inheritance, so baking `leaf` must fail.
        for (name, base) in [("root", None), ("mid", Some("root")), ("leaf", Some("mid"))] {
            document.prefabs.insert(
                name.to_owned(),
                Prefab {
                    base: base.map(str::to_owned),
                    overrides: BTreeMap::from([(
                        "test.health".to_owned(),
                        serde_json::json!({"hp": 1.0}),
                    )]),
                },
            );
        }
        document.sections.insert(
            "entity.chain".to_owned(),
            serde_json::json!({ "prefab": "leaf" }),
        );

        // `b` itself names base `a`: one level only, so the bake fails here —
        // before any caller spawns — with zero world contact.
        let err = SpawnPlan::from_document(&document, &resolve).expect_err("chain fails");
        assert!(matches!(err, StateError::File { .. }));
    }

    #[test]
    fn asset_markers_inside_prefabs_resolve() {
        use crate::authored::Prefab;
        let mut document = doc();
        document.prefabs.insert(
            "sprite".to_owned(),
            Prefab {
                base: None,
                overrides: BTreeMap::from([(
                    "test.sprite".to_owned(),
                    serde_json::json!({"tex": { "$asset": "hero.png" }}),
                )]),
            },
        );
        document.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({ "prefab": "sprite" }),
        );

        let plan = SpawnPlan::from_document(&document, &resolve).expect("plan");

        assert_eq!(
            plan.entities[0].components[0].fields.get("tex"),
            Some(&SnapshotValue::Str("resolved/hero.png".to_owned()))
        );
    }

    #[test]
    fn non_string_asset_marker_is_a_typed_error() {
        // A hostile `{ "$asset": <non-string> }` marker must fail typed at
        // plan time, never reach a decoder as a half-resolved value.
        let mut document = doc();
        document.sections.insert(
            "entity.ghost".to_owned(),
            serde_json::json!({ "test.sprite": { "tex": { "$asset": 42 } } }),
        );

        let err = SpawnPlan::from_document(&document, &resolve).expect_err("marker fails");
        assert!(matches!(err, StateError::File { .. }));
    }

    #[test]
    fn asset_markers_resolve_inside_arrays_and_nested_objects() {
        // Markers nest arbitrarily: inside arrays, inside objects inside
        // arrays. Every one must resolve before fields are built.
        let mut document = doc();
        document.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({ "test.sprite": {
                "frames": [{ "$asset": "a.png" }, { "$asset": "b.png" }],
                "nested": { "deep": [{ "$asset": "c.png" }] },
            } }),
        );

        let plan = SpawnPlan::from_document(&document, &resolve).expect("plan");

        let fields = &plan.entities[0].components[0].fields;
        assert_eq!(
            fields.get("frames"),
            Some(&SnapshotValue::List(vec![
                SnapshotValue::Str("resolved/a.png".to_owned()),
                SnapshotValue::Str("resolved/b.png".to_owned()),
            ])),
            "array markers resolve in order"
        );
        assert_eq!(
            fields.get("nested"),
            Some(&SnapshotValue::Map(BTreeMap::from([(
                "deep".to_owned(),
                SnapshotValue::List(vec![SnapshotValue::Str("resolved/c.png".to_owned())]),
            )]))),
            "deeply nested markers resolve"
        );
    }

    #[test]
    fn prefab_only_instance_bakes_the_whole_prefab() {
        // An instance naming only a prefab (no per-instance components)
        // bakes every prefab schema whole: no silent empty spawn.
        let mut document = prefab_doc();
        document.sections.insert(
            "entity.grunt".to_owned(),
            serde_json::json!({ "prefab": "goblin" }),
        );

        let plan = SpawnPlan::from_document(&document, &resolve).expect("plan");

        assert_eq!(plan.entities.len(), 1);
        assert_eq!(plan.entities[0].prefab.as_deref(), Some("goblin"));
        let schemas: Vec<&str> = plan.entities[0]
            .components
            .iter()
            .map(|component| component.schema.as_str())
            .collect();
        assert_eq!(schemas, vec!["test.health", "test.name"]);
        assert_eq!(
            plan.entities[0].components[0].fields.get("hp"),
            Some(&SnapshotValue::F64(10.0)),
            "prefab base value survives without overrides"
        );
    }
}
