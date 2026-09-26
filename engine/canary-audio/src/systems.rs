// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The audio trigger system: game state in, backend calls out.
//!
//! # Mechanism (read this before touching the phase order)
//!
//! Gameplay writes [`AudioSource::state`](crate::AudioSource) as *intent*;
//! [`audio_trigger_system`] actuates it against an [`AudioBackend`]
//! every tick, in strict phases (snapshot → drive backend → update
//! table), because `World::resource_mut::<Mutex<B>>()` holds `&mut
//! World`, which forbids concurrent queries — so the system never
//! interleaves ECS reads with the backend borrow, mirroring the physics
//! step system's snapshot discipline.
//!
//! 1. **Ensure resources.** Missing [`AudioConfig`]/
//!    [`AudioVoices`]/`Mutex<B>` are inserted from defaults (never
//!    overwriting): a test registering only the system gets a working
//!    world. The backend default is the headless/decode-only voice
//!    table — construction must never need a device, so registration
//!    order never depends on hardware.
//! 2. **Snapshot** every entity carrying [`AudioSource`]
//!    (shared borrows only), resolving source and listener poses from
//!    propagated [`GlobalTransform`] components (a missing source pose
//!    means non-positional: full gain, no attenuation) and the listener
//!    pose from the first
//!    [`AudioListener`] entity (absent listener
//!    means [`AudioListener::default_pose`]).
//! 3. **Drive** the backend under the mutex: reap tracked voices whose
//!    entity died or lost its source (stop + forget — despawn-racing-a-tick
//!    degrades to a skip, never a panic, and no orphaned voice survives),
//!    then reconcile each record's `(state, tracked voice)` pair: play
//!    on the stopped→playing edge, pause/resume on the playing↔paused
//!    edges, stop on →stopped, and refresh every playing voice's gain
//!    as `master × source × attenuation`.
//! 4. **Pump** the backend. A failed pump is deliberately NOT
//!    propagated: failing a game tick over audio would invert the real
//!    dependency (gameplay must survive sound failure, not the reverse).
//!
//! # Why after gameplay writes (the ordering law)
//!
//! Gameplay WRITES [`AudioSource`] (state
//! transitions are the trigger); this system READS it. Registration
//! order is therefore load-bearing: the trigger system MUST be
//! registered after the gameplay systems whose transitions it actuates,
//! so a scripted state change plays the same tick instead of lagging
//! one tick behind. The two ordering tests below pin both directions
//! (fresh-read when ordered, stale-read when reversed), mirroring the
//! physics-before-propagation precedent.
//!
//! # Unknown sound handles retry, they do not fail the tick
//!
//! A `Playing` source whose asset handle is not (yet) in the world's
//! [`AssetStore`] yields no voice and keeps
//! its state — retried next tick. This is the streaming-friendly
//! answer: an asset that arrives a tick late starts a tick late,
//! instead of a permanently-missing handle failing ticks forever or a
//! transiently-missing one killing a voice the game still wants.
//!
//! # No finish events in this cut (deliberate)
//!
//! One-shot voices are NOT polled for completion and the system never
//! writes [`AudioSource::state`](crate::AudioSource) back: completion
//! reporting arrives with the asset-streaming milestone, not here. The
//! game stops voices explicitly (or despawns them); the system frees
//! backend slots on exactly those paths, which is the no-orphans
//! invariant the tests pin.

use std::collections::HashMap;
use std::sync::Mutex;

use canary_assets::{AssetStore, Sound};
use canary_ecs::{Entity, World};
use canary_scheduler::{Schedule, SystemAccess};
use canary_transform::GlobalTransform;

use crate::{AudioBackend, AudioConfig, AudioListener, AudioSource, SourceHandle, SourceState};

/// A tracked voice: which backend slot an entity's source owns, plus
/// the backend-side pause bit.
///
/// The pause bit is system-side (not re-queried from the backend every
/// tick) so pause/resume calls happen exactly on transitions: without
/// it, every tick would re-issue resume to playing voices and the
/// backend call journal — the game proof's observable — would drown in
/// no-op repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceEntry {
    /// The backend voice slot this entity's source owns.
    pub handle: SourceHandle,
    /// Whether the backend voice is currently paused (set when the
    /// system pauses it, cleared when the system resumes it).
    pub paused: bool,
}

/// The system-owned entity→voice table: which backend voice each live
/// source owns.
///
/// Lives as a `World` resource (named in
/// [`audio_trigger_access`]), owned exclusively by
/// [`audio_trigger_system`] — gameplay never touches it, which is what
/// keeps every [`AudioSource`] field single-writer (game) while voices
/// still reconcile exactly. Keyed by [`Entity`]
/// (index + generation), so a despawned entity's entry can never alias
/// a later entity recycled into the same index: the generation differs,
/// the lookup misses, the newborn is treated as untracked.
#[derive(Debug, Default)]
pub struct AudioVoices {
    voices: HashMap<Entity, VoiceEntry>,
}

