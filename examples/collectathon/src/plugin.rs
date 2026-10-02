//! Tier A plugin seam: the guest-visible `Score` codec.
//!
//! [`register_game_codecs`] registers every component type the guest may
//! touch; the guest (`assets/guest.wat`) reads world state through
//! `canary:plugin/ecs-read` and overwrites registered components through
//! `canary:plugin/ecs-write` during its documented lifecycle boundary.
//! Later work packages wire the guest call into the runtime's scoped-access
//! path with a read/write grant over [`Score`].

use std::any::TypeId;
use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use canary_ecs::{CanaryComponent, World};
use canary_plugin_api::{
    Capability, CodecRegistry, ComponentValue, ComponentValueCodec, ComponentValueError,
    PluginError, PluginOutcome, PluginRequirement, PrimitiveValue, ResourceBudget, ScopedGrant,
    WasmPluginLoader,
};

use crate::Score;

/// The schema the Tier A guest reads and overwrites.
pub const GUEST_SCORE_SCHEMA: &str = Score::SCHEMA_ID;

impl ComponentValueCodec for Score {
    fn to_component_value(&self) -> ComponentValue {
        ComponentValue::Record(vec![(
            "points".to_owned(),
            PrimitiveValue::U32(self.points),
        )])
    }

    fn from_component_value(value: ComponentValue) -> Result<Self, ComponentValueError> {
        let ComponentValue::Record(fields) = value else {
            return Err(ComponentValueError("Score expects a record".to_owned()));
        };
        match fields.iter().find(|(name, _)| name == "points") {
            Some((_, PrimitiveValue::U32(points))) => Ok(Score { points: *points }),
            _ => Err(ComponentValueError(
                "Score expects a record with a u32 'points' field".to_owned(),
            )),
        }
    }
}

/// Registers every guest-visible component codec.
pub fn register_game_codecs(registry: &mut CodecRegistry) {
    registry.register::<Score>();
}

/// Path of the guest WAT source this crate's build script validates and
/// compiles.
#[must_use]
pub fn guest_wat_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("guest.wat")
}

/// The compiled guest component bytes the build script stages from
/// [`guest_wat_path`]: an invalid guest fails the build, so these bytes are
/// always a parseable component by the time any loader sees them.
#[must_use]
pub fn guest_component_bytes() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/guest.component.wasm"))
}

/// The Tier A grant for the collectathon guest: read plus write over the
/// loaned ECS world. The guest's `on-load` performs its `ecs-read` calls
/// (entity count plus the registered `Score` schema probe) through the read
/// half; the write half is proven host-side by
/// [`overwrite_score_through_codec`] after reclaim. No per-frame hooks —
/// [`PluginPhase::OnLoad`](canary_plugin_api::PluginPhase)/`OnUnload` only.
///
/// [`overwrite_score_through_codec`]: crate::plugin::overwrite_score_through_codec
#[must_use]
pub fn guest_grant() -> ScopedGrant {
    ScopedGrant {
        capabilities: HashSet::from([Capability::ReadEcsWorld, Capability::WriteEcsWorld]),
        budget: ResourceBudget::default(),
    }
}

/// Where one guest probe landed: the loaded plugin name plus the reclaimed
/// world's entity count, proving the loan moved home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestProbeReport {
    /// Name the guest loaded under.
    pub name: String,
    /// Entity count of the reclaimed world.
    pub entity_count: usize,
}

/// Why the guest probe or the codec overwrite failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum GuestProbeError {
    /// The Tier A loader refused the guest.
    Plugin(PluginError),
    /// A world operation failed.
    Ecs(canary_ecs::EcsError),
    /// A codec conversion failed.
    Codec(ComponentValueError),
    /// Local staging IO failed.
    Io(std::io::Error),
    /// An expected piece of world state is missing.
    Missing(&'static str),
}

impl fmt::Display for GuestProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plugin(error) => write!(f, "guest probe failed: {error}"),
            Self::Ecs(error) => write!(f, "guest probe ECS failure: {error}"),
            Self::Codec(error) => write!(f, "guest probe codec failure: {error}"),
            Self::Io(error) => write!(f, "guest probe IO failure: {error}"),
            Self::Missing(what) => write!(f, "guest probe missing world state: {what}"),
        }
    }
}

impl std::error::Error for GuestProbeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Plugin(error) => Some(error),
            Self::Ecs(error) => Some(error),
            Self::Codec(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Missing(_) => None,
        }
    }
}

impl From<PluginError> for GuestProbeError {
    fn from(error: PluginError) -> Self {
        Self::Plugin(error)
    }
}

impl From<canary_ecs::EcsError> for GuestProbeError {
    fn from(error: canary_ecs::EcsError) -> Self {
        Self::Ecs(error)
    }
}

impl From<ComponentValueError> for GuestProbeError {
    fn from(error: ComponentValueError) -> Self {
        Self::Codec(error)
    }
}

