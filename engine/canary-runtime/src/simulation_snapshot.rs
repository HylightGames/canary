//! Deterministic simulation boundary: capture, checksum, staged restore, step.
//!
//! [`SnapshotRegistry`] is the caller-owned simulation boundary over a
//! [`World`](canary_ecs::World). It captures only the component schemas and
//! resource schemas the game binds into it — presentation handles, editor
//! state, pool internals, and every other unbound component or resource
//! never enter a [`Snapshot`](canary_state::Snapshot) — and restores in two
//! phases (validate everything before the first world mutation), mirroring
//! [`AuthoredSpawner`](super::AuthoredSpawner). [`Simulation`] owns the
//! deterministic advance: an [`OwnedRng`](canary_state::OwnedRng) stream plus
//! a sim clock published as a [`SimClock`] resource each
//! [`step`](Simulation::step).
//!
//! Deterministic continuation across saves rides inside the snapshot as the
//! reserved sim-core record ([`SIM_STATE_SCHEMA`](canary_state::SIM_STATE_SCHEMA)):
//! [`capture_with_sim`](SnapshotRegistry::capture_with_sim) appends the
//! [`Simulation`] tick, clock, and RNG position, and
//! [`restore_with_sim`](SnapshotRegistry::restore_with_sim) hands them back,
//! so the next [`step`](Simulation::step) draws the same values it would
//! have without the round-trip. Plain [`capture`](SnapshotRegistry::capture)
//! / [`restore`](SnapshotRegistry::restore) stay entity-and-resource only:
//! restoring a sim-carrying snapshot through plain `restore` is a typed
//! error (it would silently fork the RNG), and `restore_with_sim` on a
//! snapshot without sim state is likewise refused.
//!
//! Restore purity: phase one runs every fallible check that can run without
//! fresh entities (profile agreement, world registration, entity-reference
//! totality, full component decode with placeholder entities, sim-state
//! unpacking) before the first world mutation, so any phase-one failure
//! aborts with zero mutations. Phase two re-decodes against the fresh
//! entities with the same pure functions and a resolve closure that phase
//! one proved total — see the `phase_two_redecode_is_identical_and_failure_free`
//! proof test. A decoder that fails there violates its documented purity
//! contract; the registry cannot stage opaque typed values any earlier
//! without breaking the leaf seam (`canary-state` never sees live types).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use canary_ecs::{CanaryComponent, Entity, World};
use canary_input::SimulationInput;
use canary_state::{
    decode_snapshot, encode_snapshot, snapshot_checksum, OwnedRng, RemapTable, SchemaId,
    SchemaVersion, SimStateSnapshot, Snapshot, SnapshotChecksum, SnapshotEnvelope, SnapshotProfile,
    SnapshotRecord, SnapshotValue, StateError, SIM_STATE_ID, SIM_STATE_SCHEMA,
};

use crate::StagedInsert;

/// Reserved field-map key marking a snapshot entity reference.
///
/// A [`SnapshotValue::Map`] containing this key is an entity reference and
/// must hold exactly `{ "$entity": U64(canonical_id) }` — anything else
/// shaped (extra keys, non-`U64` payload) is malformed and fails restore
/// validation with a typed error. Game components must not use this key for
/// ordinary data; [`encode_entity_ref`] / [`decode_entity_ref`] own it.
const ENTITY_REF_KEY: &str = "$entity";

/// Encodes a live runtime entity as a snapshot entity-reference value.
///
/// Assigns (or reuses) `entity`'s canonical snapshot-local ID through
/// `table` — keyed by the full `(index, generation)` tuple so recycled
/// slots never alias — and wraps it in the reserved
/// [`ENTITY_REF_KEY`] map shape. Call from
/// [`SimComponent::write_snapshot`]; the matching restore side is
/// [`decode_entity_ref`], fed by the restore's canonical-to-fresh map.
pub fn encode_entity_ref(table: &mut RemapTable, entity: Entity) -> SnapshotValue {
    let id = table.assign(entity.index(), entity.generation());
    SnapshotValue::Map(BTreeMap::from([(
        ENTITY_REF_KEY.to_owned(),
        SnapshotValue::U64(u64::from(id)),
    )]))
}

/// Decodes the canonical snapshot-local ID from an entity-reference value.
///
/// Returns `Some(id)` only for exactly `{ "$entity": U64(id) }` with `id`
/// fitting in a `u32`; every other shape (including non-reference values)
/// yields `None` so callers can raise their own typed decode error.
pub fn decode_entity_ref(value: &SnapshotValue) -> Option<u32> {
    let SnapshotValue::Map(entries) = value else {
        return None;
    };
    if entries.len() != 1 {
        return None;
    }
    match entries.get(ENTITY_REF_KEY) {
        Some(SnapshotValue::U64(raw)) => u32::try_from(*raw).ok(),
        _ => None,
    }
}

/// Splits one live entity into its canonical [`RemapTable`] key: the full
/// `(index, generation)` tuple. Tuple keys cannot collide; the previous
/// splitmix hash of the pair could alias two live entities into one
/// canonical ID and merge their records.
fn live_key(entity: Entity) -> (u32, u64) {
    (entity.index(), entity.generation())
}

/// Assigns (or reuses) `entity`'s canonical ID through `table`.
fn assign_live(table: &mut RemapTable, entity: Entity) -> u32 {
    let (index, generation) = live_key(entity);
    table.assign(index, generation)
}

/// Game-owned simulation-component seam: canonical encode plus staged decode.
///
/// `write_snapshot` returns `None` when `entity` carries no such component
/// (so capture can probe every bound component on every simulated entity);
/// otherwise it returns the canonical field map, translating live entity
/// references with [`encode_entity_ref`]. `apply_snapshot` rebuilds the
/// component from canonical fields, resolving snapshot-local IDs through
/// `resolve` — a `None` return means the ID has no live entity, which the
/// restore pre-validates into
/// [`StateError::UnresolvableEntityRef`](canary_state::StateError), so a
/// `None` here is unreachable on the restore path and must still surface a
/// typed error rather than a placeholder.
///
/// Both sides are pure with respect to the [`World`](canary_ecs::World):
/// `write_snapshot` only reads, `apply_snapshot` never touches it — the
/// staged [`StagedInsert`] performs the typed
/// [`World::insert`](canary_ecs::World::insert), never the erased overwrite
/// path (which cannot add a component the entity lacks).
pub trait SimComponent: CanaryComponent + Sized {
    /// Reads `entity`'s component into canonical fields, or `None` when the
    /// entity does not carry this component.
    fn write_snapshot(
        world: &World,
        entity: Entity,
        remap: &mut RemapTable,
    ) -> Option<BTreeMap<String, SnapshotValue>>;

    /// Rebuilds the component from canonical `fields`, resolving entity
    /// references through `resolve`.
    fn apply_snapshot(
        fields: &BTreeMap<String, SnapshotValue>,
        resolve: &dyn Fn(u32) -> Option<Entity>,
    ) -> Result<Self, StateError>;
}

/// Erased decode step behind [`ComponentBinding`]: canonical fields plus an
/// entity-reference resolver into a staged typed insert.
type DecodeFn = fn(
    &BTreeMap<String, SnapshotValue>,
    &dyn Fn(u32) -> Option<Entity>,
) -> Result<StagedInsert, StateError>;

/// One bound simulation component: erased encode/decode function pointers.
///
/// Built with [`ComponentBinding::of`] for a concrete [`SimComponent`];
/// stored in a [`SnapshotRegistry`], which sorts bindings by schema so
/// capture order never depends on caller push order.
pub struct ComponentBinding {
    schema: SchemaId,
    entities: fn(&World) -> Vec<Entity>,
    encode: fn(&World, Entity, &mut RemapTable) -> Option<BTreeMap<String, SnapshotValue>>,
    decode: DecodeFn,
}

impl ComponentBinding {
    /// Binds `T`'s [`SimComponent`] seam into an erased registry entry.
    /// Entity enumeration reads `T`'s query in `(index, generation)` order.
    pub fn of<T: SimComponent>() -> Self {
        Self {
            schema: SchemaId::new(T::SCHEMA_ID),
            entities: |world| {
                let mut found: Vec<Entity> = world.query::<T>().map(|(entity, _)| entity).collect();
                found.sort_by_key(|entity| (entity.index(), entity.generation()));
                found
            },
            encode: T::write_snapshot,
            decode: |fields, resolve| Ok(StagedInsert::stage(T::apply_snapshot(fields, resolve)?)),
        }
    }

    /// The component schema this binding captures and restores.
    pub fn schema(&self) -> &SchemaId {
        &self.schema
    }
}

impl std::fmt::Debug for ComponentBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComponentBinding")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

/// Game-owned deterministic-resource seam: canonical encode plus staged decode.
///
/// `read_snapshot` returns `None` when the resource is absent (so capture
/// skips it) and otherwise returns canonical fields, translating live entity
/// references with [`encode_entity_ref`] exactly like [`SimComponent`].
/// `write_snapshot` rebuilds the resource from canonical `fields`, resolving
/// entity references through `resolve`, and publishes it with a typed
/// `World::insert_resource` — the only world contact, and only in phase two.
/// Like [`SimComponent::apply_snapshot`], it must be pure apart from that
/// final publish: phase one cannot run it without a live world, so phase two
/// re-runs it with a resolve closure phase one proved total (see the module
/// docs and the re-decode proof test).
pub trait SimResource: Send + Sync + 'static {
    /// Schema of the resource snapshot. Must be unique within a registry.
    const SCHEMA_ID: &'static str;

    /// Reads the resource into canonical fields, or `None` when absent.
    fn read_snapshot(
        world: &World,
        remap: &mut RemapTable,
    ) -> Option<BTreeMap<String, SnapshotValue>>;

    /// Rebuilds and publishes the resource from canonical `fields`.
    fn write_snapshot(
        world: &mut World,
        fields: &BTreeMap<String, SnapshotValue>,
        resolve: &dyn Fn(u32) -> Option<Entity>,
    ) -> Result<(), StateError>;
}

/// Erased decode step behind [`ResourceBinding`]: canonical fields plus an
/// entity-reference resolver into a published resource.
type ResourceDecodeFn = fn(
    &mut World,
    &BTreeMap<String, SnapshotValue>,
    &dyn Fn(u32) -> Option<Entity>,
) -> Result<(), StateError>;

/// One bound simulation resource: erased encode/decode function pointers.
///
/// Built with [`ResourceBinding::of`] for a concrete [`SimResource`]; stored
/// in a [`SnapshotRegistry`], which sorts bindings by schema so capture
/// order never depends on caller push order.
pub struct ResourceBinding {
    schema: SchemaId,
    encode: fn(&World, &mut RemapTable) -> Option<BTreeMap<String, SnapshotValue>>,
    decode: ResourceDecodeFn,
}

impl ResourceBinding {
    /// Binds `T`'s [`SimResource`] seam into an erased registry entry.
    pub fn of<T: SimResource>() -> Self {
        Self {
            schema: SchemaId::new(T::SCHEMA_ID),
            encode: T::read_snapshot,
            decode: T::write_snapshot,
        }
    }

    /// The resource schema this binding captures and restores.
    pub fn schema(&self) -> &SchemaId {
        &self.schema
    }
}

impl std::fmt::Debug for ResourceBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResourceBinding")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

/// Reserved snapshot-local ID namespace for resource records: resources sort
/// by binding order into `u32::MAX - 1` downward, so they stay canonical-last
/// after entity records (which use small IDs) and before the sim-core record
/// at [`SIM_STATE_ID`](canary_state::SIM_STATE_ID). Returns a typed error
/// instead of panicking when the binding count cannot fit the range.
fn resource_record_id(index: usize) -> Result<u32, StateError> {
    let offset = u32::try_from(index).map_err(|_| StateError::MigrationInvalid {
        schema: "<resource-record>".to_owned(),
        to: 0,
        reason: format!("resource binding index {index} exceeds u32"),
    })?;
    (SIM_STATE_ID - 1)
        .checked_sub(offset)
        .ok_or_else(|| StateError::MigrationInvalid {
            schema: "<resource-record>".to_owned(),
            to: 0,
            reason: format!("resource binding index {index} exhausts the reserved ID range"),
        })
}