impl AudioVoices {
    /// An empty table owning no voices.
    pub fn new() -> Self {
        Self {
            voices: HashMap::new(),
        }
    }

    /// The tracked voice for `entity`, or `None` if untracked (never
    /// played, or reaped after stop/despawn).
    pub fn get(&self, entity: Entity) -> Option<VoiceEntry> {
        self.voices.get(&entity).copied()
    }

    /// Tracks `entry` for `entity`, replacing any previous entry.
    /// Called only by [`audio_trigger_system`] after the backend
    /// borrow ends.
    pub fn insert(&mut self, entity: Entity, entry: VoiceEntry) {
        self.voices.insert(entity, entry);
    }

    /// Forgets `entity`'s tracked voice, returning it. Called only by
    /// [`audio_trigger_system`] after stopping the backend voice.
    pub fn remove(&mut self, entity: Entity) -> Option<VoiceEntry> {
        self.voices.remove(&entity)
    }

    /// Every tracked entity. Used for the reap pass.
    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.voices.keys().copied()
    }

    /// How many voices are currently tracked. Exists for tests and
    /// debug overlays — gameplay reasons about entities, never this
    /// table.
    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Whether no voices are tracked.
    pub fn is_empty(&self) -> bool {
        self.voices.is_empty()
    }
}

/// Canary-owned distance attenuation: world-space distance in, linear
/// gain out.
///
/// Inverse-distance `1 / (1 + d)`: unity at the listener, one half at
/// one world unit, one quarter at three — the small math ADR 0023
/// assigns to Canary rather than to rodio's `SpatialPlayer` (which has
/// an open upstream bug with speed + looped decoders). A non-positive
/// distance reads as zero distance (full gain); a non-finite distance
/// (a NaN position somewhere upstream) also reads as full gain rather
/// than poisoning the mix with NaN — the backend's volume validation
/// stays the backstop, this is the front one.
///
/// This is *attenuation only*: HRTF, Doppler, occlusion, and reverb
/// zones are explicitly deferred (later audio milestones), and this
/// function must not grow them quietly.
pub fn attenuation_gain(distance: f32) -> f32 {
    1.0 / (1.0 + distance.max(0.0))
}

/// Declares [`audio_trigger_system`]'s data access: reads intents and
/// poses, writes voices and the backend.
///
/// Every clause earns its place in the ordering mechanism
/// (registration order + solo-write staging — see the module docs):
///
/// - `reads::<AudioSource>()` conflicts with gameplay's
///   `writes::<AudioSource>()`, so audio can never share a stage with
///   the gameplay writes it actuates and — registered after them —
///   always runs after them. The system genuinely never writes
///   `AudioSource` (no finish events in this cut), so the read-only
///   declaration is honest, not a staging trick.
/// - `reads::<GlobalTransform>()` / `reads::<AudioListener>()` name the
///   attenuation inputs. Register transform propagation before audio so
///   these are world-space poses for the current simulation run.
/// - `reads_resource::<AudioConfig>()` /
///   `reads_resource::<AssetStore<Sound>>()` name the consumed mix and
///   asset data; `writes_resource::<AudioVoices>()` /
///   `writes_resource::<Mutex<B>>()` the mutated table and backend.
///   The backend as a mutex-wrapped resource (rather than a
///   system-captured singleton) keeps the system signature at
///   `fn(&mut World)` — the scheduler's doctrine — and lets tests swap
///   the backend through the world. The `Mutex` (not a bare `B`) is
///   the `Send + Sync` resource bound made explicit: device backends
///   hold OS stream handles that are `Send` but never `Sync`, so the
///   resource wraps the backend instead of pretending the device is
///   shareable. The system itself holds `&mut World` (write systems
///   run alone), so it uses `Mutex::get_mut` — exclusive access with
///   no locking and no poison path.
pub fn audio_trigger_access<B: AudioBackend + 'static>() -> SystemAccess {
    SystemAccess::new()
        .reads::<AudioSource>()
        .reads::<GlobalTransform>()
        .reads::<AudioListener>()
        .reads_resource::<AudioConfig>()
        .reads_resource::<AssetStore<Sound>>()
        .writes_resource::<AudioVoices>()
        .writes_resource::<Mutex<B>>()
}

