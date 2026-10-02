//! Project-state seam: authored decoding plus the snapshot boundary.
//!
//! [`CollectathonDecoder`] translates `entity.<local>` sections of the room
//! document into staged typed inserts without touching any `World`.
//! [`Player`], [`Pickup`], and [`Score`] each implement the simulation
//! boundary, and [`GameStats`] captures the deterministic resource side;
//! [`snapshot_registry`] declares the full boundary in one place.

use std::collections::BTreeMap;

use canary_ecs::{CanaryComponent, Entity, World};
use canary_runtime::{
    ComponentBinding, ResourceBinding, SimComponent, SimResource, SnapshotRegistry, SpawnDecoder,
    StagedInsert,
};
use canary_state::{RemapTable, SchemaId, SchemaVersion, SnapshotValue, StateError};

use crate::{Pickup, Player, Score};

/// Game-owned authored decode seam: pure translation of canonical fields
/// into a staged typed insert.
pub struct CollectathonDecoder;

impl CollectathonDecoder {
    /// Reads a finite float field or reports which field was missing.
    fn float_field(
        fields: &BTreeMap<String, SnapshotValue>,
        name: &str,
    ) -> Result<f32, StateError> {
        match fields.get(name) {
            Some(SnapshotValue::F64(value)) => Ok(*value as f32),
            _ => Err(StateError::File {
                path: "<collectathon-room>".into(),
                reason: format!("collectathon section is missing float field '{name}'"),
            }),
        }
    }

    /// Reads an unsigned integer field or reports which field was missing.
    /// Accepts both integer arms: authored JSON prefers the exact `I64`
    /// arm for small literals (see `SnapshotValue::from_json`), so a
    /// `points: 0` in the room document arrives as `I64`, not `U64`.
    fn int_field(fields: &BTreeMap<String, SnapshotValue>, name: &str) -> Result<u32, StateError> {
        let not_u32 = || StateError::File {
            path: "<collectathon-room>".into(),
            reason: format!("collectathon field '{name}' does not fit u32"),
        };
        match fields.get(name) {
            Some(SnapshotValue::U64(value)) => u32::try_from(*value).map_err(|_| not_u32()),
            Some(SnapshotValue::I64(value)) => u32::try_from(*value).map_err(|_| not_u32()),
            _ => Err(StateError::File {
                path: "<collectathon-room>".into(),
                reason: format!("collectathon section is missing integer field '{name}'"),
            }),
        }
    }

    /// Reads a boolean field or reports which field was missing.
    fn bool_field(
        fields: &BTreeMap<String, SnapshotValue>,
        name: &str,
    ) -> Result<bool, StateError> {
        match fields.get(name) {
            Some(SnapshotValue::Bool(value)) => Ok(*value),
            _ => Err(StateError::File {
                path: "<collectathon-room>".into(),
                reason: format!("collectathon section is missing boolean field '{name}'"),
            }),
        }
    }
}

impl SpawnDecoder for CollectathonDecoder {
    fn decode(
        &self,
        schema: &SchemaId,
        fields: &BTreeMap<String, SnapshotValue>,
    ) -> Result<StagedInsert, StateError> {
        match schema.as_str() {
            Player::SCHEMA_ID => Ok(StagedInsert::stage(Player {
                x: Self::float_field(fields, "x")?,
                y: Self::float_field(fields, "y")?,
            })),
            Pickup::SCHEMA_ID => Ok(StagedInsert::stage(Pickup {
                x: Self::float_field(fields, "x")?,
                y: Self::float_field(fields, "y")?,
                collected: Self::bool_field(fields, "collected")?,
                is_goal: Self::bool_field(fields, "is_goal")?,
            })),
            Score::SCHEMA_ID => Ok(StagedInsert::stage(Score {
                points: Self::int_field(fields, "points")?,
            })),
            _ => Err(StateError::UnknownSchema(schema.as_str().to_owned())),
        }
    }
}

/// Deterministic run statistics, captured as a simulation resource.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GameStats {
    /// Pickups collected so far, goal included.
    pub collected: u32,
    /// Total pickups in the room, goal included.
    pub goal: u32,
}

impl Player {
    /// Reads one float snapshot field back into an `f32`.
    fn read_float(fields: &BTreeMap<String, SnapshotValue>, name: &str) -> Result<f32, StateError> {
        snapshot_float(Player::SCHEMA_ID, fields, name)
    }
}