/// The caller-owned simulation boundary: which components and resources participate.
///
/// Bindings sort by schema at construction — capture and restore iterate
/// that order explicitly and never rely on caller push order or any map
/// iteration order. Components and resources without a binding are
/// presentation by definition here: never captured, never cleared, never
/// checksummed. The reserved sim-core schema
/// ([`SIM_STATE_SCHEMA`](canary_state::SIM_STATE_SCHEMA)) is always declared
/// in the profile so checksums accept it; only the `_with_sim` capture and
/// restore paths emit or consume it.
#[derive(Debug)]
pub struct SnapshotRegistry {
    bindings: Vec<ComponentBinding>,
    resources: Vec<ResourceBinding>,
    schema: SchemaId,
    version: SchemaVersion,
}

impl SnapshotRegistry {
    /// Declares the boundary: the snapshot envelope identity plus one
    /// binding per simulated component and one per simulated resource.
    /// Both binding lists sort by schema string.
    pub fn new(
        schema: SchemaId,
        version: SchemaVersion,
        mut bindings: Vec<ComponentBinding>,
        mut resources: Vec<ResourceBinding>,
    ) -> Self {
        bindings.sort_by(|left, right| left.schema.as_str().cmp(right.schema.as_str()));
        resources.sort_by(|left, right| left.schema.as_str().cmp(right.schema.as_str()));
        Self {
            bindings,
            resources,
            schema,
            version,
        }
    }

    /// The declared profile: envelope identity plus bound component schemas,
    /// bound resource schemas, and the reserved sim-core schema. Component
    /// and resource order is irrelevant downstream; neither is encoded.
    pub fn profile(&self) -> SnapshotProfile {
        let mut components: Vec<SchemaId> = self
            .bindings
            .iter()
            .map(|binding| binding.schema.clone())
            .chain(self.resources.iter().map(|binding| binding.schema.clone()))
            .collect();
        components.push(SchemaId::new(SIM_STATE_SCHEMA));
        SnapshotProfile::new(self.schema.clone(), self.version, components)
    }

    /// Finds the binding for `schema`, if the registry declares it.
    fn binding(&self, schema: &SchemaId) -> Option<&ComponentBinding> {
        self.bindings.iter().find(|entry| entry.schema == *schema)
    }

    /// Finds the resource binding for `schema`, if the registry declares it.
    fn resource_binding(&self, schema: &SchemaId) -> Option<&ResourceBinding> {
        self.resources.iter().find(|entry| entry.schema == *schema)
    }

    /// Whether `record` is the reserved sim-core record (exact reserved ID
    /// and schema — a lookalike with the right schema but a small ID is an
    /// entity record naming an undeclared component, not sim state).
    fn is_sim_record(record: &SnapshotRecord) -> bool {
        record.id == SIM_STATE_ID && record.component.as_str() == SIM_STATE_SCHEMA
    }

    /// Every entity carrying at least one bound component, in
    /// `(index, generation)` order with no duplicates. Entities with no
    /// bound component (presentation-only) never appear here.
    fn sim_entities(&self, world: &World) -> Vec<Entity> {
        let mut entities = Vec::new();
        for binding in &self.bindings {
            entities.extend((binding.entities)(world));
        }
        entities.sort_by_key(|entity| (entity.index(), entity.generation()));
        entities.dedup();
        entities
    }

    /// Captures the declared simulation state: bound components and bound
    /// resources only, in canonical record order, with snapshot-local IDs
    /// assigned in sorted entity order. Read-only against the world;
    /// presentation state never enters the records. Unbound components and
    /// resources are skipped silently, values outside the profile are
    /// impossible by construction, and non-finite floats fail with a typed
    /// error at the [`encode_snapshot`](canary_state::encode_snapshot) gate.
    /// Carries no sim-core state — see
    /// [`capture_with_sim`](SnapshotRegistry::capture_with_sim) for the
    /// deterministic-continuation path.
    pub fn capture(&self, world: &World) -> Result<Snapshot, StateError> {
        self.capture_inner(world, None)
    }

    /// Captures the declared simulation state plus the reserved sim-core
    /// record (tick, clock, RNG position from `sim`; the last step and
    /// frame come from the world's [`SimClock`] resource when present, else
    /// zero). The sim record sorts last by its reserved ID and verifies
    /// under the same checksum. Restoring needs
    /// [`restore_with_sim`](SnapshotRegistry::restore_with_sim).
    pub fn capture_with_sim(
        &self,
        world: &World,
        sim: &Simulation,
    ) -> Result<Snapshot, StateError> {
        self.capture_inner(world, Some(sim.capture_record(world)))
    }

    /// Capture shared by [`capture`](SnapshotRegistry::capture) (no sim
    /// record) and [`capture_with_sim`](SnapshotRegistry::capture_with_sim).
    fn capture_inner(
        &self,
        world: &World,
        sim_record: Option<SnapshotRecord>,
    ) -> Result<Snapshot, StateError> {
        // Duplicate component or resource schemas would emit two records
        // for one schema, which restore refuses — fail fast instead of
        // capturing an unrestorable snapshot.
        {
            let mut seen = BTreeSet::new();
            for binding in &self.bindings {
                if !seen.insert(binding.schema.as_str()) {
                    return Err(StateError::MigrationInvalid {
                        schema: binding.schema.as_str().to_owned(),
                        to: 0,
                        reason: "duplicate component schema in registry".to_owned(),
                    });
                }
            }
        }
        {
            let mut seen = BTreeSet::new();
            for binding in &self.resources {
                if !seen.insert(binding.schema.as_str()) {
                    return Err(StateError::MigrationInvalid {
                        schema: binding.schema.as_str().to_owned(),
                        to: 0,
                        reason: "duplicate resource schema in registry".to_owned(),
                    });
                }
            }
        }
        let entities = self.sim_entities(world);
        // Pre-assign canonical IDs in sorted entity order so the IDs — and
        // hence the bytes — never depend on binding order or spawn history.
        let mut remap = RemapTable::default();
        for entity in &entities {
            assign_live(&mut remap, *entity);
        }
        let mut records = Vec::new();
        for entity in &entities {
            for binding in &self.bindings {
                if let Some(fields) = (binding.encode)(world, *entity, &mut remap) {
                    records.push(SnapshotRecord {
                        id: assign_live(&mut remap, *entity),
                        component: binding.schema.clone(),
                        fields,
                    });
                }
            }
        }
        // Resources follow entities in binding-schema order with reserved
        // IDs from the top of the u32 range downward. An absent resource
        // contributes no record; an unbound one is presentation, skipped.
        for (index, binding) in self.resources.iter().enumerate() {
            if let Some(fields) = (binding.encode)(world, &mut remap) {
                records.push(SnapshotRecord {
                    id: resource_record_id(index)?,
                    component: binding.schema.clone(),
                    fields,
                });
            }
        }
        if let Some(record) = sim_record {
            records.push(record);
        }
        // Round-trip through the canonical bytes so the returned snapshot is
        // byte-canonical (sorted records, verified checksum), not just
        // logically complete. `decode_snapshot` returns the canonical body
        // with its checksum field cleared (it just verified it), so re-stamp
        // the envelope the encode step pinned.
        let profile = self.profile();
        let (bytes, sum) = encode_snapshot(&profile, records)?;
        let mut snapshot = decode_snapshot(&bytes)?;
        snapshot.envelope.checksum = sum.0;
        Ok(snapshot)
    }

    /// Captures and encodes: the canonical postcard bytes plus their
    /// checksum. Byte-identical across runs for identical declared state.
    pub fn capture_bytes(&self, world: &World) -> Result<(Vec<u8>, SnapshotChecksum), StateError> {
        let snapshot = self.capture(world)?;
        encode_snapshot(&self.profile(), snapshot.records)
    }

    /// Captures with sim-core state and encodes: the canonical postcard
    /// bytes plus their checksum. Byte-identical across runs for identical
    /// declared state advanced to the same tick with the same RNG position.
    pub fn capture_bytes_with_sim(
        &self,
        world: &World,
        sim: &Simulation,
    ) -> Result<(Vec<u8>, SnapshotChecksum), StateError> {
        let snapshot = self.capture_with_sim(world, sim)?;
        encode_snapshot(&self.profile(), snapshot.records)
    }

    /// Recomputes the canonical checksum of `snapshot` under the declared
    /// profile. Records naming an undeclared component fail with
    /// [`StateError::UndeclaredComponent`](canary_state::StateError);
    /// otherwise the digest matches the envelope checksum of any snapshot
    /// this registry captured.
    pub fn checksum(&self, snapshot: &Snapshot) -> Result<SnapshotChecksum, StateError> {
        let profile = self.profile();
        for record in &snapshot.records {
            if !profile.declares(&record.component) {
                return Err(StateError::UndeclaredComponent(
                    record.component.as_str().to_owned(),
                ));
            }
        }
        snapshot_checksum(snapshot)
    }

    /// Restores entity and resource state in two phases, mirroring
    /// [`AuthoredSpawner`](super::AuthoredSpawner): phase one validates
    /// everything (every record declared in the profile, every schema
    /// registered in the world, every entity reference pointing at a
    /// snapshot-local ID that has an entity record, every component field
    /// set decodable, the sim-state unpacking when present) before the first
    /// world mutation; phase two despawns the currently simulated entities,
    /// spawns one fresh entity per snapshot-local ID, rewrites references
    /// through that map, decodes every component into a staged buffer before
    /// the first typed insert, then runs the staged inserts and the resource
    /// publishes. Any phase-one failure — unknown schema, decode failure,
    /// dangling reference — aborts with zero world mutations.
    /// Presentation-only entities and unbound resources are left untouched.
    ///
    /// A snapshot carrying the reserved sim-core record is refused here
    /// with a typed error: applying entity state while dropping the RNG
    /// position would silently fork determinism. Use
    /// [`restore_with_sim`](SnapshotRegistry::restore_with_sim).
    ///
    /// The caller owes checksum verification: pass only snapshots returned
    /// by [`decode_snapshot`] (or the `restore_bytes` path, which enforces
    /// it). The struct form carries no verifiable checksum on its own, so a
    /// hand-built [`Snapshot`] restores unchecked — exactly what the
    /// migration-fixture test relies on, and exactly why untrusted bytes
    /// must never be hand-decoded into a `Snapshot` and restored.
    pub fn restore(
        &self,
        world: &mut World,
        snapshot: &Snapshot,
    ) -> Result<RestoreReport, StateError> {
        if snapshot.records.iter().any(Self::is_sim_record) {
            return Err(StateError::MigrationInvalid {
                schema: SIM_STATE_SCHEMA.to_owned(),
                to: 0,
                reason: "snapshot carries sim-core state: use restore_with_sim so the RNG and clock continue deterministically".to_owned(),
            });
        }
        self.restore_inner(world, &snapshot.envelope, &snapshot.records)
    }

    /// Restores entity and resource state plus deterministic continuation:
    /// exactly one reserved sim-core record must be present (unpacking it is
    /// a phase-one check, so a corrupt record aborts with zero mutations),
    /// and after the entity/resource applies the [`Simulation`] tick, clock,
    /// and RNG position take the snapshotted values while the [`SimClock`]
    /// resource is republished for the next step. The step after this
    /// restore draws the same RNG value the uninterrupted run would have.
    ///
    /// The caller owes checksum verification, exactly as
    /// [`restore`](SnapshotRegistry::restore) documents: pass only
    /// [`decode_snapshot`]-verified snapshots (or `restore_bytes_with_sim`,
    /// which enforces it).
    pub fn restore_with_sim(
        &self,
        world: &mut World,
        snapshot: &Snapshot,
        sim: &mut Simulation,
    ) -> Result<RestoreReport, StateError> {
        let mut sim_records = snapshot
            .records
            .iter()
            .filter(|record| Self::is_sim_record(record));
        let sim_record = sim_records.next().ok_or_else(|| StateError::MigrationInvalid {
            schema: SIM_STATE_SCHEMA.to_owned(),
            to: 0,
            reason: "restore_with_sim needs exactly one sim-core record: capture with capture_with_sim".to_owned(),
        })?;
        if sim_records.next().is_some() {
            return Err(StateError::MigrationInvalid {
                schema: SIM_STATE_SCHEMA.to_owned(),
                to: 0,
                reason: "duplicate sim-core records: at most one per snapshot".to_owned(),
            });
        }
        // Unpack before the first mutation: a corrupt sim record aborts
        // with the world untouched.
        let state = SimStateSnapshot::from_record(sim_record)?;
        let rest: Vec<SnapshotRecord> = snapshot
            .records
            .iter()
            .filter(|record| !Self::is_sim_record(record))
            .cloned()
            .collect();
        let report = self.restore_inner(world, &snapshot.envelope, &rest)?;
        // Infallible by construction: plain field copies plus a typed
        // resource publish, which cannot fail.
        sim.apply_sim_state(world, &state);
        Ok(report)
    }

