//! Staged authored spawn: validate-all before the first [`World::spawn`].
//!
//! [`AuthoredSpawner::spawn`] runs in two phases over a
//! [`SpawnPlan`](canary_state::SpawnPlan) built from the document without
//! touching the [`World`](canary_ecs::World): first it checks every planned
//! component against the world's schema registry **and** decodes every field
//! set into a [`StagedInsert`], then — only once everything validated — it
//! spawns each entity and runs the staged typed inserts. An unknown schema,
//! an unresolvable asset, or a decode failure aborts before the first spawn,
//! so callers never need a scratch world for all-or-nothing spawning.
//!
//! The game owns concrete component types behind [`SpawnDecoder`]: `decode`
//! is pure (it must not touch the world) and returns a [`StagedInsert`],
//! usually via [`StagedInsert::stage`], whose apply step uses typed
//! [`World::insert`](canary_ecs::World::insert). The erased overwrite path
//! (`set_erased`) can never place a component on a fresh spawn — it only
//! overwrites components the entity already has — so the staged typed-insert
//! path is the only correct one.

use std::collections::BTreeMap;

use canary_ecs::{Entity, World};
use canary_state::{AuthoredDocument, SchemaId, SnapshotValue, SpawnPlan, StateError};

/// The staged apply step: runs the captured typed insert on a fresh entity.
type StagedApply = Box<dyn FnOnce(&mut World, Entity) -> Result<(), StateError>>;

/// Where one spawn landed, keyed by authored-local name.
#[derive(Debug, Default)]
pub struct SpawnReport {
    /// Authored-local entity name to fresh runtime entity.
    pub entities: BTreeMap<String, Entity>,
}

impl SpawnReport {
    /// Looks up the fresh entity for an authored-local name.
    #[must_use]
    pub fn entity(&self, local: &str) -> Option<Entity> {
        self.entities.get(local).copied()
    }
}

/// A decoded component awaiting placement on a fresh entity.
///
/// The closure captures a concrete typed value and inserts it with
/// [`World::insert`](canary_ecs::World::insert) — never the erased
/// overwrite path, which cannot add a component the entity lacks.
pub struct StagedInsert {
    insert: StagedApply,
}

impl StagedInsert {
    /// Stages a concrete component value for typed insert on a fresh entity.
    /// Insert on a just-spawned entity only fails if the handle went stale,
    /// which the spawner makes impossible; the error path stays typed.
    pub fn stage<T: Send + Sync + 'static>(component: T) -> Self {
        Self {
            insert: Box::new(|world, entity| {
                world
                    .insert(entity, component)
                    .map_err(|error| StateError::PlacementFailed {
                        entity: format!("{entity:?}"),
                        reason: error.to_string(),
                    })
            }),
        }
    }

    /// Runs the staged typed insert on `entity`.
    pub fn insert(self, world: &mut World, entity: Entity) -> Result<(), StateError> {
        (self.insert)(world, entity)
    }
}

impl std::fmt::Debug for StagedInsert {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StagedInsert")
            .finish_non_exhaustive()
    }
}

/// Game-owned decode seam: pure translation of canonical fields into a
/// staged typed insert. One trait, not a codec-plus-placer pair — decoding
/// and the typed insert travel together inside [`StagedInsert`], so the two
/// can never disagree about the component type.
pub trait SpawnDecoder {
    /// Decodes canonical `fields` for `schema` into a staged insert. Must
    /// not touch any `World`; failures abort the spawn before the first
    /// entity exists.
    fn decode(
        &self,
        schema: &SchemaId,
        fields: &BTreeMap<String, SnapshotValue>,
    ) -> Result<StagedInsert, StateError>;
}

/// Spawns authored documents into a world. Holds the two caller-owned
/// seams — component decoder and asset resolver — so `spawn` itself stays
/// a two-argument call.
pub struct AuthoredSpawner<'a> {
    decoder: &'a dyn SpawnDecoder,
    assets: &'a dyn Fn(&str) -> Option<String>,
}

impl<'a> AuthoredSpawner<'a> {
    /// Borrows the caller-owned seams. Both outlive the spawn call.
    #[must_use]
    pub fn new(decoder: &'a dyn SpawnDecoder, assets: &'a dyn Fn(&str) -> Option<String>) -> Self {
        Self { decoder, assets }
    }