/// Reads one finite float snapshot field for `schema`.
fn snapshot_float(
    schema: &str,
    fields: &BTreeMap<String, SnapshotValue>,
    name: &str,
) -> Result<f32, StateError> {
    match fields.get(name) {
        Some(SnapshotValue::F64(value)) if value.is_finite() => Ok(*value as f32),
        _ => Err(StateError::MigrationInvalid {
            schema: schema.to_owned(),
            to: 1,
            reason: format!("snapshot is missing finite float field '{name}'"),
        }),
    }
}

impl SimComponent for Player {
    fn write_snapshot(
        world: &World,
        entity: Entity,
        _remap: &mut RemapTable,
    ) -> Option<BTreeMap<String, SnapshotValue>> {
        let player = world.get::<Player>(entity)?;
        BTreeMap::from([
            ("x".to_owned(), SnapshotValue::F64(f64::from(player.x))),
            ("y".to_owned(), SnapshotValue::F64(f64::from(player.y))),
        ])
        .into()
    }

    fn apply_snapshot(
        fields: &BTreeMap<String, SnapshotValue>,
        _resolve: &dyn Fn(u32) -> Option<Entity>,
    ) -> Result<Self, StateError> {
        Ok(Player {
            x: Self::read_float(fields, "x")?,
            y: Self::read_float(fields, "y")?,
        })
    }
}

impl SimComponent for Pickup {
    fn write_snapshot(
        world: &World,
        entity: Entity,
        _remap: &mut RemapTable,
    ) -> Option<BTreeMap<String, SnapshotValue>> {
        let pickup = world.get::<Pickup>(entity)?;
        BTreeMap::from([
            ("x".to_owned(), SnapshotValue::F64(f64::from(pickup.x))),
            ("y".to_owned(), SnapshotValue::F64(f64::from(pickup.y))),
            (
                "collected".to_owned(),
                SnapshotValue::Bool(pickup.collected),
            ),
            ("is_goal".to_owned(), SnapshotValue::Bool(pickup.is_goal)),
        ])
        .into()
    }

    fn apply_snapshot(
        fields: &BTreeMap<String, SnapshotValue>,
        _resolve: &dyn Fn(u32) -> Option<Entity>,
    ) -> Result<Self, StateError> {
        let bool_field = |name: &str| match fields.get(name) {
            Some(SnapshotValue::Bool(value)) => Ok(*value),
            _ => Err(StateError::MigrationInvalid {
                schema: Pickup::SCHEMA_ID.to_owned(),
                to: 1,
                reason: format!("snapshot is missing boolean field '{name}'"),
            }),
        };
        Ok(Pickup {
            x: snapshot_float(Pickup::SCHEMA_ID, fields, "x")?,
            y: snapshot_float(Pickup::SCHEMA_ID, fields, "y")?,
            collected: bool_field("collected")?,
            is_goal: bool_field("is_goal")?,
        })
    }
}

impl SimComponent for Score {
    fn write_snapshot(
        world: &World,
        entity: Entity,
        _remap: &mut RemapTable,
    ) -> Option<BTreeMap<String, SnapshotValue>> {
        let score = world.get::<Score>(entity)?;
        BTreeMap::from([(
            "points".to_owned(),
            SnapshotValue::U64(u64::from(score.points)),
        )])
        .into()
    }

    fn apply_snapshot(
        fields: &BTreeMap<String, SnapshotValue>,
        _resolve: &dyn Fn(u32) -> Option<Entity>,
    ) -> Result<Self, StateError> {
        match fields.get("points") {
            Some(SnapshotValue::U64(points)) => u32::try_from(*points)
                .map(|points| Score { points })
                .map_err(|_| StateError::MigrationInvalid {
                    schema: Score::SCHEMA_ID.to_owned(),
                    to: 1,
                    reason: "snapshot points do not fit u32".to_owned(),
                }),
            _ => Err(StateError::MigrationInvalid {
                schema: Score::SCHEMA_ID.to_owned(),
                to: 1,
                reason: "snapshot is missing integer field 'points'".to_owned(),
            }),
        }
    }
}

impl SimResource for GameStats {
    const SCHEMA_ID: &'static str = "collectathon.stats@1";

    fn read_snapshot(
        world: &World,
        _remap: &mut RemapTable,
    ) -> Option<BTreeMap<String, SnapshotValue>> {
        let stats = world.resource::<GameStats>()?;
        BTreeMap::from([
            (
                "collected".to_owned(),
                SnapshotValue::U64(u64::from(stats.collected)),
            ),
            ("goal".to_owned(), SnapshotValue::U64(u64::from(stats.goal))),
        ])
        .into()
    }