    /// Phase one reads only — no spawns, despawns, or inserts until every
    /// check below has passed. Phase two mutates, but decodes every
    /// component into a staged buffer before the first typed insert runs;
    /// the re-decode uses the same pure functions with a resolve closure
    /// phase one proved total (see the module docs).
    ///
    /// The envelope check below is the first phase-one gate: the snapshot's
    /// schema and version must agree exactly with this registry's. A payload
    /// from another simulation (or another version, which has no migration
    /// path on this boundary) aborts with zero mutations before any record
    /// is inspected.
    fn restore_inner(
        &self,
        world: &mut World,
        envelope: &SnapshotEnvelope,
        records: &[SnapshotRecord],
    ) -> Result<RestoreReport, StateError> {
        if envelope.schema != self.schema {
            return Err(StateError::MigrationInvalid {
                schema: envelope.schema.as_str().to_owned(),
                to: self.version.0,
                reason: format!(
                    "snapshot envelope schema '{}' does not match registry schema '{}'",
                    envelope.schema.as_str(),
                    self.schema.as_str()
                ),
            });
        }
        if envelope.version != self.version {
            return Err(StateError::MigrationInvalid {
                schema: envelope.schema.as_str().to_owned(),
                to: self.version.0,
                reason: format!(
                    "snapshot envelope version {} does not match registry version {}",
                    envelope.version.0, self.version.0
                ),
            });
        }
        // Partition resource records (declared resource schemas) from entity
        // records. Anything else must name a bound component schema.
        let mut entity_records: Vec<&SnapshotRecord> = Vec::new();
        let mut resource_records: Vec<&SnapshotRecord> = Vec::new();
        for record in records {
            if self.resource_binding(&record.component).is_some() {
                resource_records.push(record);
            } else {
                entity_records.push(record);
            }
        }
        for record in &entity_records {
            if self.binding(&record.component).is_none() {
                return Err(StateError::UndeclaredComponent(
                    record.component.as_str().to_owned(),
                ));
            }
            if world
                .type_id_for_schema(record.component.as_str())
                .is_none()
            {
                return Err(StateError::UnknownSchema(
                    record.component.as_str().to_owned(),
                ));
            }
        }
        // At most one record per resource schema: duplicates would publish
        // twice with the last write winning silently. At most one record
        // per `(id, schema)` entity pair for the same reason: one component
        // instance lives on one entity, so a doubled pair is corruption, not
        // data.
        let mut seen_resources = BTreeSet::new();
        for record in &resource_records {
            if !seen_resources.insert(record.component.as_str()) {
                return Err(StateError::MigrationInvalid {
                    schema: record.component.as_str().to_owned(),
                    to: 0,
                    reason: "duplicate resource record: at most one per schema".to_owned(),
                });
            }
        }
        let mut seen_pairs = BTreeSet::new();
        for record in &entity_records {
            if !seen_pairs.insert((record.id, record.component.as_str())) {
                return Err(StateError::MigrationInvalid {
                    schema: record.component.as_str().to_owned(),
                    to: 0,
                    reason: format!(
                        "duplicate entity record for snapshot-local id {}: at most one per schema",
                        record.id
                    ),
                });
            }
        }
        // Entity-reference totality is checked against entity IDs only:
        // references to resource or sim-core IDs fail here, before any
        // mutation, instead of dangling in phase two.
        let mut ids = BTreeSet::new();
        for record in &entity_records {
            ids.insert(record.id);
        }
        for record in entity_records
            .iter()
            .copied()
            .chain(resource_records.iter().copied())
        {
            for fields in record.fields.values() {
                check_value_refs(fields, &ids, &record.component)?;
            }
        }
        // Decode validation with placeholder entities: proves every field
        // set decodes while entity references are still canonical IDs. The
        // results are discarded — phase two re-decodes against the fresh
        // entities with the same pure functions (see the module docs and
        // the re-decode proof test for why that call cannot fail).
        for record in &entity_records {
            let binding = self.binding(&record.component).ok_or_else(|| {
                StateError::UndeclaredComponent(record.component.as_str().to_owned())
            })?;
            (binding.decode)(&record.fields, &|id| {
                ids.contains(&id).then(|| Entity::from_raw_parts(id, 0))
            })?;
        }
        // Resource decode validation on a scratch world: resource publishes
        // run directly against the world in phase two with no staged buffer,
        // so a bad resource field set would otherwise abort after the
        // entities were already despawned and respawned. The scratch run
        // proves every resource field set publishes cleanly while entity
        // references are still canonical IDs; phase two re-runs the same
        // pure functions against the fresh map. `SimResource::write_snapshot`
        // is pure apart from its final publish, so the double call is safe.
        // Validation runs in phase-two publish order (schema order, as the
        // loop below does): the scratch world must see the same predecessor
        // resources phase two will have published, or a cross-resource read
        // could validate against a different world than it publishes into.
        {
            let mut scratch = World::new();
            let mut ordered_validation: Vec<&&SnapshotRecord> = resource_records.iter().collect();
            ordered_validation
                .sort_by(|left, right| left.component.as_str().cmp(right.component.as_str()));
            for record in ordered_validation {
                let binding = self.resource_binding(&record.component).ok_or_else(|| {
                    StateError::UndeclaredComponent(record.component.as_str().to_owned())
                })?;
                (binding.decode)(&mut scratch, &record.fields, &|id| {
                    ids.contains(&id).then(|| Entity::from_raw_parts(id, 0))
                })?;
            }
        }

        // Phase two: mutate. First collect, then despawn in reverse sorted
        // order, so no live query runs across a despawn. Reversal matters:
        // `World` recycles slots through a LIFO free stack (`pop` on spawn,
        // `push` on despawn), so despawning ascending would hand the slots
        // back descending and the fresh spawns below would land inverted —
        // a restore would still be logically identical but a recapture
        // would assign flipped canonical IDs. Despawning descending makes
        // the pops come out ascending, so spawning in canonical ID order
        // lands each fresh entity on an ascending slot and the live-sort
        // order matches ID order: capture→restore→recapture is
        // byte-stable. (When the snapshot holds more entities than the
        // world, the surplus spawns reuse older free slots or fresh ones;
        // the restore stays correct through the live map, but recapture
        // IDs then reflect allocator history rather than the snapshot's.)
        let doomed = self.sim_entities(world);
        for entity in doomed.into_iter().rev() {
            world
                .despawn(entity)
                .map_err(|error| StateError::PlacementFailed {
                    entity: format!("{entity:?}"),
                    reason: error.to_string(),
                })?;
        }
        let mut live = BTreeMap::new();
        for id in &ids {
            live.insert(*id, world.spawn());
        }
        // Deterministic decode order: canonical ID, then schema string. Every
        // record decodes into the staged buffer before the first typed
        // insert runs, so a (contract-violating) decode failure here still
        // precedes component placement.
        let mut ordered: Vec<&&SnapshotRecord> = entity_records.iter().collect();
        ordered.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.component.as_str().cmp(right.component.as_str()))
        });
        let mut staged: Vec<(Entity, StagedInsert)> = Vec::with_capacity(ordered.len());
        for record in ordered {
            let binding = self.binding(&record.component).ok_or_else(|| {
                StateError::UndeclaredComponent(record.component.as_str().to_owned())
            })?;
            let entity = live
                .get(&record.id)
                .copied()
                .ok_or(StateError::UnresolvableEntityRef { id: record.id })?;
            // Re-decodes deterministically: phase one already proved this
            // exact call (modulo placeholder-versus-fresh entities, which
            // pure decoders only store) succeeds.
            let insert = (binding.decode)(&record.fields, &|id| live.get(&id).copied())?;
            staged.push((entity, insert));
        }
        for (entity, insert) in staged {
            insert.insert(world, entity)?;
        }
        // Resources publish last in schema order so entity references they
        // carry resolve through the same fresh map.
        let mut ordered_resources = resource_records;
        ordered_resources
            .sort_by(|left, right| left.component.as_str().cmp(right.component.as_str()));
        for record in ordered_resources {
            let binding = self.resource_binding(&record.component).ok_or_else(|| {
                StateError::UndeclaredComponent(record.component.as_str().to_owned())
            })?;
            (binding.decode)(world, &record.fields, &|id| live.get(&id).copied())?;
        }
        Ok(RestoreReport { entities: live })
    }

    /// Decodes (checksum-verified) and restores: the bytes path for
    /// save-file loads. A corrupt or non-canonical payload fails at the
    /// [`decode_snapshot`] gate before validation — and hence before any
    /// world mutation — begins.
    pub fn restore_bytes(
        &self,
        world: &mut World,
        bytes: &[u8],
    ) -> Result<RestoreReport, StateError> {
        let snapshot = decode_snapshot(bytes)?;
        self.restore(world, &snapshot)
    }

    /// Decodes (checksum-verified) and restores with deterministic
    /// continuation: the bytes path for save-file loads of snapshots
    /// written by [`capture_bytes_with_sim`](SnapshotRegistry::capture_bytes_with_sim).
    pub fn restore_bytes_with_sim(
        &self,
        world: &mut World,
        bytes: &[u8],
        sim: &mut Simulation,
    ) -> Result<RestoreReport, StateError> {
        let snapshot = decode_snapshot(bytes)?;
        self.restore_with_sim(world, &snapshot, sim)
    }
}

/// Where one restore landed: fresh runtime entity per snapshot-local ID.
#[derive(Debug, Default, Clone)]
pub struct RestoreReport {
    /// Snapshot-local ID to the fresh runtime entity spawned for it.
    pub entities: BTreeMap<u32, Entity>,
}

impl RestoreReport {
    /// Looks up the fresh entity spawned for snapshot-local `id`.
    #[must_use]
    pub fn entity(&self, id: u32) -> Option<Entity> {
        self.entities.get(&id).copied()
    }
}

/// Recursively validates entity references inside one field value: any map
/// holding the reserved [`ENTITY_REF_KEY`] must be exactly
/// `{ "$entity": U64(id) }` with `id` present in the snapshot's record set.
/// Ordinary maps and lists recurse; every other value holds no references.
fn check_value_refs(
    value: &SnapshotValue,
    ids: &BTreeSet<u32>,
    schema: &SchemaId,
) -> Result<(), StateError> {
    if let SnapshotValue::Map(entries) = value {
        if entries.contains_key(ENTITY_REF_KEY) {
            let well_formed = entries.len() == 1
                && matches!(entries.get(ENTITY_REF_KEY), Some(SnapshotValue::U64(_)));
            if !well_formed {
                return Err(StateError::MigrationInvalid {
                    schema: schema.as_str().to_owned(),
                    to: 0,
                    reason: format!(
                        "malformed snapshot entity reference: '{ENTITY_REF_KEY}' maps must hold exactly one U64 id"
                    ),
                });
            }
            if let Some(SnapshotValue::U64(raw)) = entries.get(ENTITY_REF_KEY) {
                let id = u32::try_from(*raw).map_err(|_| StateError::MigrationInvalid {
                    schema: schema.as_str().to_owned(),
                    to: 0,
                    reason: format!("snapshot entity id {raw} exceeds u32"),
                })?;
                if !ids.contains(&id) {
                    return Err(StateError::UnresolvableEntityRef { id });
                }
            }
            return Ok(());
        }
        for nested in entries.values() {
            check_value_refs(nested, ids, schema)?;
        }
        return Ok(());
    }
    if let SnapshotValue::List(items) = value {
        for item in items {
            check_value_refs(item, ids, schema)?;
        }
    }
    Ok(())
}

/// The simulation clock as the world sees it: published by
/// [`Simulation::step`] as a per-step-overwritten resource, read by systems.
///
/// A resource write stamps no component change ticks, so publishing the
/// clock never trips component change detection — the same reason
/// [`RunContext`](super::RunContext) and
/// [`SimulationInput::publish`](canary_input::SimulationInput::publish) use
/// resources rather than component data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimClock {
    /// Completed simulation steps.
    pub tick: u64,
    /// Accumulated simulation time.
    pub sim_time: Duration,
    /// The `dt` that advanced the most recent step.
    pub step: Duration,
    /// The input frame that drove the most recent step.
    pub frame_index: u64,
}