/// Owned per-entity snapshot: everything one tick needs from ECS,
/// collected through shared queries BEFORE the backend borrow begins.
/// `World::resource_mut::<Mutex<B>>()` holds `&mut World`, which
/// forbids concurrent queries — so the system works in strict phases
/// (snapshot → drive backend → update table), never interleaved.
/// Everything here is owned (`Sound` is cloned, not borrowed) so no
/// lifetime escapes the snapshot phase.
struct SourceRecord {
    /// The entity owning this source.
    entity: Entity,
    /// The game-owned playback intent.
    state: SourceState,
    /// The source gain folded into the per-tick volume product.
    volume: f32,
    /// The loop mode fixed at the next play.
    looping: bool,
    /// World-space position when the entity carries a propagated
    /// `GlobalTransform`; `None` means non-positional (full gain, no
    /// attenuation).
    position: Option<[f32; 3]>,
    /// The voice the table currently tracks for this entity, if any.
    voice: Option<VoiceEntry>,
    /// The decoded sound for a voice about to start (`Playing` with
    /// no tracked voice and a resolving handle); `None` otherwise —
    /// unknown handles simply carry `None` and retry next tick.
    sound_data: Option<Sound>,
}

/// Drives backend voices from game state: snapshot → reap → actuate →
/// pump.
///
/// See the module docs for the phase order and why each phase exists;
/// see [`audio_trigger_access`] for how the access declaration plus
/// registration-after-gameplay turns the declaration into the ordering
/// guarantee. Type parameters: `B` is the backend behind the
/// `Mutex<B>` world resource (rodio in the game, the stub in tests).
pub fn audio_trigger_system<B: AudioBackend + Default + 'static>(world: &mut World) {
    if world.resource::<AudioConfig>().is_none() {
        world.insert_resource(AudioConfig::default());
    }
    if world.resource::<AudioVoices>().is_none() {
        world.insert_resource(AudioVoices::new());
    }
    if world.resource::<Mutex<B>>().is_none() {
        world.insert_resource(Mutex::new(B::default()));
    }

    // Snapshot (shared borrows only): every source plus its pose,
    // its tracked voice, the sound bytes for voices about to start,
    // and the listener pose. All owned — no borrow survives this,
    // because the drive phase holds `&mut World` through the backend
    // resource. `Sound` is cloned only on the play edge (a Playing
    // source with no tracked voice whose handle resolves), never
    // speculatively for every source every tick.
    let config = world
        .resource::<AudioConfig>()
        .map_or(AudioConfig::default(), |config| *config);
    let records: Vec<SourceRecord> = world
        .query::<AudioSource>()
        .map(|(entity, source)| {
            let position = world.get::<GlobalTransform>(entity).map(|transform| {
                transform
                    .matrix()
                    .transform_point3(glam::Vec3::ZERO)
                    .to_array()
            });
            let voice = tracked_voice(world, entity);
            let sound_data = match (source.state, voice) {
                (SourceState::Playing, None) => world
                    .resource::<AssetStore<Sound>>()
                    .and_then(|store| store.get(source.sound))
                    .cloned(),
                _ => None,
            };
            SourceRecord {
                entity,
                state: source.state,
                volume: source.volume,
                looping: source.looping,
                position,
                voice,
                sound_data,
            }
        })
        .collect();
    let (listener_position, listener_forward) = world
        .query::<AudioListener>()
        .filter_map(|(entity, _)| {
            world
                .get::<GlobalTransform>(entity)
                .map(audio_listener_world_pose)
        })
        .next()
        .unwrap_or_else(AudioListener::default_pose);

    // Reap list (shared borrows only): tracked entities that died or
    // lost their AudioSource since the last tick, each with the voice
    // to stop. Owned pairs — the drive phase touches no ECS at all.
    let reap: Vec<(Entity, Option<VoiceEntry>)> =
        world
            .resource::<AudioVoices>()
            .map_or(Vec::new(), |voices| {
                voices
                    .entities()
                    .filter(|entity| {
                        !world.is_alive(*entity) || world.get::<AudioSource>(*entity).is_none()
                    })
                    .map(|entity| (entity, voices.get(entity)))
                    .collect()
            });

    // Drive under the backend borrow; collect owned table mutations so
    // the AudioVoices writes happen AFTER the borrow ends (separate
    // `&mut World` borrow, sequential not nested).
    let mut remove: Vec<Entity> = Vec::new();
    let mut upsert: Vec<(Entity, VoiceEntry)> = Vec::new();
    if let Some(mutex) = world.resource_mut::<Mutex<B>>() {
        // Exclusive access through the `&mut World` borrow: `get_mut`
        // skips locking entirely (no contention — write systems run
        // alone in their own stage). The `Mutex` still earns its
        // place: it is what makes an OS-device backend (`Send` but
        // never `Sync`) a legal `Send + Sync` resource. A poisoned
        // mutex means someone panicked while holding a shared lock;
        // that panic already propagated, so this tick skips audio
        // rather than adding a second failure on top of it.
        let Ok(backend) = mutex.get_mut() else {
            return;
        };

        for (entity, voice) in reap {
            if let Some(entry) = voice {
                backend.stop(entry.handle);
            }
            remove.push(entity);
        }

        for record in &records {
            match (record.state, record.voice) {
                (SourceState::Playing, None) => {
                    // Unknown sound handle (`None` data): no voice,
                    // state kept, retried next tick (see the module
                    // docs — streaming-friendly, never a failed tick).
                    if let Some(sound) = &record.sound_data {
                        if let Ok(handle) = backend.play_sound(sound, record.looping) {
                            let gain = effective_gain(
                                config.master_volume,
                                record.volume,
                                record.position,
                                listener_position,
                            );
                            // A rejected initial gain (NaN
                            // source volume) keeps the
                            // backend default; the per-tick
                            // refresh below will Err-skip the
                            // same way until the game fixes it.
                            if backend.set_volume(handle, gain).is_err() {
                                // Intentionally ignored: the
                                // voice exists and plays; only
                                // its gain update was refused.
                            }
                            upsert.push((
                                record.entity,
                                VoiceEntry {
                                    handle,
                                    paused: false,
                                },
                            ));
                        }
                    }
                }
                (SourceState::Playing, Some(entry)) => {
                    if entry.paused && backend.resume(entry.handle) {
                        upsert.push((
                            record.entity,
                            VoiceEntry {
                                handle: entry.handle,
                                paused: false,
                            },
                        ));
                    }
                    let gain = effective_gain(
                        config.master_volume,
                        record.volume,
                        record.position,
                        listener_position,
                    );
                    if backend.set_volume(entry.handle, gain).is_err() {
                        // Intentionally ignored: a refused gain
                        // (non-finite/negative game volume, or a
                        // backend that reaped behind the table)
                        // keeps the voice's last good gain and
                        // must never fail the tick.
                    }
                }
                (SourceState::Paused, Some(entry)) => {
                    if !entry.paused && backend.pause(entry.handle) {
                        upsert.push((
                            record.entity,
                            VoiceEntry {
                                handle: entry.handle,
                                paused: true,
                            },
                        ));
                    }
                }
                (SourceState::Paused, None) => {}
                (SourceState::Stopped, Some(entry)) => {
                    backend.stop(entry.handle);
                    remove.push(record.entity);
                }
                (SourceState::Stopped, None) => {}
            }
        }

        backend.set_listener(listener_position, listener_forward);
        if backend.pump().is_err() {
            // Intentionally ignored: gameplay survives sound
            // failure, not the reverse (see the module docs).
        }
    }

    // Table updates after the backend borrow ends.
    if !remove.is_empty() || !upsert.is_empty() {
        if let Some(voices) = world.resource_mut::<AudioVoices>() {
            for entity in remove {
                voices.remove(entity);
            }
            for (entity, entry) in upsert {
                voices.insert(entity, entry);
            }
        }
    }
}