    fn write_snapshot(
        world: &mut World,
        fields: &BTreeMap<String, SnapshotValue>,
        _resolve: &dyn Fn(u32) -> Option<Entity>,
    ) -> Result<(), StateError> {
        let int_field = |name: &str| match fields.get(name) {
            Some(SnapshotValue::U64(value)) => {
                u32::try_from(*value).map_err(|_| StateError::MigrationInvalid {
                    schema: GameStats::SCHEMA_ID.to_owned(),
                    to: 1,
                    reason: format!("snapshot field '{name}' does not fit u32"),
                })
            }
            _ => Err(StateError::MigrationInvalid {
                schema: GameStats::SCHEMA_ID.to_owned(),
                to: 1,
                reason: format!("snapshot is missing integer field '{name}'"),
            }),
        };
        world.insert_resource(GameStats {
            collected: int_field("collected")?,
            goal: int_field("goal")?,
        });
        Ok(())
    }
}

/// Declares the simulation boundary: the snapshot envelope identity plus
/// one binding per simulated component and resource.
#[must_use]
pub fn snapshot_registry() -> SnapshotRegistry {
    SnapshotRegistry::new(
        SchemaId::new("collectathon.snapshot@1"),
        SchemaVersion(1),
        vec![
            ComponentBinding::of::<Player>(),
            ComponentBinding::of::<Pickup>(),
            ComponentBinding::of::<Score>(),
        ],
        vec![ResourceBinding::of::<GameStats>()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_assets::{AssetHandle, Sound};
    use canary_audio::AudioSource;
    use canary_physics::Velocity;

    use crate::game::register_game_components;

    /// Builds a game world for the determinism proof: the player carries
    /// the score (the room's shape), one shard plus the goal wait, and the
    /// run stats count one collection. A presentation-only velocity rides
    /// the player and a stopped voice rides the shard — both outside the
    /// snapshot boundary, so capture must never see them.
    fn determinism_world() -> World {
        let mut world = World::new();
        register_game_components(&mut world).expect("game components register");
        let player = world.spawn();
        world
            .insert(player, Player { x: 1.0, y: -2.0 })
            .expect("player takes Player");
        world
            .insert(player, Score { points: 1 })
            .expect("player takes Score");
        world
            .insert(player, Velocity::zero())
            .expect("player takes its presentation velocity");
        let shard = world.spawn();
        world
            .insert(
                shard,
                Pickup {
                    x: -60.0,
                    y: 20.0,
                    collected: false,
                    is_goal: false,
                },
            )
            .expect("shard takes Pickup");
        world
            .insert(
                shard,
                AudioSource::new(AssetHandle::<Sound>::from_raw_parts(0, 0)),
            )
            .expect("shard takes its presentation voice");
        let goal = world.spawn();
        world
            .insert(
                goal,
                Pickup {
                    x: 100.0,
                    y: -80.0,
                    collected: false,
                    is_goal: true,
                },
            )
            .expect("goal takes Pickup");
        world.insert_resource(GameStats {
            collected: 1,
            goal: 2,
        });
        world
    }

    #[test]
    fn capture_restore_recapture_is_byte_stable() {
        let registry = snapshot_registry();
        let world = determinism_world();
        let (first_bytes, _) = registry.capture_bytes(&world).expect("capture");

        // Fresh world: restore spawns new entities for every snapshot
        // record, and the recapture must assign identical canonical IDs.
        let mut fresh = World::new();
        register_game_components(&mut fresh).expect("game components register");
        registry
            .restore_bytes(&mut fresh, &first_bytes)
            .expect("restore");
        let (second_bytes, _) = registry.capture_bytes(&fresh).expect("recapture");
        assert_eq!(
            first_bytes, second_bytes,
            "fresh-world capture/restore/recapture must be byte-stable"
        );

        // Same world: restore despawns in reverse sorted order (the LIFO
        // discipline — the free stack pops ascending), so spawning in
        // canonical ID order lands each fresh entity on an ascending slot
        // and the recapture assigns identical canonical IDs.
        let mut lived = world;
        registry
            .restore_bytes(&mut lived, &first_bytes)
            .expect("restore");
        let (third_bytes, _) = registry.capture_bytes(&lived).expect("recapture");
        assert_eq!(
            first_bytes, third_bytes,
            "same-world capture/restore/recapture must be byte-stable"
        );
    }
}