/// What one [`Simulation::step`] advanced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepReport {
    /// Completed simulation steps after this step.
    pub tick: u64,
    /// Accumulated simulation time after this step.
    pub sim_time: Duration,
    /// The RNG draw for this step, from the owned stream.
    pub rng_value: u64,
}

/// Owned deterministic simulation state: RNG stream plus sim clock.
///
/// `step` advances the world's ECS tick exactly once, folds `dt` into the
/// sim clock, draws one RNG value from the seed-owned splitmix64 stream, and
/// publishes the [`SimClock`] resource. Same seed plus same
/// input-and-`dt` sequence reproduces the same reports forever; no OS
/// randomness is ever consulted.
///
/// Authored change tracking is untouched by construction: only
/// [`AuthoredDocument::record_change`](canary_state::AuthoredDocument::record_change)
/// appends authored changes, and `step` never calls it — ticks without
/// edits record nothing, edits record without ticks (see tests).
#[derive(Debug)]
pub struct Simulation {
    rng: OwnedRng,
    tick: u64,
    sim_time: Duration,
}

impl Simulation {
    /// Starts deterministic state from `seed`. Any `u64` is a valid seed,
    /// including zero; equal seeds agree on every future [`step`](Self::step).
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            rng: OwnedRng::from_seed(seed),
            tick: 0,
            sim_time: Duration::ZERO,
        }
    }

    /// Advances one deterministic simulation pass for `input`: ECS tick,
    /// sim clock, one RNG draw, [`SimClock`] publication, in that order.
    /// `input`'s frame identity travels into the published clock so systems
    /// can correlate the pass with the input frame that drove it.
    pub fn step(&mut self, world: &mut World, input: &SimulationInput, dt: Duration) -> StepReport {
        world.advance_tick();
        self.tick = self.tick.saturating_add(1);
        self.sim_time = self.sim_time.checked_add(dt).unwrap_or(Duration::MAX);
        let rng_value = self.rng.next_u64();
        world.insert_resource(SimClock {
            tick: self.tick,
            sim_time: self.sim_time,
            step: dt,
            frame_index: input.frame_index,
        });
        StepReport {
            tick: self.tick,
            sim_time: self.sim_time,
            rng_value,
        }
    }

    /// Completed simulation steps.
    #[must_use]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Accumulated simulation time.
    #[must_use]
    pub fn sim_time(&self) -> Duration {
        self.sim_time
    }

    /// Current owned-RNG stream position, for snapshots. Feeding it back
    /// through [`OwnedRng::from_state`] (via
    /// [`apply_sim_state`](Simulation::apply_sim_state)) continues the same
    /// deterministic sequence.
    #[must_use]
    pub fn rng_state(&self) -> u64 {
        self.rng.state()
    }

    /// Packs the reserved sim-core record for
    /// [`capture_with_sim`](SnapshotRegistry::capture_with_sim): this
    /// simulation's tick, clock, and RNG position, plus the last step and
    /// frame from the world's [`SimClock`] resource when present (zero when
    /// no step has published one yet). Read-only against both inputs.
    #[must_use]
    pub fn capture_record(&self, world: &World) -> SnapshotRecord {
        let (step, frame_index) = world
            .resource::<SimClock>()
            .map(|clock| (clock.step, clock.frame_index))
            .unwrap_or_default();
        SimStateSnapshot::to_record(
            self.tick,
            self.sim_time,
            step,
            frame_index,
            self.rng.state(),
        )
    }

    /// Applies validated sim-core state after a restore: the tick, clock,
    /// and RNG position take the snapshotted values, and the [`SimClock`]
    /// resource is republished so systems observe the restored clock before
    /// the next step. Infallible by construction — plain field copies plus
    /// one typed resource publish — so calling it after the entity/resource
    /// applies cannot introduce a new failure mode.
    pub fn apply_sim_state(&mut self, world: &mut World, state: &SimStateSnapshot) {
        self.tick = state.tick;
        self.sim_time = state.sim_time();
        self.rng = OwnedRng::from_state(state.rng_state);
        world.insert_resource(SimClock {
            tick: state.tick,
            sim_time: state.sim_time(),
            step: state.step(),
            frame_index: state.frame_index,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Health {
        hp: i64,
    }

    impl CanaryComponent for Health {
        const SCHEMA_ID: &'static str = "test.health";
    }

    impl SimComponent for Health {
        fn write_snapshot(
            world: &World,
            entity: Entity,
            _remap: &mut RemapTable,
        ) -> Option<BTreeMap<String, SnapshotValue>> {
            world
                .get::<Health>(entity)
                .map(|health| BTreeMap::from([("hp".to_owned(), SnapshotValue::I64(health.hp))]))
        }

        fn apply_snapshot(
            fields: &BTreeMap<String, SnapshotValue>,
            _resolve: &dyn Fn(u32) -> Option<Entity>,
        ) -> Result<Self, StateError> {
            match fields.get("hp") {
                Some(SnapshotValue::I64(hp)) => Ok(Self { hp: *hp }),
                other => Err(StateError::MigrationInvalid {
                    schema: Self::SCHEMA_ID.to_owned(),
                    to: 0,
                    reason: format!("test.health wants I64 hp, got {other:?}"),
                }),
            }
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Follows {
        target: Entity,
    }

    impl CanaryComponent for Follows {
        const SCHEMA_ID: &'static str = "test.follows";
    }

    impl SimComponent for Follows {
        fn write_snapshot(
            world: &World,
            entity: Entity,
            remap: &mut RemapTable,
        ) -> Option<BTreeMap<String, SnapshotValue>> {
            world.get::<Follows>(entity).map(|follows| {
                BTreeMap::from([(
                    "target".to_owned(),
                    encode_entity_ref(remap, follows.target),
                )])
            })
        }

        fn apply_snapshot(
            fields: &BTreeMap<String, SnapshotValue>,
            resolve: &dyn Fn(u32) -> Option<Entity>,
        ) -> Result<Self, StateError> {
            let invalid = |reason: String| StateError::MigrationInvalid {
                schema: Self::SCHEMA_ID.to_owned(),
                to: 0,
                reason,
            };
            match fields.get("target") {
                Some(value) => match decode_entity_ref(value) {
                    Some(id) => match resolve(id) {
                        Some(target) => Ok(Self { target }),
                        None => Err(StateError::UnresolvableEntityRef { id }),
                    },
                    None => Err(invalid(format!(
                        "test.follows wants an entity ref, got {value:?}"
                    ))),
                },
                None => Err(invalid("test.follows is missing its target".to_owned())),
            }
        }
    }

    /// Presentation-only: registered in the world's schema registry but
    /// deliberately bound to no registry, so capture must never see it.
    #[derive(Debug, Clone, PartialEq)]
    struct RenderHandle {
        label: String,
    }

    impl CanaryComponent for RenderHandle {
        const SCHEMA_ID: &'static str = "test.render-handle";
    }

    /// Presentation-only resource: UI state, never simulated.
    #[derive(Debug, Clone, PartialEq)]
    struct UiSelection {
        focused: String,
    }

    /// Presentation-only resource: renderer cache internals, never simulated.
    #[derive(Debug, Clone, PartialEq)]
    struct RenderCache {
        frames: u64,
    }

    fn registry() -> SnapshotRegistry {
        SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![
                ComponentBinding::of::<Health>(),
                ComponentBinding::of::<Follows>(),
            ],
            Vec::new(),
        )
    }

    fn register_all(world: &mut World) {
        world
            .register_component::<Health>()
            .expect("register health");
        world
            .register_component::<Follows>()
            .expect("register follows");
        world
            .register_component::<RenderHandle>()
            .expect("register render handle");
    }

    /// Hero (health + presentation handle), sidekick (health + follows hero),
    /// plus presentation-only resources. Returns the world and both sim
    /// entities.
    fn populated_world() -> (World, Entity, Entity) {
        let mut world = World::new();
        register_all(&mut world);
        let hero = world.spawn();
        world
            .insert(hero, Health { hp: 10 })
            .expect("insert hero health");
        world
            .insert(
                hero,
                RenderHandle {
                    label: "PRESENTATION-SENTINEL-hero-mesh".to_owned(),
                },
            )
            .expect("insert hero handle");
        let sidekick = world.spawn();
        world
            .insert(sidekick, Health { hp: 20 })
            .expect("insert sidekick health");
        world
            .insert(sidekick, Follows { target: hero })
            .expect("insert sidekick follows");
        world.insert_resource(UiSelection {
            focused: "PRESENTATION-SENTINEL-focus".to_owned(),
        });
        world.insert_resource(RenderCache { frames: 7 });
        (world, hero, sidekick)
    }

    fn test_input(frame_index: u64) -> SimulationInput {
        SimulationInput {
            player: canary_input::PlayerSlot::LOCAL,
            frame_index,
            tick: None,
            actions: Vec::new(),
        }
    }

    fn health_of(world: &World, entity: Entity) -> i64 {
        world.get::<Health>(entity).expect("health present").hp
    }

    fn snapshot_with_extra_record(base: &Snapshot, record: SnapshotRecord) -> Snapshot {
        let mut tampered = base.clone();
        tampered.records.push(record);
        tampered
    }

    #[test]
    fn capture_holds_only_declared_components_in_canonical_order() {
        let (world, _, _) = populated_world();
        let snapshot = registry().capture(&world).expect("capture");

        assert_eq!(snapshot.envelope.schema.as_str(), "test.sim");
        for record in &snapshot.records {
            assert!(
                record.component.as_str() == Health::SCHEMA_ID
                    || record.component.as_str() == Follows::SCHEMA_ID,
                "undeclared component captured: {}",
                record.component.as_str()
            );
        }
        // Hero (index 0) sorts first: one health record, no render handle.
        // Sidekick (index 1): follows then health, schema-sorted.
        let order: Vec<(u32, &str)> = snapshot
            .records
            .iter()
            .map(|record| (record.id, record.component.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![
                (0, Health::SCHEMA_ID),
                (1, Follows::SCHEMA_ID),
                (1, Health::SCHEMA_ID),
            ]
        );
        assert!(snapshot.records.windows(2).all(|pair| {
            (pair[0].id, pair[0].component.as_str()) <= (pair[1].id, pair[1].component.as_str())
        }));
    }

    #[test]
    fn capture_bytes_hide_all_presentation_state() {
        let (world, _, _) = populated_world();
        let count_before = world.entity_count();
        let (bytes, _) = registry().capture_bytes(&world).expect("capture bytes");

        for sentinel in [
            b"PRESENTATION-SENTINEL-hero-mesh".as_slice(),
            b"PRESENTATION-SENTINEL-focus".as_slice(),
            b"test.render-handle".as_slice(),
        ] {
            assert!(
                !bytes
                    .windows(sentinel.len())
                    .any(|window| window == sentinel),
                "presentation state leaked into snapshot bytes"
            );
        }
        // Capture is read-only: entities and resources are untouched.
        assert_eq!(world.entity_count(), count_before);
        assert_eq!(world.resource::<RenderCache>().expect("cache").frames, 7);
    }

    #[test]
    fn reordered_bindings_capture_byte_identical_snapshots() {
        let (world, _, _) = populated_world();
        let ordered = SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![
                ComponentBinding::of::<Health>(),
                ComponentBinding::of::<Follows>(),
            ],
            Vec::new(),
        );
        let reversed = SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![
                ComponentBinding::of::<Follows>(),
                ComponentBinding::of::<Health>(),
            ],
            Vec::new(),
        );
        let (first_bytes, first_sum) = ordered.capture_bytes(&world).expect("capture");
        let (second_bytes, second_sum) = reversed.capture_bytes(&world).expect("capture");
        assert_eq!(first_bytes, second_bytes);
        assert_eq!(first_sum, second_sum);
    }

    #[test]
    fn recycled_entity_slots_capture_identically() {
        let mut plain = World::new();
        register_all(&mut plain);
        let plain_first = plain.spawn();
        plain.insert(plain_first, Health { hp: 1 }).expect("insert");
        let plain_second = plain.spawn();
        plain
            .insert(plain_second, Health { hp: 2 })
            .expect("insert");

        let mut recycled = World::new();
        register_all(&mut recycled);
        let scratch = recycled.spawn();
        recycled.despawn(scratch).expect("despawn scratch");
        let recycled_first = recycled.spawn();
        recycled
            .insert(recycled_first, Health { hp: 1 })
            .expect("insert");
        let recycled_second = recycled.spawn();
        recycled
            .insert(recycled_second, Health { hp: 2 })
            .expect("insert");

        // The recycled world really reused the scratch slot with a bumped
        // generation: without generation keying this comparison would pass
        // vacuously on index equality alone.
        assert_eq!(
            recycled_first.index(),
            scratch.index(),
            "test needs slot reuse"
        );
        assert_ne!(
            recycled_first.generation(),
            scratch.generation(),
            "test needs a bumped generation after reuse"
        );
        assert_ne!(
            recycled_first.generation(),
            plain_first.generation(),
            "allocator histories must differ for the comparison to mean anything"
        );

        let (plain_bytes, _) = registry().capture_bytes(&plain).expect("capture");
        let (recycled_bytes, _) = registry().capture_bytes(&recycled).expect("capture");
        assert_eq!(plain_bytes, recycled_bytes);
    }

    #[test]
    fn checksum_recomputes_the_envelope_digest() {
        let (world, _, _) = populated_world();
        let registry = registry();
        let snapshot = registry.capture(&world).expect("capture");

        let sum = registry.checksum(&snapshot).expect("checksum");
        assert_eq!(sum.0, snapshot.envelope.checksum);

        let mut tampered = snapshot.clone();
        tampered.records[0]
            .fields
            .insert("hp".to_owned(), SnapshotValue::I64(999));
        let tampered_sum = registry.checksum(&tampered).expect("checksum");
        assert_ne!(tampered_sum, sum);
    }

    #[test]
    fn checksum_rejects_records_outside_the_declared_profile() {
        let (world, _, _) = populated_world();
        let registry = registry();
        let snapshot = registry.capture(&world).expect("capture");

        let ghost = SnapshotRecord {
            id: 99,
            component: SchemaId::new("test.ghost"),
            fields: BTreeMap::new(),
        };
        let err = registry
            .checksum(&snapshot_with_extra_record(&snapshot, ghost))
            .expect_err("ghost component undeclared");
        assert!(matches!(err, StateError::UndeclaredComponent(_)));
    }

    #[test]
    fn restore_round_trip_recreates_state_on_fresh_entities() {
        let (mut world, hero, sidekick) = populated_world();
        let registry = registry();
        let snapshot = registry.capture(&world).expect("capture");
        let hero_hp = health_of(&world, hero);
        let sidekick_hp = health_of(&world, sidekick);

        let report = registry.restore(&mut world, &snapshot).expect("restore");

        // One fresh entity per snapshot-local ID, remapped by the report.
        assert_eq!(report.entities.len(), 2);
        let hero_now = report.entity(0).expect("hero remapped");
        let sidekick_now = report.entity(1).expect("sidekick remapped");
        assert_ne!(hero_now, hero, "restore spawns fresh entities");
        assert_ne!(sidekick_now, sidekick, "restore spawns fresh entities");
        assert_eq!(health_of(&world, hero_now), hero_hp);
        assert_eq!(health_of(&world, sidekick_now), sidekick_hp);
        // The entity reference now points at the fresh hero.
        assert_eq!(
            world.get::<Follows>(sidekick_now).expect("follows").target,
            hero_now
        );
        assert_eq!(
            health_of(
                &world,
                world.get::<Follows>(sidekick_now).expect("follows").target
            ),
            10
        );
    }

    #[test]
    fn restore_with_undeclared_schema_aborts_without_mutations() {
        let (mut world, hero, _) = populated_world();
        let registry = registry();
        let snapshot = registry.capture(&world).expect("capture");
        let count_before = world.entity_count();
        let ghost = SnapshotRecord {
            id: 99,
            component: SchemaId::new("test.ghost"),
            fields: BTreeMap::new(),
        };

        let err = registry
            .restore(&mut world, &snapshot_with_extra_record(&snapshot, ghost))
            .expect_err("ghost schema undeclared");
        assert!(matches!(err, StateError::UndeclaredComponent(_)));
        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on abort"
        );
        assert_eq!(health_of(&world, hero), 10);
    }

    #[test]
    fn restore_with_unregistered_schema_aborts_without_mutations() {
        let (source, _, _) = populated_world();
        let registry = registry();
        let snapshot = registry.capture(&source).expect("capture");

        // The profile declares follows, but this world never registered it.
        let mut world = World::new();
        world
            .register_component::<Health>()
            .expect("register health");
        let bystander = world.spawn();
        world.insert(bystander, Health { hp: 5 }).expect("insert");

        let err = registry
            .restore(&mut world, &snapshot)
            .expect_err("follows unregistered");
        assert!(matches!(err, StateError::UnknownSchema(_)));
        assert_eq!(world.entity_count(), 1, "zero mutations on abort");
        assert_eq!(health_of(&world, bystander), 5);
    }

    #[test]
    fn restore_with_decode_failure_aborts_without_mutations() {
        let (mut world, hero, _) = populated_world();
        let registry = registry();
        let mut snapshot = registry.capture(&world).expect("capture");
        let count_before = world.entity_count();
        let health_record = snapshot
            .records
            .iter_mut()
            .find(|record| record.component.as_str() == Health::SCHEMA_ID)
            .expect("health record");
        health_record
            .fields
            .insert("hp".to_owned(), SnapshotValue::Str("ten".to_owned()));

        let err = registry
            .restore(&mut world, &snapshot)
            .expect_err("hp no longer decodes");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on abort"
        );
        assert_eq!(health_of(&world, hero), 10);
    }

    #[test]
    fn restore_with_dangling_entity_ref_aborts_without_mutations() {
        let (mut world, hero, _) = populated_world();
        let registry = registry();
        let mut snapshot = registry.capture(&world).expect("capture");
        let count_before = world.entity_count();
        let follows_record = snapshot
            .records
            .iter_mut()
            .find(|record| record.component.as_str() == Follows::SCHEMA_ID)
            .expect("follows record");
        follows_record.fields.insert(
            "target".to_owned(),
            SnapshotValue::Map(BTreeMap::from([(
                "$entity".to_owned(),
                SnapshotValue::U64(77),
            )])),
        );

        let err = registry
            .restore(&mut world, &snapshot)
            .expect_err("dangling entity ref");
        assert!(matches!(err, StateError::UnresolvableEntityRef { id: 77 }));
        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on abort"
        );
        assert_eq!(health_of(&world, hero), 10);
    }

    #[test]
    fn malformed_entity_ref_shapes_abort_without_mutations() {
        // `check_value_refs` malformed-`$entity` branches: a non-`U64`
        // payload and an over-full map are both `MigrationInvalid`, and both
        // abort in phase one with the world untouched.
        for malformed in [
            BTreeMap::from([(
                "$entity".to_owned(),
                SnapshotValue::Str("not-an-id".to_owned()),
            )]),
            BTreeMap::from([
                ("$entity".to_owned(), SnapshotValue::U64(0)),
                ("extra".to_owned(), SnapshotValue::U64(2)),
            ]),
        ] {
            let (mut world, hero, _) = populated_world();
            let registry = registry();
            let mut snapshot = registry.capture(&world).expect("capture");
            let count_before = world.entity_count();
            let follows_record = snapshot
                .records
                .iter_mut()
                .find(|record| record.component.as_str() == Follows::SCHEMA_ID)
                .expect("follows record");
            follows_record
                .fields
                .insert("target".to_owned(), SnapshotValue::Map(malformed));

            let err = registry
                .restore(&mut world, &snapshot)
                .expect_err("malformed entity ref");
            assert!(
                matches!(err, StateError::MigrationInvalid { .. }),
                "malformed $entity must fail typed, got {err:?}"
            );
            assert_eq!(
                world.entity_count(),
                count_before,
                "zero mutations on abort"
            );
            assert_eq!(health_of(&world, hero), 10);
        }
    }

    #[test]
    fn envelope_schema_or_version_mismatch_aborts_without_mutations() {
        // `restore_inner` envelope agreement: a snapshot from another
        // simulation or another version aborts before any record is read.
        let (mut world, hero, _) = populated_world();
        let registry = registry();
        let count_before = world.entity_count();

        let mut foreign = registry.capture(&world).expect("capture");
        foreign.envelope.schema = SchemaId::new("test.other-sim");
        let err = registry
            .restore(&mut world, &foreign)
            .expect_err("foreign schema refused");
        assert!(
            matches!(err, StateError::MigrationInvalid { .. }),
            "schema mismatch must fail typed, got {err:?}"
        );

        let mut stale = registry.capture(&world).expect("capture");
        stale.envelope.version = SchemaVersion(0);
        let err = registry
            .restore(&mut world, &stale)
            .expect_err("stale version refused");
        assert!(
            matches!(err, StateError::MigrationInvalid { .. }),
            "version mismatch must fail typed, got {err:?}"
        );

        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on abort"
        );
        assert_eq!(health_of(&world, hero), 10);
    }

    #[test]
    fn restore_bytes_rejects_a_tampered_payload_without_mutations() {
        let (mut world, hero, _) = populated_world();
        let registry = registry();
        let (mut bytes, _) = registry.capture_bytes(&world).expect("capture bytes");
        let count_before = world.entity_count();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;

        let err = registry
            .restore_bytes(&mut world, &bytes)
            .expect_err("tampered payload rejected");
        assert!(matches!(
            err,
            StateError::ChecksumMismatch { .. } | StateError::SnapshotCodec(_)
        ));
        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on abort"
        );
        assert_eq!(health_of(&world, hero), 10);
    }

    #[test]
    fn presentation_state_survives_capture_and_restore_untouched() {
        let (mut world, _, _) = populated_world();
        let registry = registry();
        // An entity with no simulated components at all: pure presentation.
        let hud = world.spawn();
        world
            .insert(
                hud,
                RenderHandle {
                    label: "PRESENTATION-SENTINEL-hud".to_owned(),
                },
            )
            .expect("insert hud handle");

        let snapshot = registry.capture(&world).expect("capture");
        // Two simulated entities, three records: the presentation-only
        // entity gains no snapshot-local ID and contributes no records.
        assert_eq!(snapshot.records.len(), 3);
        assert!(snapshot.records.iter().all(|record| record.id <= 1));
        registry.restore(&mut world, &snapshot).expect("restore");

        assert!(world.is_alive(hud), "presentation entity survives restore");
        assert_eq!(
            world.get::<RenderHandle>(hud).expect("hud handle").label,
            "PRESENTATION-SENTINEL-hud"
        );
        assert_eq!(
            world.resource::<UiSelection>().expect("ui state").focused,
            "PRESENTATION-SENTINEL-focus"
        );
    }

    #[test]
    fn simulation_step_is_deterministic_per_seed() {
        let dt = Duration::from_millis(16);
        let mut first_world = World::new();
        let mut second_world = World::new();
        let mut first = Simulation::new(99);
        let mut second = Simulation::new(99);
        for frame in 0..3 {
            let input = test_input(frame);
            let first_report = first.step(&mut first_world, &input, dt);
            let second_report = second.step(&mut second_world, &input, dt);
            assert_eq!(first_report, second_report);
        }
        assert_eq!(first.tick(), 3);
        assert_eq!(first.sim_time(), dt * 3);
        let clock = first_world.resource::<SimClock>().expect("sim clock");
        assert_eq!(clock.tick, 3);
        assert_eq!(clock.sim_time, dt * 3);
        assert_eq!(clock.step, dt);
        assert_eq!(clock.frame_index, 2);

        let mut other_world = World::new();
        let mut other = Simulation::new(100);
        let other_report = other.step(&mut other_world, &test_input(0), dt);
        let mut same_world = World::new();
        let mut same = Simulation::new(99);
        let same_report = same.step(&mut same_world, &test_input(0), dt);
        assert_ne!(
            other_report.rng_value, same_report.rng_value,
            "distinct seeds must diverge"
        );
    }

    #[test]
    fn simulation_step_advances_clock_without_touching_components() {
        // `Simulation::step` owns the tick, the clock, one RNG draw, and
        // the `SimClock` publication — and nothing else. Stepping a
        // populated world must leave every simulated component
        // byte-identical while the clock advances: a step that scribbled on
        // components would fork every save after it, and the old version of
        // this test could not catch that — it stepped a world holding no
        // document-adjacent state and asserted a locally-built change log
        // stayed empty, which passes no matter what `step` does.
        let dt = Duration::from_millis(16);
        let (mut world, hero, sidekick) = populated_world();
        let before = registry().capture(&world).expect("capture before");
        let mut simulation = Simulation::new(7);

        for frame in 0..5 {
            simulation.step(&mut world, &test_input(frame), dt);
        }

        assert_eq!(simulation.tick(), 5);
        assert_eq!(simulation.sim_time(), dt * 5);
        let clock = world.resource::<SimClock>().expect("sim clock");
        assert_eq!(
            (clock.tick, clock.sim_time, clock.step, clock.frame_index),
            (5, dt * 5, dt, 4),
            "the published clock names the last step exactly"
        );
        let after = registry().capture(&world).expect("capture after");
        assert_eq!(before, after, "steps must not touch simulated components");
        assert_eq!(health_of(&world, hero), 10);
        assert_eq!(health_of(&world, sidekick), 20);
    }

    /// Decode-call recorder for the re-decode proof test: one entry per
    /// `apply_snapshot` call, holding the fields seen and the debug form of
    /// every entity `resolve` returned. Cleared at each test start.
    static LINK_CALLS: std::sync::Mutex<Vec<(BTreeMap<String, SnapshotValue>, Vec<String>)>> =
        std::sync::Mutex::new(Vec::new());

    /// Entity-link component that records every decode call. Identical in
    /// shape to `Follows` but observable, so the proof test can compare the
    /// phase-one (placeholder) decode against the phase-two (fresh) one.
    #[derive(Debug, Clone, PartialEq)]
    struct TracedLink {
        target: Entity,
    }

    impl CanaryComponent for TracedLink {
        const SCHEMA_ID: &'static str = "test.traced-link";
    }

    impl SimComponent for TracedLink {
        fn write_snapshot(
            world: &World,
            entity: Entity,
            remap: &mut RemapTable,
        ) -> Option<BTreeMap<String, SnapshotValue>> {
            world.get::<TracedLink>(entity).map(|link| {
                BTreeMap::from([("target".to_owned(), encode_entity_ref(remap, link.target))])
            })
        }

        fn apply_snapshot(
            fields: &BTreeMap<String, SnapshotValue>,
            resolve: &dyn Fn(u32) -> Option<Entity>,
        ) -> Result<Self, StateError> {
            let id = decode_entity_ref(fields.get("target").ok_or_else(|| {
                StateError::MigrationInvalid {
                    schema: Self::SCHEMA_ID.to_owned(),
                    to: 0,
                    reason: "test.traced-link is missing its target".to_owned(),
                }
            })?)
            .ok_or_else(|| StateError::MigrationInvalid {
                schema: Self::SCHEMA_ID.to_owned(),
                to: 0,
                reason: "test.traced-link wants an entity ref".to_owned(),
            })?;
            let target = resolve(id).ok_or(StateError::UnresolvableEntityRef { id })?;
            LINK_CALLS
                .lock()
                .expect("link recorder")
                .push((fields.clone(), vec![format!("{target:?}")]));
            Ok(Self { target })
        }
    }

    #[test]
    fn phase_two_redecode_is_identical_and_failure_free() {
        LINK_CALLS.lock().expect("link recorder").clear();
        let mut world = World::new();
        world
            .register_component::<Health>()
            .expect("register health");
        world
            .register_component::<TracedLink>()
            .expect("register link");
        let hero = world.spawn();
        world.insert(hero, Health { hp: 10 }).expect("insert");
        let sidekick = world.spawn();
        world.insert(sidekick, Health { hp: 20 }).expect("insert");
        world
            .insert(sidekick, TracedLink { target: hero })
            .expect("insert");
        let registry = SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![
                ComponentBinding::of::<Health>(),
                ComponentBinding::of::<TracedLink>(),
            ],
            Vec::new(),
        );
        let snapshot = registry.capture(&world).expect("capture");

        registry.restore(&mut world, &snapshot).expect("restore");

        // Three component records (hero health, sidekick health, sidekick
        // link) decode once in phase one and once in phase two.
        let calls = LINK_CALLS.lock().expect("link recorder");
        assert_eq!(calls.len(), 2, "link decodes exactly twice");
        assert_eq!(calls[0].0, calls[1].0, "identical fields both phases");
        assert_ne!(
            calls[0].1, calls[1].1,
            "placeholder versus fresh entities: the re-decode really ran"
        );
        // Both resolves returned Some (the call pushes only after a
        // successful resolve), so phase two's resolve was total — the same
        // totality phase one's reference check proved before any mutation.
    }

    /// Deterministic simulation config resource for the participation test.
    #[derive(Debug, Clone, PartialEq)]
    struct SimConfig {
        gravity: f64,
    }

    impl SimResource for SimConfig {
        const SCHEMA_ID: &'static str = "test.sim-config";

        fn read_snapshot(
            world: &World,
            _remap: &mut RemapTable,
        ) -> Option<BTreeMap<String, SnapshotValue>> {
            world.resource::<SimConfig>().map(|config| {
                BTreeMap::from([("gravity".to_owned(), SnapshotValue::F64(config.gravity))])
            })
        }

        fn write_snapshot(
            world: &mut World,
            fields: &BTreeMap<String, SnapshotValue>,
            _resolve: &dyn Fn(u32) -> Option<Entity>,
        ) -> Result<(), StateError> {
            match fields.get("gravity") {
                Some(SnapshotValue::F64(gravity)) => {
                    world.insert_resource(SimConfig { gravity: *gravity });
                    Ok(())
                }
                other => Err(StateError::MigrationInvalid {
                    schema: Self::SCHEMA_ID.to_owned(),
                    to: 0,
                    reason: format!("test.sim-config wants F64 gravity, got {other:?}"),
                }),
            }
        }
    }

    fn resource_registry() -> SnapshotRegistry {
        SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![ComponentBinding::of::<Health>()],
            vec![ResourceBinding::of::<SimConfig>()],
        )
    }

    /// Upstream resource for the validation-order proof test: its publish
    /// reads `BetaState`, so it only succeeds when `test.beta` is already
    /// published. Sorts before `test.beta`, while snapshot record order
    /// (top-down reserved IDs, ascending sort) lists beta first — the two
    /// orders disagree, which is exactly what the test exploits.
    #[derive(Debug, Clone, PartialEq)]
    struct AlphaState {
        score: i64,
    }

    /// Downstream resource with no dependencies: always publishes cleanly.
    #[derive(Debug, Clone, PartialEq)]
    struct BetaState {
        score: i64,
    }

    impl SimResource for AlphaState {
        const SCHEMA_ID: &'static str = "test.alpha";

        fn read_snapshot(
            world: &World,
            _remap: &mut RemapTable,
        ) -> Option<BTreeMap<String, SnapshotValue>> {
            world.resource::<AlphaState>().map(|alpha| {
                BTreeMap::from([("score".to_owned(), SnapshotValue::I64(alpha.score))])
            })
        }

        fn write_snapshot(
            world: &mut World,
            fields: &BTreeMap<String, SnapshotValue>,
            _resolve: &dyn Fn(u32) -> Option<Entity>,
        ) -> Result<(), StateError> {
            // Cross-resource read: `test.beta` must already be published.
            // Resource publishes run in schema order, so this documents the
            // contract — and the scratch validation must run in that same
            // order or it proves nothing about phase two.
            world
                .resource::<BetaState>()
                .ok_or_else(|| StateError::MigrationInvalid {
                    schema: Self::SCHEMA_ID.to_owned(),
                    to: 0,
                    reason: "test.alpha needs test.beta published first".to_owned(),
                })?;
            match fields.get("score") {
                Some(SnapshotValue::I64(score)) => {
                    world.insert_resource(AlphaState { score: *score });
                    Ok(())
                }
                other => Err(StateError::MigrationInvalid {
                    schema: Self::SCHEMA_ID.to_owned(),
                    to: 0,
                    reason: format!("test.alpha wants I64 score, got {other:?}"),
                }),
            }
        }
    }

    impl SimResource for BetaState {
        const SCHEMA_ID: &'static str = "test.beta";

        fn read_snapshot(
            world: &World,
            _remap: &mut RemapTable,
        ) -> Option<BTreeMap<String, SnapshotValue>> {
            world
                .resource::<BetaState>()
                .map(|beta| BTreeMap::from([("score".to_owned(), SnapshotValue::I64(beta.score))]))
        }

        fn write_snapshot(
            world: &mut World,
            fields: &BTreeMap<String, SnapshotValue>,
            _resolve: &dyn Fn(u32) -> Option<Entity>,
        ) -> Result<(), StateError> {
            match fields.get("score") {
                Some(SnapshotValue::I64(score)) => {
                    world.insert_resource(BetaState { score: *score });
                    Ok(())
                }
                other => Err(StateError::MigrationInvalid {
                    schema: Self::SCHEMA_ID.to_owned(),
                    to: 0,
                    reason: format!("test.beta wants I64 score, got {other:?}"),
                }),
            }
        }
    }

    fn dependency_registry() -> SnapshotRegistry {
        SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![ComponentBinding::of::<Health>()],
            vec![
                ResourceBinding::of::<AlphaState>(),
                ResourceBinding::of::<BetaState>(),
            ],
        )
    }

    /// Entity-link resource for the resource-carried reference test: the
    /// same `{ "$entity": id }` shape components use, but riding on a
    /// resource record instead of an entity record.
    #[derive(Debug, Clone, PartialEq)]
    struct HerdLeader {
        leader: Entity,
    }

    impl SimResource for HerdLeader {
        const SCHEMA_ID: &'static str = "test.herd-leader";

        fn read_snapshot(
            world: &World,
            remap: &mut RemapTable,
        ) -> Option<BTreeMap<String, SnapshotValue>> {
            world.resource::<HerdLeader>().map(|herd| {
                BTreeMap::from([("leader".to_owned(), encode_entity_ref(remap, herd.leader))])
            })
        }

        fn write_snapshot(
            world: &mut World,
            fields: &BTreeMap<String, SnapshotValue>,
            resolve: &dyn Fn(u32) -> Option<Entity>,
        ) -> Result<(), StateError> {
            let invalid = |reason: String| StateError::MigrationInvalid {
                schema: Self::SCHEMA_ID.to_owned(),
                to: 0,
                reason,
            };
            match fields.get("leader") {
                Some(value) => match decode_entity_ref(value) {
                    Some(id) => match resolve(id) {
                        Some(leader) => {
                            world.insert_resource(HerdLeader { leader });
                            Ok(())
                        }
                        None => Err(StateError::UnresolvableEntityRef { id }),
                    },
                    None => Err(invalid(format!(
                        "test.herd-leader wants an entity ref, got {value:?}"
                    ))),
                },
                None => Err(invalid("test.herd-leader is missing its leader".to_owned())),
            }
        }
    }

    fn leader_registry() -> SnapshotRegistry {
        SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![
                ComponentBinding::of::<Health>(),
                ComponentBinding::of::<Follows>(),
            ],
            vec![ResourceBinding::of::<HerdLeader>()],
        )
    }

    #[test]
    fn resource_participation_is_explicit_and_excludes_the_rest() {
        let mut world = World::new();
        world
            .register_component::<Health>()
            .expect("register health");
        let hero = world.spawn();
        world.insert(hero, Health { hp: 3 }).expect("insert");
        world.insert_resource(SimConfig { gravity: 9.81 });
        world.insert_resource(UiSelection {
            focused: "PRESENTATION-SENTINEL-focus".to_owned(),
        });

        let registry = resource_registry();
        let (bytes, _) = registry.capture_bytes(&world).expect("capture bytes");
        assert!(
            !bytes
                .windows(b"PRESENTATION-SENTINEL-focus".len())
                .any(|window| window == b"PRESENTATION-SENTINEL-focus"),
            "unbound resources never enter the payload"
        );
        let snapshot = registry.capture(&world).expect("capture");
        assert!(
            snapshot
                .records
                .iter()
                .any(|record| record.component.as_str() == SimConfig::SCHEMA_ID),
            "bound resources are captured"
        );

        let mut fresh = World::new();
        fresh
            .register_component::<Health>()
            .expect("register health");
        fresh.insert_resource(UiSelection {
            focused: "PRESENTATION-SENTINEL-stays".to_owned(),
        });
        registry.restore(&mut fresh, &snapshot).expect("restore");
        assert_eq!(fresh.resource::<SimConfig>().expect("config").gravity, 9.81);
        assert_eq!(
            fresh.resource::<UiSelection>().expect("ui state").focused,
            "PRESENTATION-SENTINEL-stays",
            "unbound resources survive restore untouched"
        );
    }

    #[test]
    fn duplicate_resource_records_are_rejected_before_mutation() {
        let mut world = World::new();
        world
            .register_component::<Health>()
            .expect("register health");
        world.insert_resource(SimConfig { gravity: 1.0 });
        let registry = resource_registry();
        let snapshot = registry.capture(&world).expect("capture");
        let mut doubled = snapshot.clone();
        let resource = snapshot
            .records
            .iter()
            .find(|record| record.component.as_str() == SimConfig::SCHEMA_ID)
            .expect("resource record")
            .clone();
        doubled.records.push(resource);
        let count_before = world.entity_count();

        let err = registry
            .restore(&mut world, &doubled)
            .expect_err("duplicate resource refused");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
        assert_eq!(world.entity_count(), count_before);
    }

    #[test]
    fn same_seed_determinism_holds_across_the_save_restore_boundary() {
        let dt = Duration::from_millis(16);
        let (mut world, _, _) = populated_world();
        // Register the presentation components the fixture world needs;
        // the sim boundary stays health-plus-follows.
        let mut sim = Simulation::new(5);
        for frame in 0..3 {
            sim.step(&mut world, &test_input(frame), dt);
        }
        let registry = registry();
        let snapshot = registry.capture_with_sim(&world, &sim).expect("capture");

        // A fresh world and a deliberately wrong-seeded simulation: the
        // restore must overwrite both entity state and sim-core state.
        let (mut fresh, _, _) = populated_world();
        let doomed_count = fresh.entity_count();
        let mut resumed = Simulation::new(0xBEEF);
        let report = registry
            .restore_with_sim(&mut fresh, &snapshot, &mut resumed)
            .expect("restore with sim");
        assert_eq!(report.entities.len(), 2);
        assert_ne!(
            fresh.entity_count(),
            doomed_count + 2,
            "doomed entities are gone"
        );
        assert_eq!(resumed.tick(), 3);
        assert_eq!(resumed.sim_time(), dt * 3);
        assert_eq!(resumed.rng_state(), sim.rng_state());

        // Both simulations now draw identically, step for step.
        for frame in 3..6 {
            let input = test_input(frame);
            let continued = sim.step(&mut world, &input, dt);
            let revived = resumed.step(&mut fresh, &input, dt);
            assert_eq!(continued, revived, "frame {frame} diverges");
        }
        let (first_bytes, _) = registry
            .capture_bytes_with_sim(&world, &sim)
            .expect("capture");
        let (second_bytes, _) = registry
            .capture_bytes_with_sim(&fresh, &resumed)
            .expect("capture");
        assert_eq!(first_bytes, second_bytes);
    }

    #[test]
    fn plain_restore_refuses_sim_carrying_snapshots_without_mutations() {
        let dt = Duration::from_millis(16);
        let (mut world, hero, _) = populated_world();
        let mut sim = Simulation::new(5);
        sim.step(&mut world, &test_input(0), dt);
        let snapshot = registry().capture_with_sim(&world, &sim).expect("capture");
        let count_before = world.entity_count();

        let err = registry()
            .restore(&mut world, &snapshot)
            .expect_err("plain restore must refuse sim state");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
        assert_eq!(world.entity_count(), count_before);
        assert_eq!(health_of(&world, hero), 10);
    }

    #[test]
    fn restore_with_sim_requires_sim_state_without_mutations() {
        let (mut world, hero, _) = populated_world();
        let snapshot = registry().capture(&world).expect("capture without sim");
        let count_before = world.entity_count();
        let mut sim = Simulation::new(5);

        let err = registry()
            .restore_with_sim(&mut world, &snapshot, &mut sim)
            .expect_err("sim state required");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
        assert_eq!(world.entity_count(), count_before);
        assert_eq!(health_of(&world, hero), 10);
        assert_eq!(sim.tick(), 0, "simulation untouched");
    }

    #[test]
    fn migration_fixture_applies_on_the_restore_path() {
        use canary_state::{MigrationChain, MigrationStep};
        // Old schema v0 stores `health`; current v1 stores `hp`. The bridge
        // migrates the fields, then the ordinary staged restore consumes them.
        let mut chain = MigrationChain::new(SchemaId::new("test.health"));
        chain
            .push(MigrationStep {
                from: 0,
                description: "rename 'health' to 'hp'",
                run: Box::new(|mut body: serde_json::Value| {
                    if let Some(health) = body.get("health").cloned() {
                        if let Some(map) = body.as_object_mut() {
                            map.remove("health");
                            map.insert("hp".to_owned(), health);
                        }
                    }
                    Ok(body)
                }),
            })
            .expect("linear push");
        let old_fields = BTreeMap::from([("health".to_owned(), SnapshotValue::I64(42))]);
        let migrated = chain
            .migrate_fields(&old_fields, SchemaVersion(0), SchemaVersion(1))
            .expect("migrate");
        let snapshot = Snapshot {
            envelope: canary_state::SnapshotEnvelope {
                schema: SchemaId::new("test.sim"),
                version: SchemaVersion(1),
                encoding: canary_state::schema::SNAPSHOT_ENCODING,
                checksum: String::new(),
            },
            records: vec![SnapshotRecord {
                id: 0,
                component: SchemaId::new(Health::SCHEMA_ID),
                fields: migrated,
            }],
        };

        let mut world = World::new();
        register_all(&mut world);
        registry().restore(&mut world, &snapshot).expect("restore");
        let entity = world.query::<Health>().next().expect("health").0;
        assert_eq!(health_of(&world, entity), 42);
    }

    /// Unknown-tolerant component for the preservation fixture: it stores
    /// every field it decodes and writes them all back, so fields the
    /// current build does not interpret still round-trip.
    #[derive(Debug, Clone, PartialEq)]
    struct Echo {
        fields: BTreeMap<String, SnapshotValue>,
    }

    impl CanaryComponent for Echo {
        const SCHEMA_ID: &'static str = "test.echo";
    }

    impl SimComponent for Echo {
        fn write_snapshot(
            world: &World,
            entity: Entity,
            _remap: &mut RemapTable,
        ) -> Option<BTreeMap<String, SnapshotValue>> {
            world.get::<Echo>(entity).map(|echo| echo.fields.clone())
        }

        fn apply_snapshot(
            fields: &BTreeMap<String, SnapshotValue>,
            _resolve: &dyn Fn(u32) -> Option<Entity>,
        ) -> Result<Self, StateError> {
            Ok(Self {
                fields: fields.clone(),
            })
        }
    }

    #[test]
    fn restore_then_recapture_is_byte_stable() {
        let (mut world, _, _) = populated_world();
        let registry = registry();
        let (first_bytes, _) = registry.capture_bytes(&world).expect("capture");

        registry
            .restore_bytes(&mut world, &first_bytes)
            .expect("restore");
        let (second_bytes, _) = registry.capture_bytes(&world).expect("recapture");
        assert_eq!(
            first_bytes, second_bytes,
            "capture→restore→recapture must be byte-stable"
        );
    }

    #[test]
    fn unknown_fields_survive_a_capture_restore_cycle_byte_identical() {
        let registry = SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![ComponentBinding::of::<Echo>()],
            Vec::new(),
        );
        let mut world = World::new();
        world.register_component::<Echo>().expect("register echo");
        let entity = world.spawn();
        world
            .insert(
                entity,
                Echo {
                    fields: BTreeMap::from([
                        ("hp".to_owned(), SnapshotValue::I64(7)),
                        (
                            "future_flag".to_owned(),
                            SnapshotValue::Str("tomorrow".to_owned()),
                        ),
                    ]),
                },
            )
            .expect("insert");

        let (first_bytes, _) = registry.capture_bytes(&world).expect("capture");
        let mut fresh = World::new();
        fresh.register_component::<Echo>().expect("register echo");
        registry
            .restore_bytes(&mut fresh, &first_bytes)
            .expect("restore");
        let (second_bytes, _) = registry.capture_bytes(&fresh).expect("recapture");
        assert_eq!(
            first_bytes, second_bytes,
            "unknown fields must survive capture→restore byte-identical"
        );
    }

    #[test]
    fn interrupted_snapshot_save_recovers_then_continues_deterministically() {
        use canary_state::{load_snapshot, save_snapshot};
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "canary-runtime-snapshot-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("sim.bin");

        let dt = Duration::from_millis(16);
        let (mut world, _, _) = populated_world();
        let mut sim = Simulation::new(11);
        for frame in 0..2 {
            sim.step(&mut world, &test_input(frame), dt);
        }
        let registry = registry();
        let (good_bytes, _) = registry
            .capture_bytes_with_sim(&world, &sim)
            .expect("capture bytes");
        save_snapshot(&path, &good_bytes).expect("first save");

        // Crashed writer: partial sibling temp, no rename. The committed
        // file still loads and verifies. The temp name appends the suffix
        // (`sim.bin.tmp`), never replacing the extension — see
        // `canary_state::authored::atomic_write`.
        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, &good_bytes[..good_bytes.len() / 2]).expect("plant partial");
        let recovered = load_snapshot(&path).expect("recover");
        decode_snapshot(&recovered).expect("good file verifies");

        // A truncated final file fails typed at the checksum gate — never a
        // partial restore — and the next good save recovers the path.
        std::fs::write(&path, &good_bytes[..good_bytes.len() / 2]).expect("truncate");
        let broken = load_snapshot(&path).expect("truncated file still reads");
        assert!(matches!(
            decode_snapshot(&broken).unwrap_err(),
            StateError::ChecksumMismatch { .. } | StateError::SnapshotCodec(_)
        ));
        save_snapshot(&path, &good_bytes).expect("second save");
        assert!(!tmp.exists(), "rename consumes the temp file");

        // The recovered file continues deterministically: restore into a
        // wrong-seeded simulation, step both, compare.
        let loaded = load_snapshot(&path).expect("reload");
        let (mut fresh, _, _) = populated_world();
        let mut resumed = Simulation::new(0xFFFF);
        registry
            .restore_bytes_with_sim(&mut fresh, &loaded, &mut resumed)
            .expect("restore");
        for frame in 2..4 {
            let input = test_input(frame);
            assert_eq!(
                sim.step(&mut world, &input, dt),
                resumed.step(&mut fresh, &input, dt),
                "post-recovery step diverges"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recycled_slot_generations_capture_as_distinct_entities() {
        // Proof for the tuple-key remap: the same slot index recycled across
        // generations must yield two canonical IDs. A `u64` hash of the pair
        // could alias them into one ID and merge their records.
        let mut world = World::new();
        register_all(&mut world);
        let first = world.spawn();
        world.insert(first, Health { hp: 1 }).expect("insert");
        let index = first.index();
        world.despawn(first).expect("despawn");
        let second = world.spawn();
        world.insert(second, Health { hp: 2 }).expect("insert");
        assert_eq!(
            second.index(),
            index,
            "test needs slot reuse to prove generation keying"
        );
        assert_ne!(
            first.generation(),
            second.generation(),
            "test needs distinct generations"
        );
        // Both handles alive in sequence — capture each alone and require
        // distinct canonical assignments through the tuple key.
        let mut table = RemapTable::default();
        let first_id = table.assign(first.index(), first.generation());
        let second_id = table.assign(second.index(), second.generation());
        assert_ne!(first_id, second_id, "tuple keys never alias recycled slots");

        // And the live capture round-trips the surviving generation's value.
        let snapshot = registry().capture(&world).expect("capture");
        assert_eq!(snapshot.records.len(), 1);
        let mut fresh = World::new();
        register_all(&mut fresh);
        let report = registry().restore(&mut fresh, &snapshot).expect("restore");
        let entity = report.entity(0).expect("remapped");
        assert_eq!(health_of(&fresh, entity), 2);
    }

    #[test]
    fn resource_record_id_overflow_is_a_typed_error() {
        assert!(
            matches!(
                resource_record_id(usize::try_from(u64::from(u32::MAX) + 1).unwrap_or(usize::MAX)),
                Err(StateError::MigrationInvalid { .. })
            ),
            "out-of-range resource index must fail typed"
        );
        // The first reserved ID sits just below the sim-core record.
        assert_eq!(
            resource_record_id(0).expect("first resource ID"),
            SIM_STATE_ID - 1
        );
        // Boundary: the last usable slot maps to canonical ID 0, and the
        // next index exhausts the reserved range — both typed, never wrapped.
        let last_valid = usize::try_from(SIM_STATE_ID - 1).expect("fits usize");
        assert_eq!(resource_record_id(last_valid).expect("last resource ID"), 0);
        let exhausted = usize::try_from(SIM_STATE_ID).expect("fits usize");
        assert!(
            matches!(
                resource_record_id(exhausted),
                Err(StateError::MigrationInvalid { .. })
            ),
            "exhausted reserved range must fail typed"
        );
    }

    #[test]
    fn bad_resource_fields_abort_restore_without_mutations() {
        // Proof for scratch-world resource validation: a resource record
        // that fails decode must abort before the entities are despawned.
        // Without phase-one resource validation the entities would already
        // be gone when the publish failed.
        let mut world = World::new();
        world
            .register_component::<Health>()
            .expect("register health");
        let hero = world.spawn();
        world.insert(hero, Health { hp: 3 }).expect("insert");
        world.insert_resource(SimConfig { gravity: 9.81 });
        let registry = resource_registry();
        let mut snapshot = registry.capture(&world).expect("capture");
        let resource = snapshot
            .records
            .iter_mut()
            .find(|record| record.component.as_str() == SimConfig::SCHEMA_ID)
            .expect("resource record");
        resource.fields.insert(
            "gravity".to_owned(),
            SnapshotValue::Str("not-a-float".to_owned()),
        );
        let count_before = world.entity_count();

        let err = registry
            .restore(&mut world, &snapshot)
            .expect_err("bad resource fails");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on resource abort"
        );
        assert_eq!(health_of(&world, hero), 3);
        assert_eq!(
            world.resource::<SimConfig>().expect("config").gravity,
            9.81,
            "published resource untouched"
        );
    }

    #[test]
    fn duplicate_resource_schemas_fail_fast_at_capture() {
        let registry = SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![ComponentBinding::of::<Health>()],
            vec![
                ResourceBinding::of::<SimConfig>(),
                ResourceBinding::of::<SimConfig>(),
            ],
        );
        let mut world = World::new();
        world
            .register_component::<Health>()
            .expect("register health");
        world.insert_resource(SimConfig { gravity: 1.0 });
        let err = registry.capture(&world).expect_err("duplicates refused");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
    }

    #[test]
    fn duplicate_component_bindings_fail_fast_at_capture() {
        // Mirror of the resource fail-fast: two bindings for one component
        // schema would emit two records per entity, which restore refuses —
        // so capture refuses first instead of writing an unrestorable file.
        let registry = SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![
                ComponentBinding::of::<Health>(),
                ComponentBinding::of::<Health>(),
            ],
            Vec::new(),
        );
        let (world, _, _) = populated_world();
        let err = registry.capture(&world).expect_err("duplicates refused");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
    }

    #[test]
    fn duplicate_entity_records_are_rejected_before_mutation() {
        // A hand-doubled `(id, schema)` pair would decode twice with the
        // last write winning silently; restore refuses it in phase one.
        let (mut world, hero, _) = populated_world();
        let registry = registry();
        let snapshot = registry.capture(&world).expect("capture");
        let mut doubled = snapshot.clone();
        let record = snapshot
            .records
            .iter()
            .find(|record| record.component.as_str() == Health::SCHEMA_ID)
            .expect("health record")
            .clone();
        doubled.records.push(record);
        let count_before = world.entity_count();

        let err = registry
            .restore(&mut world, &doubled)
            .expect_err("duplicate refused");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on abort"
        );
        assert_eq!(health_of(&world, hero), 10);
    }

    #[test]
    fn cross_resource_reads_validate_in_publish_order() {
        // `test.alpha` publishes by reading `test.beta`, and publishes run
        // in schema order (alpha first). Snapshot record order is the
        // reverse (beta's reserved ID sorts lower), so a scratch validation
        // in record order would see beta-then-alpha succeed while phase two
        // runs alpha-then-beta and fails after the entities were already
        // respawned. Validation must run in publish order, aborting before
        // the first mutation instead.
        let mut source = World::new();
        source
            .register_component::<Health>()
            .expect("register health");
        let hero = source.spawn();
        source.insert(hero, Health { hp: 3 }).expect("insert");
        source.insert_resource(AlphaState { score: 1 });
        source.insert_resource(BetaState { score: 10 });
        let registry = dependency_registry();
        let snapshot = registry.capture(&source).expect("capture");

        // A fresh world with no resources: alpha cannot publish first.
        let mut fresh = World::new();
        fresh
            .register_component::<Health>()
            .expect("register health");
        let err = registry
            .restore(&mut fresh, &snapshot)
            .expect_err("alpha-before-beta fails");
        assert!(matches!(err, StateError::MigrationInvalid { .. }));
        assert_eq!(
            fresh.entity_count(),
            0,
            "resource abort precedes every world mutation"
        );
        assert!(
            fresh.resource::<AlphaState>().is_none(),
            "no partial resource publish"
        );
    }

    #[test]
    fn reordered_resource_bindings_capture_byte_identical() {
        // The constructor sorts both binding lists, so caller push order —
        // components and resources alike — never reaches the bytes.
        let mut world = World::new();
        world
            .register_component::<Health>()
            .expect("register health");
        let hero = world.spawn();
        world.insert(hero, Health { hp: 3 }).expect("insert");
        world.insert_resource(AlphaState { score: 1 });
        world.insert_resource(BetaState { score: 10 });
        let ordered = SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![ComponentBinding::of::<Health>()],
            vec![
                ResourceBinding::of::<AlphaState>(),
                ResourceBinding::of::<BetaState>(),
            ],
        );
        let reversed = SnapshotRegistry::new(
            SchemaId::new("test.sim"),
            SchemaVersion(1),
            vec![ComponentBinding::of::<Health>()],
            vec![
                ResourceBinding::of::<BetaState>(),
                ResourceBinding::of::<AlphaState>(),
            ],
        );
        let (first_bytes, first_sum) = ordered.capture_bytes(&world).expect("capture");
        let (second_bytes, second_sum) = reversed.capture_bytes(&world).expect("capture");
        assert_eq!(first_bytes, second_bytes);
        assert_eq!(first_sum, second_sum);
    }

    #[test]
    fn resource_carried_dangling_refs_abort_without_mutations() {
        // Entity-reference totality covers resource records too: a resource
        // pointing at a snapshot-local ID with no entity record aborts in
        // phase one, before the entities are touched.
        let (mut world, hero, _) = populated_world();
        world.insert_resource(HerdLeader { leader: hero });
        let registry = leader_registry();
        let mut snapshot = registry.capture(&world).expect("capture");
        let resource = snapshot
            .records
            .iter_mut()
            .find(|record| record.component.as_str() == HerdLeader::SCHEMA_ID)
            .expect("resource record");
        resource.fields.insert(
            "leader".to_owned(),
            SnapshotValue::Map(BTreeMap::from([(
                "$entity".to_owned(),
                SnapshotValue::U64(77),
            )])),
        );
        let count_before = world.entity_count();

        let err = registry
            .restore(&mut world, &snapshot)
            .expect_err("dangling resource ref");
        assert!(matches!(err, StateError::UnresolvableEntityRef { id: 77 }));
        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on abort"
        );
        assert_eq!(health_of(&world, hero), 10);
        assert_eq!(
            world.resource::<HerdLeader>().expect("herd").leader,
            hero,
            "published resource untouched"
        );
    }

    #[test]
    fn dead_reference_never_aliases_a_recycled_slot() {
        // Proof that the tuple-key remap cannot silently rewire a dangling
        // reference: the sidekick follows the hero, the hero dies, a
        // newcomer reuses the hero's slot (same index, new generation), and
        // the sidekick's stored handle still names the dead generation.
        // Tuple keying assigns the dead target and the newcomer distinct
        // canonical IDs, so restore refuses the dangling reference; an
        // index-only key would alias them and silently rewire the sidekick
        // to follow the newcomer.
        let mut world = World::new();
        register_all(&mut world);
        let hero = world.spawn();
        world.insert(hero, Health { hp: 1 }).expect("insert");
        let sidekick = world.spawn();
        world.insert(sidekick, Health { hp: 2 }).expect("insert");
        world
            .insert(sidekick, Follows { target: hero })
            .expect("insert");
        let hero_index = hero.index();
        let hero_generation = hero.generation();
        world.despawn(hero).expect("despawn hero");
        let newcomer = world.spawn();
        world.insert(newcomer, Health { hp: 99 }).expect("insert");
        assert_eq!(
            newcomer.index(),
            hero_index,
            "test needs slot reuse to prove generation keying"
        );
        assert_ne!(
            newcomer.generation(),
            hero_generation,
            "test needs distinct generations"
        );

        let snapshot = registry().capture(&world).expect("capture");
        let follows_record = snapshot
            .records
            .iter()
            .find(|record| record.component.as_str() == Follows::SCHEMA_ID)
            .expect("follows record");
        let dead_target =
            decode_entity_ref(follows_record.fields.get("target").expect("follows target"))
                .expect("entity ref shape");
        let newcomer_record = snapshot
            .records
            .iter()
            .find(|record| {
                record.component.as_str() == Health::SCHEMA_ID
                    && record.fields.get("hp") == Some(&SnapshotValue::I64(99))
            })
            .expect("newcomer health record");
        assert_ne!(
            dead_target, newcomer_record.id,
            "dead handle and recycled slot share an index but never an ID"
        );

        let count_before = world.entity_count();
        let err = registry()
            .restore(&mut world, &snapshot)
            .expect_err("dead reference refused");
        assert!(
            matches!(err, StateError::UnresolvableEntityRef { id } if id == dead_target),
            "dangling dead reference fails typed, got {err:?}"
        );
        assert_eq!(
            world.entity_count(),
            count_before,
            "zero mutations on abort"
        );
        assert_eq!(health_of(&world, newcomer), 99);
    }

    #[test]
    fn empty_world_captures_and_restores_cleanly() {
        let mut world = World::new();
        register_all(&mut world);
        let registry = registry();
        let snapshot = registry.capture(&world).expect("capture empty");
        assert!(snapshot.records.is_empty());
        registry
            .restore(&mut world, &snapshot)
            .expect("restore empty");
        assert_eq!(world.entity_count(), 0);
    }

    #[test]
    fn capture_with_sim_before_any_step_carries_zero_clock() {
        // No `SimClock` resource published yet: the sim record must carry
        // zero step/frame values and still restore deterministically.
        let (world, _, _) = populated_world();
        let sim = Simulation::new(13);
        let snapshot = registry()
            .capture_with_sim(&world, &sim)
            .expect("capture with sim");
        let state = snapshot
            .records
            .iter()
            .find(|record| {
                record.id == SIM_STATE_ID && record.component.as_str() == SIM_STATE_SCHEMA
            })
            .expect("sim record present");
        let unpacked = SimStateSnapshot::from_record(state).expect("unpack");
        assert_eq!(unpacked.tick, 0);
        assert_eq!(unpacked.step_nanos, 0);
        assert_eq!(unpacked.frame_index, 0);

        let (mut fresh, _, _) = populated_world();
        let mut resumed = Simulation::new(0xBEEF);
        registry()
            .restore_with_sim(&mut fresh, &snapshot, &mut resumed)
            .expect("restore");
        assert_eq!(resumed.tick(), 0);
        assert_eq!(resumed.rng_state(), sim.rng_state());
    }

    #[test]
    fn ticks_do_not_record_authored_changes_and_edits_do_not_advance_the_world() {
        // `Simulation::step` takes no document handle so it cannot call
        // `record_change`; `record_change` takes no `World` so it cannot
        // advance the tick. Pin both directions.
        let dt = Duration::from_millis(16);
        let mut world = World::new();
        let mut sim = Simulation::new(7);
        let mut doc = canary_state::AuthoredDocument::new(
            canary_state::ProjectId::generate().expect("project id"),
        );
        for frame in 0..3 {
            sim.step(&mut world, &test_input(frame), dt);
        }
        assert_eq!(sim.tick(), 3);
        assert!(doc.changes.is_empty(), "ticks record no authored changes");
        doc.record_change("authored edit");
        assert_eq!(doc.changes.len(), 1);
        assert_eq!(sim.tick(), 3, "authored edits advance no world tick");
        assert_eq!(
            world.resource::<SimClock>().expect("sim clock").tick,
            3,
            "authored edits publish no clock"
        );
    }
}