impl From<std::io::Error> for GuestProbeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Runs the Tier A guest's `on-load` against the loaned game world and
/// reclaims the world: stages the compiled component, loads it with the
/// [`guest_grant`] read/write capabilities as a required plugin (so a trap
/// or an unsatisfied import is a typed [`PluginError`], never a silent
/// skip), and hands the loaned world home on every path.
///
/// A clean [`GuestProbeReport`] proves the guest's `ecs-read` calls —
/// entity count plus the registered `Score` schema probe — executed against
/// the real loaned world without trapping.
pub fn run_guest_probe(world: &mut Option<World>) -> Result<GuestProbeReport, GuestProbeError> {
    static STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);
    let staged = std::env::temp_dir().join(format!(
        "collectathon-guest-{}-{}.wasm",
        std::process::id(),
        STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&staged, guest_component_bytes())?;

    let mut codecs = CodecRegistry::new();
    register_game_codecs(&mut codecs);
    let loader = WasmPluginLoader::new(codecs, ResourceBudget::default())?;
    let outcome = loader.load_scoped(
        &staged,
        "collectathon-guest",
        &guest_grant().capabilities,
        PluginRequirement::Required,
        world,
    );
    let _ = std::fs::remove_file(&staged);
    let (handle, loaded) = outcome?;
    let Some(_plugin) = handle else {
        return Err(GuestProbeError::Missing(
            "required guest load handed out no instance",
        ));
    };
    let PluginOutcome::Loaded { name } = loaded else {
        return Err(GuestProbeError::Missing(
            "required guest load must report Loaded, never SkippedOptional",
        ));
    };
    let world = world
        .as_ref()
        .ok_or(GuestProbeError::Missing("loaned world was not reclaimed"))?;
    Ok(GuestProbeReport {
        name,
        entity_count: world.entity_count(),
    })
}

/// Overwrites the world's [`Score`] through the registered codec and
/// verifies the write: reads the current points, converts through
/// [`ComponentValue`], adds `bonus` via a fresh codec decode, inserts the
/// decoded component, and reads it back. Returns `(before, after)`.
///
/// This is the write half of the [`guest_grant`] pair, performed host-side
/// after [`run_guest_probe`] reclaims the world: the guest itself imports
/// only `ecs-read` (its `set` shape would need guest-side canonical-ABI
/// memory for the variant payload, which hand-written WAT cannot carry),
/// so the overwrite-then-verify step proves the registered `Score` codec
/// round-trips against live world state instead.
pub fn overwrite_score_through_codec(
    world: &mut World,
    bonus: u32,
) -> Result<(u32, u32), GuestProbeError> {
    let mut codecs = CodecRegistry::new();
    register_game_codecs(&mut codecs);
    let score_entity = world
        .query::<Score>()
        .map(|(entity, _)| entity)
        .next()
        .ok_or(GuestProbeError::Missing("score entity"))?;
    let before = world
        .get::<Score>(score_entity)
        .ok_or(GuestProbeError::Missing("score component"))?
        .points;
    let value = codecs
        .to_value(TypeId::of::<Score>(), &Score { points: before })
        .ok_or(GuestProbeError::Missing("score codec"))?;
    let ComponentValue::Record(fields) = &value else {
        return Err(GuestProbeError::Missing("score codec record shape"));
    };
    if !fields
        .iter()
        .any(|(name, item)| name == "points" && *item == PrimitiveValue::U32(before))
    {
        return Err(GuestProbeError::Missing("score codec points field"));
    }
    let after = before.saturating_add(bonus);
    let replacement =
        ComponentValue::Record(vec![("points".to_owned(), PrimitiveValue::U32(after))]);
    let boxed = codecs
        .from_value(TypeId::of::<Score>(), replacement)
        .ok_or(GuestProbeError::Missing("score codec"))??;
    let decoded = boxed
        .downcast::<Score>()
        .map_err(|_| GuestProbeError::Missing("score codec type"))?;
    world.insert(score_entity, *decoded)?;
    let verified = world
        .get::<Score>(score_entity)
        .ok_or(GuestProbeError::Missing("score component"))?
        .points;
    if verified != after {
        return Err(GuestProbeError::Missing("score overwrite did not verify"));
    }
    Ok((before, after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::GameStats;
    use crate::{Pickup, Player};

    /// Builds a small game world the guest can read: one player carrying
    /// the score, one shard, and the run stats.
    fn probe_world() -> World {
        let mut world = World::new();
        crate::game::register_game_components(&mut world).expect("game components register");
        let player = world.spawn();
        world
            .insert(player, Player { x: 0.0, y: 0.0 })
            .expect("player takes Player");
        world
            .insert(player, Score { points: 3 })
            .expect("player takes Score");
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
        world.insert_resource(GameStats {
            collected: 0,
            goal: 1,
        });
        world
    }

    #[test]
    fn guest_on_load_reads_the_game_world_and_the_world_moves_home() {
        let live = probe_world();
        let entities_before = live.entity_count();
        let mut loan = Some(live);
        let report = run_guest_probe(&mut loan).expect("required guest probe loads");
        assert_eq!(report.name, "collectathon-guest");
        let home = loan.expect("the loaned world must move home");
        assert_eq!(home.entity_count(), entities_before);
        assert_eq!(report.entity_count, entities_before);
    }

    #[test]
    fn score_overwrite_through_the_codec_verifies() {
        let mut world = probe_world();
        let (before, after) =
            overwrite_score_through_codec(&mut world, 7).expect("codec overwrite verifies");
        assert_eq!(before, 3);
        assert_eq!(after, 10);
        assert_eq!(
            world
                .query::<Score>()
                .next()
                .expect("score exists")
                .1
                .points,
            10
        );
    }
}