    /// Spawns every `entity.<local>` section into fresh runtime entities.
    /// Sections naming a `prefab` bake it first (one level: prefab base
    /// fields, then the instance's own fields win per field — see
    /// `canary-state`'s spawn plan); the authored document itself is never
    /// rewritten, only read. Non-entity sections (settings, future tables)
    /// are ignored. Components place in section order. Validation (prefab
    /// bake, asset resolution, registry agreement, decode) completes for
    /// every entity before the first spawn, so any failure leaves the world
    /// untouched.
    pub fn spawn(
        &self,
        world: &mut World,
        doc: &AuthoredDocument,
    ) -> Result<SpawnReport, StateError> {
        let plan = SpawnPlan::from_document(doc, self.assets)?;
        let mut staged: Vec<(String, Vec<StagedInsert>)> = Vec::with_capacity(plan.entities.len());
        for entity in &plan.entities {
            let mut parts = Vec::with_capacity(entity.components.len());
            for component in &entity.components {
                // Registration first: the world is authoritative for which
                // schemas it understands, and decoding an unregistered
                // schema would only fail later and louder.
                if world
                    .type_id_for_schema(component.schema.as_str())
                    .is_none()
                {
                    return Err(StateError::UnknownSchema(
                        component.schema.as_str().to_owned(),
                    ));
                }
                parts.push(self.decoder.decode(&component.schema, &component.fields)?);
            }
            staged.push((entity.local.clone(), parts));
        }
        let mut report = SpawnReport::default();
        for (local, parts) in staged {
            let entity = world.spawn();
            for staged_insert in parts {
                staged_insert.insert(world, entity)?;
            }
            report.entities.insert(local, entity);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_ecs::CanaryComponent;
    use canary_state::ProjectId;
    use std::cell::RefCell;

    #[derive(Debug, Clone, PartialEq)]
    struct Health {
        hp: f32,
    }

    impl CanaryComponent for Health {
        const SCHEMA_ID: &'static str = "test.health";
    }

    struct TestDecoder {
        seen: RefCell<Vec<BTreeMap<String, SnapshotValue>>>,
    }

    impl SpawnDecoder for TestDecoder {
        fn decode(
            &self,
            schema: &SchemaId,
            fields: &BTreeMap<String, SnapshotValue>,
        ) -> Result<StagedInsert, StateError> {
            assert_eq!(schema.as_str(), Health::SCHEMA_ID);
            self.seen.borrow_mut().push(fields.clone());
            let hp = match fields.get("hp") {
                Some(SnapshotValue::F64(hp)) => *hp as f32,
                other => panic!("expected hp field, got {other:?}"),
            };
            Ok(StagedInsert::stage(Health { hp }))
        }
    }

    struct FailingDecoder;

    impl SpawnDecoder for FailingDecoder {
        fn decode(
            &self,
            _schema: &SchemaId,
            _fields: &BTreeMap<String, SnapshotValue>,
        ) -> Result<StagedInsert, StateError> {
            Err(StateError::File {
                path: std::path::PathBuf::from("<test>"),
                reason: "decode boom".to_owned(),
            })
        }
    }

    fn doc_with_entities() -> AuthoredDocument {
        let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
        doc.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({ "test.health": { "hp": 50.0 } }),
        );
        doc.sections.insert(
            "entity.sidekick".to_owned(),
            serde_json::json!({ "test.health": { "hp": 30.0 } }),
        );
        doc
    }

    fn decoder() -> TestDecoder {
        TestDecoder {
            seen: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn spawn_places_decoded_components_on_fresh_entities() {
        let doc = doc_with_entities();
        let mut world = World::new();
        world.register_component::<Health>().expect("register");
        let codec = decoder();
        let spawner = AuthoredSpawner::new(&codec, &|id| Some(format!("resolved/{id}")));

        let report = spawner.spawn(&mut world, &doc).expect("spawn");

        let hero = report.entity("hero").expect("hero mapped");
        let sidekick = report.entity("sidekick").expect("sidekick mapped");
        assert_ne!(hero, sidekick);
        assert_eq!(world.get::<Health>(hero).expect("hero health").hp, 50.0);
        assert_eq!(
            world.get::<Health>(sidekick).expect("sidekick health").hp,
            30.0
        );
    }

    #[test]
    fn asset_markers_resolve_before_decode() {
        let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
        doc.sections.insert(
            "entity.hero".to_owned(),
            serde_json::json!({ "test.health": { "hp": 10.0, "sprite": { "$asset": "hero.png" } } }),
        );
        let mut world = World::new();
        world.register_component::<Health>().expect("register");
        let codec = decoder();
        let spawner = AuthoredSpawner::new(&codec, &|id| Some(format!("resolved/{id}")));

        spawner.spawn(&mut world, &doc).expect("spawn");

        let seen = codec.seen.borrow();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].get("sprite"),
            Some(&SnapshotValue::Str("resolved/hero.png".to_owned()))
        );
    }

    #[test]
    fn unknown_schema_is_a_typed_error_before_the_first_spawn() {
        let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
        // `entity.aaa-valid` sorts before `entity.zzz-ghost`: the valid
        // entity must still be unspawned when the ghost schema fails.
        doc.sections.insert(
            "entity.aaa-valid".to_owned(),
            serde_json::json!({ "test.health": { "hp": 1.0 } }),
        );
        doc.sections.insert(
            "entity.zzz-ghost".to_owned(),
            serde_json::json!({ "test.ghost": { "hp": 1.0 } }),
        );
        let mut world = World::new();
        world.register_component::<Health>().expect("register");
        let codec = decoder();
        let spawner = AuthoredSpawner::new(&codec, &|id| Some(format!("resolved/{id}")));

        let err = spawner
            .spawn(&mut world, &doc)
            .expect_err("ghost schema unknown");
        assert!(matches!(err, StateError::UnknownSchema(_)));
        assert_eq!(
            world.query::<Health>().count(),
            0,
            "no entity may spawn before validation completes"
        );
    }

    #[test]
    fn decode_failure_aborts_before_the_first_spawn() {
        let doc = doc_with_entities();
        let mut world = World::new();
        world.register_component::<Health>().expect("register");
        let codec = FailingDecoder;
        let spawner = AuthoredSpawner::new(&codec, &|id| Some(format!("resolved/{id}")));

        spawner.spawn(&mut world, &doc).expect_err("decode fails");

        assert_eq!(
            world.query::<Health>().count(),
            0,
            "no entity may spawn before validation completes"
        );
    }

    #[test]
    fn unresolved_asset_aborts_before_the_first_spawn() {
        let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
        doc.sections.insert(
            "entity.aaa-valid".to_owned(),
            serde_json::json!({ "test.health": { "hp": 1.0 } }),
        );
        doc.sections.insert(
            "entity.zzz-marked".to_owned(),
            serde_json::json!({ "test.health": { "hp": 1.0, "tex": { "$asset": "gone.png" } } }),
        );
        let mut world = World::new();
        world.register_component::<Health>().expect("register");
        let codec = decoder();
        let spawner = AuthoredSpawner::new(&codec, &|_| None);

        let err = spawner.spawn(&mut world, &doc).expect_err("asset missing");
        assert!(matches!(err, StateError::AssetUnresolved { .. }));
        assert_eq!(
            world.query::<Health>().count(),
            0,
            "no entity may spawn before validation completes"
        );
    }

    fn prefab_doc() -> AuthoredDocument {
        use canary_state::Prefab;
        use std::collections::BTreeMap;
        let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
        doc.prefabs.insert(
            "goblin".to_owned(),
            Prefab {
                base: None,
                overrides: BTreeMap::from([(
                    "test.health".to_owned(),
                    serde_json::json!({"hp": 10.0}),
                )]),
            },
        );
        doc
    }

    #[test]
    fn prefab_instance_spawns_baked_components() {
        let mut doc = prefab_doc();
        doc.sections.insert(
            "entity.chief".to_owned(),
            serde_json::json!({
                "prefab": "goblin",
                "test.health": { "hp": 99.0 },
            }),
        );
        let mut world = World::new();
        world.register_component::<Health>().expect("register");
        let codec = decoder();
        let spawner = AuthoredSpawner::new(&codec, &|id| Some(format!("resolved/{id}")));

        let report = spawner.spawn(&mut world, &doc).expect("spawn");

        let chief = report.entity("chief").expect("chief mapped");
        assert_eq!(world.get::<Health>(chief).expect("health").hp, 99.0);
        // The authored representation survives the bake: prefab table and
        // instance reference are still in the document.
        assert!(doc.prefabs.contains_key("goblin"));
        assert_eq!(
            doc.sections["entity.chief"].get("prefab"),
            Some(&serde_json::json!("goblin"))
        );
    }

    #[test]
    fn prefab_spawn_matches_the_hand_written_equivalent() {
        let mut baked_doc = prefab_doc();
        baked_doc.sections.insert(
            "entity.chief".to_owned(),
            serde_json::json!({
                "prefab": "goblin",
                "test.health": { "hp": 99.0 },
            }),
        );
        let mut hand_doc = AuthoredDocument::new(baked_doc.project);
        hand_doc.sections.insert(
            "entity.chief".to_owned(),
            serde_json::json!({ "test.health": { "hp": 99.0 } }),
        );
        let spawner_assets = |id: &str| Some(format!("resolved/{id}"));

        let mut baked_world = World::new();
        baked_world
            .register_component::<Health>()
            .expect("register");
        let baked_codec = decoder();
        let baked_spawner = AuthoredSpawner::new(&baked_codec, &spawner_assets);
        let baked_report = baked_spawner
            .spawn(&mut baked_world, &baked_doc)
            .expect("spawn");

        let mut hand_world = World::new();
        hand_world.register_component::<Health>().expect("register");
        let hand_codec = decoder();
        let hand_spawner = AuthoredSpawner::new(&hand_codec, &spawner_assets);
        let hand_report = hand_spawner
            .spawn(&mut hand_world, &hand_doc)
            .expect("spawn");

        assert_eq!(
            baked_world
                .get::<Health>(baked_report.entity("chief").expect("chief"))
                .expect("health"),
            hand_world
                .get::<Health>(hand_report.entity("chief").expect("chief"))
                .expect("health"),
            "bake output equals the hand-written equivalent"
        );
    }

    #[test]
    fn unknown_prefab_aborts_before_the_first_spawn() {
        let mut doc = prefab_doc();
        doc.sections.insert(
            "entity.aaa-valid".to_owned(),
            serde_json::json!({ "test.health": { "hp": 1.0 } }),
        );
        doc.sections.insert(
            "entity.zzz-lost".to_owned(),
            serde_json::json!({ "prefab": "no-such-prefab" }),
        );
        let mut world = World::new();
        world.register_component::<Health>().expect("register");
        let codec = decoder();
        let spawner = AuthoredSpawner::new(&codec, &|id| Some(format!("resolved/{id}")));

        spawner
            .spawn(&mut world, &doc)
            .expect_err("unknown prefab fails");
        assert_eq!(
            world.query::<Health>().count(),
            0,
            "prefab bake failure mutates nothing"
        );
    }

    #[test]
    fn chained_prefab_aborts_before_the_first_spawn() {
        use canary_state::Prefab;
        use std::collections::BTreeMap;
        let mut doc = AuthoredDocument::new(ProjectId::generate().expect("os randomness"));
        for (name, base) in [("root", None), ("mid", Some("root")), ("leaf", Some("mid"))] {
            doc.prefabs.insert(
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
        let mut bystander_world = World::new();
        bystander_world
            .register_component::<Health>()
            .expect("register");
        let bystander = bystander_world.spawn();
        bystander_world
            .insert(bystander, Health { hp: 5.0 })
            .expect("insert");
        doc.sections.insert(
            "entity.chain".to_owned(),
            serde_json::json!({ "prefab": "leaf" }),
        );
        let codec = decoder();
        let spawner = AuthoredSpawner::new(&codec, &|id| Some(format!("resolved/{id}")));

        spawner
            .spawn(&mut bystander_world, &doc)
            .expect_err("chained prefab fails");
        assert_eq!(
            bystander_world.query::<Health>().count(),
            1,
            "prefab bake failure mutates nothing"
        );
        assert_eq!(
            bystander_world.get::<Health>(bystander).expect("health").hp,
            5.0,
            "pre-existing entities survive the abort"
        );
    }
}