/// Extracts world position and a normalized world-forward direction from
/// a propagated matrix, ignoring scale for the listener's orientation.
fn audio_listener_world_pose(transform: &GlobalTransform) -> ([f32; 3], [f32; 3]) {
    let matrix = transform.matrix();
    let position = matrix.transform_point3(glam::Vec3::ZERO);
    let forward = matrix.transform_vector3(glam::Vec3::NEG_Z);
    let forward = if forward.is_finite() && forward.length_squared() > 0.0 {
        forward.normalize()
    } else {
        glam::Vec3::NEG_Z
    };
    (position.to_array(), forward.to_array())
}

/// Reads one entity's tracked voice: the table is a separate resource
/// from the backend, so this nests a shared table borrow inside the
/// drive phase rather than interleaving ECS access with backend
/// mutation.
fn tracked_voice(world: &World, entity: Entity) -> Option<VoiceEntry> {
    world
        .resource::<AudioVoices>()
        .and_then(|voices| voices.get(entity))
}

/// Folds one voice's per-tick gain: `master × source × attenuation`.
///
/// Non-positional sources (no propagated `GlobalTransform`) attenuate to
/// unity.
/// Non-finite products (a NaN game volume) pass through to the
/// backend, which refuses them typed and keeps the last good gain —
/// this function invents no clamping policy the backend would then
/// have to agree with.
fn effective_gain(master: f32, source: f32, position: Option<[f32; 3]>, listener: [f32; 3]) -> f32 {
    let attenuation = match position {
        Some(at) => {
            let dx = at[0] - listener[0];
            let dy = at[1] - listener[1];
            let dz = at[2] - listener[2];
            attenuation_gain((dx * dx + dy * dy + dz * dz).sqrt())
        }
        None => 1.0,
    };
    master * source * attenuation
}

/// Registers [`audio_trigger_system`] on `schedule` as a write system
/// with [`audio_trigger_access`]'s declaration.
///
/// MUST be called AFTER transform propagation and the gameplay systems
/// whose [`AudioSource`] state transitions it actuates — registration order plus
/// solo-write staging is the ordering mechanism (see the module docs);
/// registering audio before its gameplay writers actuates one-tick-stale
/// intents. Type parameter `B` names the backend behind the `Mutex<B>`
/// world resource (`RodioBackend` in the game, the stub in tests). The
/// host initializes an audible backend resource before the first run;
/// `B::default()` is the headless fallback.
pub fn register_audio_trigger<B: AudioBackend + Default + 'static>(schedule: &mut Schedule) {
    schedule.add_write_system(audio_trigger_access::<B>(), audio_trigger_system::<B>);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{StubBackend, StubEvent};
    use crate::AudioBackendName;
    use canary_assets::{load_sound, AssetHandle};
    use canary_transform::Transform;

    /// Path to a checked-in sound fixture (WAV + Ogg live in
    /// `canary-assets/tests/fixtures/` — the asset pipeline this
    /// crate plays through, not copies of it).
    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures")
            .join(name)
    }

    /// A store holding the mono WAV fixture plus its handle.
    fn fixture_store() -> (AssetStore<Sound>, AssetHandle<Sound>) {
        let sound = load_sound(&fixture_path("tone-mono-8k.wav"))
            .expect("WAV fixture must load for the game proof");
        let mut store = AssetStore::new();
        let handle = store.insert(sound);
        (store, handle)
    }

    /// A world with store + default config + stub backend + empty
    /// voice table and one stopped, unity-gain, one-shot source.
    fn setup() -> (World, Entity) {
        let (store, handle) = fixture_store();
        let mut world = World::new();
        world.insert_resource(store);
        world.insert_resource(AudioConfig::default());
        world.insert_resource(Mutex::new(StubBackend::new()));
        world.insert_resource(AudioVoices::new());
        let entity = world.spawn();
        world
            .insert(entity, AudioSource::new(handle))
            .expect("source insert must succeed");
        (world, entity)
    }

    /// Drives one trigger tick.
    fn tick(world: &mut World) {
        audio_trigger_system::<StubBackend>(world);
    }

    /// Clones the stub's call journal.
    fn journal(world: &World) -> Vec<StubEvent> {
        world
            .resource::<Mutex<StubBackend>>()
            .expect("backend resource must exist")
            .lock()
            .expect("test mutex must not be poisoned")
            .journal
            .clone()
    }

    /// Live voices on the stub backend.
    fn backend_count(world: &World) -> usize {
        world
            .resource::<Mutex<StubBackend>>()
            .expect("backend resource must exist")
            .lock()
            .expect("test mutex must not be poisoned")
            .source_count()
    }

    /// Tracked voices in the system table.
    fn tracked_count(world: &World) -> usize {
        world
            .resource::<AudioVoices>()
            .expect("voice table must exist")
            .voice_count()
    }

    /// The game writes intent: a scripted state change.
    fn set_state(world: &mut World, entity: Entity, state: SourceState) {
        world
            .get_mut::<AudioSource>(entity)
            .expect("source must still exist")
            .state = state;
    }

    /// The game proof: a scripted stopped→playing→paused→playing→
    /// stopped walk produces exactly the expected backend call order
    /// — play, pause, resume, stop, one gain refresh per playing tick —
    /// with the listener update and pump riding every tick. Asserted on
    /// the stub journal, never on audible output.
    #[test]
    fn scripted_state_change_drives_backend_in_exact_call_order() {
        let (mut world, entity) = setup();
        let voice = SourceHandle::from_raw_parts(0, 0);
        let (default_position, default_forward) = AudioListener::default_pose();

        tick(&mut world);
        assert_eq!(
            journal(&world),
            vec![
                StubEvent::SetListener {
                    position: default_position,
                    forward: default_forward,
                },
                StubEvent::Pump,
            ],
            "a stopped source actuates nothing, but listener + pump still ride the tick"
        );

        set_state(&mut world, entity, SourceState::Playing);
        tick(&mut world);
        assert_eq!(
            journal(&world)[2..],
            vec![
                StubEvent::Play {
                    handle: voice,
                    looping: false,
                },
                StubEvent::SetVolume {
                    handle: voice,
                    volume: 1.0,
                },
                StubEvent::SetListener {
                    position: default_position,
                    forward: default_forward,
                },
                StubEvent::Pump,
            ]
        );
        assert_eq!(backend_count(&world), 1);
        assert_eq!(tracked_count(&world), 1);

        set_state(&mut world, entity, SourceState::Paused);
        tick(&mut world);
        assert!(
            journal(&world).ends_with(&[
                StubEvent::Pause { handle: voice },
                StubEvent::SetListener {
                    position: default_position,
                    forward: default_forward,
                },
                StubEvent::Pump,
            ]),
            "pausing issues exactly one pause: {:?}",
            journal(&world)
        );

        // A second paused tick repeats nothing voice-side: the pause
        // bit in the table suppresses no-op repeats.
        let len = journal(&world).len();
        tick(&mut world);
        assert_eq!(
            journal(&world)[len..],
            vec![
                StubEvent::SetListener {
                    position: default_position,
                    forward: default_forward,
                },
                StubEvent::Pump,
            ]
        );

        set_state(&mut world, entity, SourceState::Playing);
        tick(&mut world);
        assert!(
            journal(&world).ends_with(&[
                StubEvent::Resume { handle: voice },
                StubEvent::SetVolume {
                    handle: voice,
                    volume: 1.0,
                },
                StubEvent::SetListener {
                    position: default_position,
                    forward: default_forward,
                },
                StubEvent::Pump,
            ]),
            "resuming issues exactly one resume plus the gain refresh: {:?}",
            journal(&world)
        );

        set_state(&mut world, entity, SourceState::Stopped);
        tick(&mut world);
        assert!(journal(&world).ends_with(&[
            StubEvent::Stop { handle: voice },
            StubEvent::SetListener {
                position: default_position,
                forward: default_forward,
            },
            StubEvent::Pump,
        ]));
        assert_eq!(backend_count(&world), 0);
        assert_eq!(tracked_count(&world), 0);
    }

    /// Removal and despawn both stop playback: no orphaned voices.
    #[test]
    fn removal_and_despawn_stop_playback_with_no_orphans() {
        let (mut world, entity) = setup();
        set_state(&mut world, entity, SourceState::Playing);
        tick(&mut world);
        assert_eq!(backend_count(&world), 1);

        // Losing the component (not just despawn) reaps the voice.
        assert!(
            world.remove::<AudioSource>(entity).is_some(),
            "the source must exist to be removed"
        );
        tick(&mut world);
        assert_eq!(backend_count(&world), 0);
        assert_eq!(tracked_count(&world), 0);
        assert!(
            journal(&world)
                .iter()
                .any(|event| matches!(event, StubEvent::Stop { .. })),
            "component removal must stop the voice"
        );

        // A fresh source on a fresh entity plays again, then despawn
        // reaps it the same way.
        let store_handle = {
            let store = world
                .resource_mut::<AssetStore<Sound>>()
                .expect("store must exist");
            store.insert(load_sound(&fixture_path("tone-mono-8k.wav")).expect("fixture must load"))
        };
        let entity2 = world.spawn();
        world
            .insert(
                entity2,
                AudioSource {
                    state: SourceState::Playing,
                    ..AudioSource::new(store_handle)
                },
            )
            .expect("source insert must succeed");
        tick(&mut world);
        assert_eq!(backend_count(&world), 1);

        world.despawn(entity2).expect("despawn must succeed");
        tick(&mut world);
        assert_eq!(backend_count(&world), 0);
        assert_eq!(tracked_count(&world), 0);
        assert_eq!(
            journal(&world)
                .iter()
                .filter(|event| matches!(event, StubEvent::Stop { .. }))
                .count(),
            2,
            "removal and despawn each stop exactly one voice"
        );
    }

    /// An unknown sound handle yields no voice, keeps the intent, and
    /// recovers when the asset arrives — the streaming-friendly retry.
    #[test]
    fn unknown_sound_handle_yields_no_voice_and_recovers() {
        let (mut world, entity) = setup();
        let forged = AssetHandle::<Sound>::from_raw_parts(4242, 0);
        world
            .get_mut::<AudioSource>(entity)
            .expect("source must exist")
            .sound = forged;
        set_state(&mut world, entity, SourceState::Playing);

        tick(&mut world);
        assert_eq!(backend_count(&world), 0);
        assert_eq!(tracked_count(&world), 0);
        assert!(
            journal(&world)
                .iter()
                .all(|event| !matches!(event, StubEvent::Play { .. })),
            "no voice without an asset: {:?}",
            journal(&world)
        );
        assert_eq!(
            world
                .get::<AudioSource>(entity)
                .expect("source must exist")
                .state,
            SourceState::Playing,
            "the intent survives for the retry"
        );

        // The asset arrives: the same still-Playing source plays next
        // tick with no game-side repair.
        let handle = {
            let store = world
                .resource_mut::<AssetStore<Sound>>()
                .expect("store must exist");
            store.insert(load_sound(&fixture_path("tone-mono-8k.wav")).expect("fixture must load"))
        };
        world
            .get_mut::<AudioSource>(entity)
            .expect("source must exist")
            .sound = handle;
        tick(&mut world);
        assert_eq!(backend_count(&world), 1);
    }

    /// Per-tick gain uses composed world poses: master 0.5, source 0.5,
    /// and a parented source three world units from its listener
    /// (attenuation 0.25) → 0.0625. The listener's rotated parent also
    /// proves its world-forward direction reaches the backend.
    #[test]
    fn parented_world_poses_drive_attenuation_and_listener_orientation() {
        let (mut world, entity) = setup();
        world.insert_resource(AudioConfig::new(0.5, AudioBackendName::Rodio));

        let source_parent = world.spawn();
        world
            .insert(
                source_parent,
                Transform::from_translation(glam::Vec3::new(1.0, 0.0, 0.0)),
            )
            .expect("source parent transform must succeed");
        world
            .insert(
                entity,
                Transform::from_translation(glam::Vec3::new(2.0, 0.0, 0.0)),
            )
            .expect("source transform must succeed");
        canary_transform::set_parent(&mut world, entity, Some(source_parent))
            .expect("source parenting must succeed");

        let listener_parent = world.spawn();
        world
            .insert(
                listener_parent,
                Transform {
                    translation: glam::Vec3::ZERO,
                    rotation: glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                    scale: glam::Vec3::ONE,
                },
            )
            .expect("listener parent transform must succeed");
        let listener = world.spawn();
        world
            .insert(listener, AudioListener)
            .expect("listener insert must succeed");
        world
            .insert(listener, Transform::identity())
            .expect("listener pose must succeed");
        canary_transform::set_parent(&mut world, listener, Some(listener_parent))
            .expect("listener parenting must succeed");

        world.advance_tick();
        canary_transform::propagate_transforms(&mut world);
        world
            .get_mut::<AudioSource>(entity)
            .expect("source must exist")
            .volume = 0.5;
        set_state(&mut world, entity, SourceState::Playing);

        tick(&mut world);

        let voice = SourceHandle::from_raw_parts(0, 0);
        let applied = world
            .resource::<Mutex<StubBackend>>()
            .expect("backend must exist")
            .lock()
            .expect("test mutex must not be poisoned")
            .applied_volume(voice)
            .expect("the voice must exist");
        assert!(
            (applied - 0.0625).abs() < 1e-9,
            "master 0.5 × source 0.5 × attenuation(3.0)=0.25 must be 0.0625, got {applied}"
        );
        let listener_event = journal(&world)
            .into_iter()
            .find_map(|event| match event {
                StubEvent::SetListener { position, forward } => Some((position, forward)),
                _ => None,
            })
            .expect("the backend must receive the listener pose");
        assert_eq!(listener_event.0, [0.0, 0.0, 0.0]);
        assert!(
            listener_event
                .1
                .iter()
                .zip([-1.0, 0.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < 1e-6),
            "the listener's propagated world-forward must reach the backend: {:?}",
            listener_event.1
        );
    }

    /// A NaN source volume keeps the last good gain: refused typed at
    /// the backend boundary, never applied, never failing the tick.
    #[test]
    fn nan_source_volume_keeps_the_last_good_gain() {
        let (mut world, entity) = setup();
        set_state(&mut world, entity, SourceState::Playing);
        tick(&mut world);
        let voice = SourceHandle::from_raw_parts(0, 0);
        assert_eq!(
            world
                .resource::<Mutex<StubBackend>>()
                .expect("backend must exist")
                .lock()
                .expect("test mutex must not be poisoned")
                .applied_volume(voice),
            Some(1.0)
        );

        world
            .get_mut::<AudioSource>(entity)
            .expect("source must exist")
            .volume = f32::NAN;
        tick(&mut world);

        let backend = world
            .resource::<Mutex<StubBackend>>()
            .expect("backend must exist")
            .lock()
            .expect("test mutex must not be poisoned");
        assert_eq!(
            backend.applied_volume(voice),
            Some(1.0),
            "the refused gain must leave the last good gain in place"
        );
        assert_eq!(
            backend
                .journal
                .iter()
                .filter(|event| matches!(event, StubEvent::SetVolume { .. }))
                .count(),
            1,
            "the refused update journals nothing"
        );
    }

    /// A failing pump does not fail the tick: the play still lands.
    #[test]
    fn pump_failure_does_not_fail_the_tick() {
        let (mut world, entity) = setup();
        world
            .resource::<Mutex<StubBackend>>()
            .expect("backend must exist")
            .lock()
            .expect("test mutex must not be poisoned")
            .fail_pump = true;
        set_state(&mut world, entity, SourceState::Playing);

        tick(&mut world);

        assert_eq!(backend_count(&world), 1);
        assert!(
            journal(&world)
                .iter()
                .any(|event| matches!(event, StubEvent::Play { .. })),
            "the play must land despite the failed pump"
        );
    }

    /// The ordering law, correct direction: audio registered AFTER the
    /// gameplay write actuates the transition the same tick.
    #[test]
    fn gameplay_write_then_audio_sees_fresh_intent() {
        let (mut world, entity) = setup();
        let mut schedule = Schedule::new();
        schedule.add_write_system(
            SystemAccess::new().writes::<AudioSource>(),
            move |world: &mut World| {
                world
                    .get_mut::<AudioSource>(entity)
                    .expect("gameplay write must find the source")
                    .state = SourceState::Playing;
            },
        );
        register_audio_trigger::<StubBackend>(&mut schedule);
        schedule.run(&mut world);

        assert_eq!(
            backend_count(&world),
            1,
            "ordered audio must actuate the same-tick transition"
        );
    }

    /// The ordering law, failure proof: audio registered BEFORE its
    /// gameplay writer sees the pre-write intent, so no voice starts
    /// this tick while the intent already moved. If this ever
    /// goes green-by-failure (a voice despite reversed order), the
    /// scheduler's staging changed and every ordering claim needs
    /// re-proof.
    #[test]
    fn audio_before_gameplay_write_sees_stale_intent() {
        let (mut world, entity) = setup();
        let mut schedule = Schedule::new();
        register_audio_trigger::<StubBackend>(&mut schedule);
        schedule.add_write_system(
            SystemAccess::new().writes::<AudioSource>(),
            move |world: &mut World| {
                world
                    .get_mut::<AudioSource>(entity)
                    .expect("gameplay write must find the source")
                    .state = SourceState::Playing;
            },
        );
        schedule.run(&mut world);

        assert_eq!(
            backend_count(&world),
            0,
            "reversed audio must not see the later write"
        );
        assert_eq!(
            world
                .get::<AudioSource>(entity)
                .expect("source must exist")
                .state,
            SourceState::Playing,
            "the gameplay write still ran — only audio missed it this tick"
        );
    }

    /// A world with no backend resource gets the default (which needs
    /// no device), and the tick still plays.
    #[test]
    fn missing_backend_resource_is_created_from_default() {
        let (store, handle) = fixture_store();
        let mut world = World::new();
        world.insert_resource(store);
        let entity = world.spawn();
        world
            .insert(
                entity,
                AudioSource {
                    state: SourceState::Playing,
                    ..AudioSource::new(handle)
                },
            )
            .expect("source insert must succeed");

        tick(&mut world);

        assert_eq!(
            backend_count(&world),
            1,
            "the ensured default backend must drive the tick"
        );
    }

    /// Attenuation matches hand-computed values, including the
    /// degenerate inputs (negative, infinite, NaN distances).
    #[test]
    fn attenuation_matches_hand_computed_values() {
        assert_eq!(attenuation_gain(0.0), 1.0);
        assert_eq!(attenuation_gain(1.0), 0.5);
        assert_eq!(attenuation_gain(3.0), 0.25);
        assert_eq!(
            attenuation_gain(-5.0),
            1.0,
            "non-positive distance reads as zero distance"
        );
        assert_eq!(
            attenuation_gain(f32::INFINITY),
            0.0,
            "infinite distance silences"
        );
        assert_eq!(
            attenuation_gain(f32::NAN),
            1.0,
            "non-finite distance degrades to full gain, never NaN"
        );
    }

    // Attenuation properties: unit range and monotone falloff over
    // arbitrary nonnegative distances — the math as invariants, not
    // examples, mirroring the canary-ecs op-sequence proptest style.
    proptest::proptest! {
        #[test]
        fn attenuation_stays_in_unit_range(distance in 0.0f32..1.0e6) {
            let gain = attenuation_gain(distance);
            proptest::prop_assert!((0.0..=1.0).contains(&gain));
        }

        #[test]
        fn attenuation_falls_monotonically_with_distance(
            first in 0.0f32..1.0e6,
            second in 0.0f32..1.0e6,
        ) {
            let (near, far) = if first < second {
                (first, second)
            } else {
                (second, first)
            };
            proptest::prop_assert!(attenuation_gain(near) >= attenuation_gain(far));
        }
    }
}
