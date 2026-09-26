// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The private rodio backend: [`RodioBackend`], the v0.0.12 implementor
//! of [`AudioBackend`](crate::AudioBackend).
//!
//! This module is intentionally private (`mod rodio_backend;` in
//! `lib.rs`, with only the [`RodioBackend`] type re-exported): engine
//! and game code program against the [`AudioBackend`](crate::AudioBackend)
//! trait, never against this concrete type's rodio-flavored internals.
//! The re-export exists so the [`RodioBackend`] resource type can be
//! named in [`SystemAccess`](canary_scheduler::SystemAccess) declarations
//! and in `World::resource::<Mutex<RodioBackend>>()` calls — naming the
//! *type* is not depending on the *player*, the same way naming a trait
//! is not depending on its implementors. No `rodio` (and therefore no
//! re-exported `cpal`, no `symphonia`) type appears in any public
//! signature here; the `cargo doc` plus `grep` recheck pins that.
//!
//! # Mechanism (why this shape)
//!
//! The backend has two modes behind one type. **Device mode** opens the
//! OS default output sink at [`RodioBackend::try_new`] and plays one
//! rodio [`Player`](rodio::Player) per voice, fed by a
//! [`SamplesBuffer`](rodio::buffer::SamplesBuffer) built from the
//! [`Sound`](canary_assets::Sound) PCM the asset pipeline already
//! decoded — rodio never re-decodes files here, it only clocks owned
//! samples out to hardware. **Headless mode** (the
//! [`RodioBackend::headless`] constructor and the [`Default`] impl)
//! tracks the same voice lifecycle — slots, generations, pause bits,
//! gains, listener pose — against a simulation table and never touches
//! device code at all, so headless CI (no sound card) runs the full
//! trigger path with zero hardware dependence.
//!
//! Voices live in monotonic slots that are never recycled in this cut
//! (indices grow; generations stay zero): with no reuse there is no
//! aliasing class to defend, and a wrap-around needs four billion plays
//! in one backend lifetime. Stale-handle behavior still holds by
//! liveness check, exactly like the stub's.
//!
//! Distance attenuation is NOT rodio's `SpatialPlayer` (open upstream
//! bug with speed + looped decoders, per ADR 0023): the trigger system
//! computes Canary-owned gains and this backend only applies them via
//! [`Player::set_volume`](rodio::Player::set_volume). The stored
//! listener pose round-trips for tests and future backends.
//!
//! # No-device degradation (why `try_new` returns `Result`)
//!
//! Opening a device can fail (no sound card, no permission, device
//! lost): [`RodioBackend::try_new`] reports that typed as
//! [`AudioError::NoDevice`] or [`AudioError::StreamFailed`], never by
//! panicking. Mid-run device loss has no portable signal in this cut
//! (rodio exposes none cheaply), so [`pump`](crate::AudioBackend::pump)
//! is a documented no-op here: device health is a construction-time
//! property, and the seam keeps the fallible shape for backends that
//! can report more.

use std::collections::HashMap;
use std::num::NonZero;

use canary_assets::Sound;
use rodio::buffer::SamplesBuffer;
use rodio::Source as _;

use crate::{AudioBackend, AudioError, SourceHandle};

/// A headless voice slot: the lifecycle without a device.
struct HeadlessSlot {
    generation: u64,
    alive: bool,
    paused: bool,
    volume: f32,
}

/// A device voice: the rodio player behind a slot.
///
/// Dropping the `Player` stops its sounds, so freeing a slot IS
/// stopping the voice — no separate teardown call exists or is needed.
struct DeviceVoice {
    generation: u64,
    alive: bool,
    player: rodio::Player,
}

/// The backend's device posture: simulation table or OS sink.
enum BackendMode {
    /// Decode-only simulation: full voice lifecycle, no device code.
    Headless {
        /// Voice slots by index; never recycled in this cut.
        slots: HashMap<u32, HeadlessSlot>,
        /// Next slot index; grows monotonically.
        next_index: u32,
        /// Live voice count (the table could derive it, but an
        /// explicit counter keeps `source_count` O(1)).
        live: usize,
        /// Stored listener pose (position, forward).
        listener: ([f32; 3], [f32; 3]),
    },
    /// OS playback: one rodio player per voice on the default sink.
    Device {
        /// The OS output sink; dropping it ends all playback, so the
        /// backend owns it for its whole lifetime.
        sink: rodio::MixerDeviceSink,
        /// Voice slots by index; never recycled in this cut.
        voices: HashMap<u32, DeviceVoice>,
        /// Next slot index; grows monotonically.
        next_index: u32,
        /// Live voice count.
        live: usize,
        /// Stored listener pose (position, forward).
        listener: ([f32; 3], [f32; 3]),
    },
}

/// The rodio bootstrap backend: [`AudioBackend`](crate::AudioBackend)
/// over the OS default output device, with a decode-only headless mode
/// for hardware-less environments.
///
/// See the module docs for the two modes, the slot discipline, and the
/// no-device degradation contract. Construct with
/// [`RodioBackend::try_new`] where a device is expected (Ships with
/// typed device errors) or [`RodioBackend::headless`] where none may
/// exist (infallible, never touches device code).
pub struct RodioBackend {
    mode: BackendMode,
}

impl RodioBackend {
    /// Builds a decode-only backend: full voice lifecycle against a
    /// simulation table, zero device interaction.
    ///
    /// Infallible by construction — no device is opened, probed, or
    /// enumerated, so this is the constructor for headless CI,
    /// servers, and tests that must prove the trigger path without
    /// hardware. Voices play/pause/resume/stop and gains apply
    /// exactly as in device mode; without a device clock nothing
    /// auto-finishes (one-shot completion reporting is deferred with
    /// the streaming milestone, never invented here).
    pub fn headless() -> Self {
        Self {
            mode: BackendMode::Headless {
                slots: HashMap::new(),
                next_index: 0,
                live: 0,
                listener: ([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            },
        }
    }

    /// Opens the OS default output sink and builds a playing backend.
    ///
    /// Fails typed — [`AudioError::NoDevice`] when the OS reports no
    /// default output device, [`AudioError::StreamFailed`] when a
    /// device exists but its stream cannot start — never by panicking.
    /// Callers without a device guarantee use
    /// [`RodioBackend::headless`] instead.
    pub fn try_new() -> Result<Self, AudioError> {
        match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => Ok(Self {
                mode: BackendMode::Device {
                    sink,
                    voices: HashMap::new(),
                    next_index: 0,
                    live: 0,
                    listener: ([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
                },
            }),
            Err(rodio::DeviceSinkError::NoDevice) => Err(AudioError::NoDevice),
            Err(other) => Err(AudioError::StreamFailed {
                message: other.to_string(),
            }),
        }
    }

    /// Whether this backend is the decode-only simulation (no device).
    ///
    /// Exists so tests and diagnostics can assert which posture a
    /// backend was built with — gameplay never branches on it (both
    /// modes honor the same trait contract).
    pub fn is_headless(&self) -> bool {
        matches!(self.mode, BackendMode::Headless { .. })
    }

    /// The stored listener pose as `(position, forward)`.
    ///
    /// The bootstrap applies Canary-computed gains instead of
    /// spatializing, so this round-trips what
    /// [`set_listener`](crate::AudioBackend::set_listener) stored —
    /// the observable proving the call reached the backend, and the
    /// pose a future spatializing backend will consume.
    pub fn listener_pose(&self) -> ([f32; 3], [f32; 3]) {
        match &self.mode {
            BackendMode::Headless { listener, .. } => *listener,
            BackendMode::Device { listener, .. } => *listener,
        }
    }

    /// Mints the next slot index, or `None` if four billion voices
    /// have been played on one backend lifetime (the documented
    /// exhaustion bound of the never-recycle discipline).
    fn next_index(slots_next: &mut u32) -> Option<u32> {
        let index = *slots_next;
        *slots_next = slots_next.checked_add(1)?;
        Some(index)
    }
}

impl Default for RodioBackend {
    /// Same as [`RodioBackend::headless`]: the default backend needs
    /// no device, so systems that ensure a backend resource (see
    /// [`crate::audio_trigger_system`]) never depend on hardware to
    /// register.
    fn default() -> Self {
        Self::headless()
    }
}

impl AudioBackend for RodioBackend {
    fn play_sound(&mut self, sound: &Sound, looping: bool) -> Result<SourceHandle, AudioError> {
        // `Sound`'s documented invariants (nonzero rate, one or two
        // channels) are the precondition — only the asset loaders can
        // build a `Sound`, and they enforce all three — so a violation
        // here is a violated invariant, not a runtime failure, and
        // `expect` with the invariant named is the honest shape.
        let channels =
            NonZero::new(sound.channel_count()).expect("Sound guarantees one or two channels");
        let rate = NonZero::new(sound.sample_rate()).expect("Sound guarantees a nonzero rate");

        match &mut self.mode {
            BackendMode::Headless {
                slots,
                next_index,
                live,
                ..
            } => {
                let Some(index) = Self::next_index(next_index) else {
                    // Four billion voices on one backend: refuse the
                    // play as a lost device rather than wrapping the
                    // index into aliasing. Unreachable in practice;
                    // typed, never a panic.
                    return Err(AudioError::StreamFailed {
                        message: "voice index space exhausted".to_owned(),
                    });
                };
                slots.insert(
                    index,
                    HeadlessSlot {
                        generation: 0,
                        alive: true,
                        paused: false,
                        volume: 1.0,
                    },
                );
                *live += 1;
                Ok(SourceHandle::from_raw_parts(index, 0))
            }
            BackendMode::Device {
                sink,
                voices,
                next_index,
                live,
                ..
            } => {
                let buffer = SamplesBuffer::new(channels, rate, sound.samples().to_vec());
                let player = rodio::Player::connect_new(sink.mixer());
                if looping {
                    player.append(buffer.repeat_infinite());
                } else {
                    player.append(buffer);
                }
                let Some(index) = Self::next_index(next_index) else {
                    return Err(AudioError::StreamFailed {
                        message: "voice index space exhausted".to_owned(),
                    });
                };
                voices.insert(
                    index,
                    DeviceVoice {
                        generation: 0,
                        alive: true,
                        player,
                    },
                );
                *live += 1;
                Ok(SourceHandle::from_raw_parts(index, 0))
            }
        }
    }

    fn stop(&mut self, source: SourceHandle) -> bool {
        match &mut self.mode {
            BackendMode::Headless { slots, live, .. } => match slots.get_mut(&source.index()) {
                Some(slot) if slot.alive && slot.generation == source.generation() => {
                    slot.alive = false;
                    slot.generation = slot.generation.wrapping_add(1);
                    *live -= 1;
                    true
                }
                _ => false,
            },
            BackendMode::Device { voices, live, .. } => match voices.get_mut(&source.index()) {
                Some(voice) if voice.alive && voice.generation == source.generation() => {
                    voice.alive = false;
                    voice.generation = voice.generation.wrapping_add(1);
                    *live -= 1;
                    // Dropping the player next (via removal) stops its
                    // sounds; the flag flip above is what makes the
                    // handle stale first, so no interleaving can
                    // observe a live handle to a dead voice.
                    voices.remove(&source.index());
                    true
                }
                _ => false,
            },
        }
    }

    fn pause(&mut self, source: SourceHandle) -> bool {
        match &mut self.mode {
            BackendMode::Headless { slots, .. } => match slots.get_mut(&source.index()) {
                Some(slot) if slot.alive && slot.generation == source.generation() => {
                    slot.paused = true;
                    true
                }
                _ => false,
            },
            BackendMode::Device { voices, .. } => match voices.get(&source.index()) {
                Some(voice) if voice.alive && voice.generation == source.generation() => {
                    voice.player.pause();
                    true
                }
                _ => false,
            },
        }
    }

    fn resume(&mut self, source: SourceHandle) -> bool {
        match &mut self.mode {
            BackendMode::Headless { slots, .. } => match slots.get_mut(&source.index()) {
                Some(slot) if slot.alive && slot.generation == source.generation() => {
                    slot.paused = false;
                    true
                }
                _ => false,
            },
            BackendMode::Device { voices, .. } => match voices.get(&source.index()) {
                Some(voice) if voice.alive && voice.generation == source.generation() => {
                    voice.player.play();
                    true
                }
                _ => false,
            },
        }
    }

    fn set_volume(&mut self, source: SourceHandle, volume: f32) -> Result<(), AudioError> {
        if !volume.is_finite() || volume < 0.0 {
            return Err(AudioError::InvalidVolume { volume });
        }
        match &mut self.mode {
            BackendMode::Headless { slots, .. } => match slots.get_mut(&source.index()) {
                Some(slot) if slot.alive && slot.generation == source.generation() => {
                    slot.volume = volume;
                    Ok(())
                }
                _ => Err(AudioError::unknown_source(source)),
            },
            BackendMode::Device { voices, .. } => match voices.get(&source.index()) {
                Some(voice) if voice.alive && voice.generation == source.generation() => {
                    voice.player.set_volume(volume);
                    Ok(())
                }
                _ => Err(AudioError::unknown_source(source)),
            },
        }
    }

    fn set_listener(&mut self, position: [f32; 3], forward: [f32; 3]) {
        match &mut self.mode {
            BackendMode::Headless { listener, .. } => *listener = (position, forward),
            BackendMode::Device { listener, .. } => *listener = (position, forward),
        }
    }

    fn pump(&mut self) -> Result<(), AudioError> {
        // Documented no-op in this cut: device health is a
        // construction-time property (see `try_new`), voices free only
        // on stop (no auto-finish polling — finish events arrive with
        // the streaming milestone), and the simulation clock needs no
        // servicing. The fallible shape stays for backends that report
        // mid-run device loss here.
        Ok(())
    }

    fn source_count(&self) -> usize {
        match &self.mode {
            BackendMode::Headless { live, .. } => *live,
            BackendMode::Device { live, .. } => *live,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use canary_assets::{load_sound, AssetStore};
    use canary_ecs::World;
    use canary_scheduler::Schedule;

    use crate::{
        audio_trigger_system, register_audio_trigger, AudioConfig, AudioSource, AudioVoices,
        SourceState,
    };

    /// rustdoc-style guarantee, stated as a test: the backend travels
    /// behind a `Mutex` world resource, which needs `Send`. If a
    /// future rodio/cpal version ever drops `Send` on the device sink,
    /// this fails at compile time — loudly, at the seam — instead of
    /// surfacing as an inscrutable resource-bound error.
    #[test]
    fn backend_is_send_for_the_mutex_resource() {
        fn assert_send<T: Send>() {}
        assert_send::<RodioBackend>();
    }

    /// The default backend is headless: registering a system around a
    /// default-constructed backend never needs hardware.
    #[test]
    fn default_is_headless() {
        assert!(RodioBackend::default().is_headless());
        assert!(RodioBackend::headless().is_headless());
    }

    /// Headless honors the same observable contract as the stub:
    /// play → pause → resume → volume → stop, stale handles stay
    /// stale, bad gains fail typed. (The stub is the executable form
    /// of "honor"; this is the second implementation proving it.)
    #[test]
    fn headless_honors_stale_and_invalid_contracts() {
        let sound = load_sound(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../canary-assets/tests/fixtures/tone-mono-8k.wav"),
        )
        .expect("WAV fixture must load");
        let mut backend = RodioBackend::headless();

        let handle = backend
            .play_sound(&sound, true)
            .expect("valid play must succeed");
        assert_eq!(backend.source_count(), 1);
        backend
            .set_volume(handle, 0.25)
            .expect("valid gain must apply");
        assert!(backend.pause(handle));
        assert!(backend.resume(handle));

        let forged = SourceHandle::from_raw_parts(999, 0);
        assert!(!backend.stop(forged));
        assert!(!backend.pause(forged));
        assert!(!backend.resume(forged));
        assert_eq!(
            backend.set_volume(forged, 0.5),
            Err(AudioError::UnknownSource {
                index: 999,
                generation: 0,
            })
        );
        for volume in [f32::NAN, f32::INFINITY, -1.0] {
            // NaN never equals itself: assert the variant for NaN,
            // exact equality (value included) for the rest.
            if volume.is_nan() {
                assert!(
                    matches!(
                        backend.set_volume(handle, volume),
                        Err(AudioError::InvalidVolume { .. })
                    ),
                    "NaN gain must fail typed"
                );
            } else {
                assert_eq!(
                    backend.set_volume(handle, volume),
                    Err(AudioError::InvalidVolume { volume }),
                    "gain {volume} must fail typed"
                );
            }
        }

        assert!(backend.stop(handle));
        assert!(!backend.stop(handle));
        assert_eq!(
            backend.set_volume(handle, 0.5),
            Err(AudioError::unknown_source(handle)),
            "stopped handles stay stale"
        );
        assert_eq!(backend.source_count(), 0);
        assert!(backend.pump().is_ok());
    }

    /// The listener pose round-trips: the stored pose is what the
    /// last `set_listener` call stored.
    #[test]
    fn listener_pose_round_trips() {
        let mut backend = RodioBackend::headless();
        assert_eq!(backend.listener_pose(), ([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]));

        backend.set_listener([1.0, 2.0, 3.0], [0.0, 0.0, 1.0]);
        assert_eq!(backend.listener_pose(), ([1.0, 2.0, 3.0], [0.0, 0.0, 1.0]));
    }

    /// Opening a device degrades typed, never panics: on hardware it
    /// succeeds empty and pumps clean; headless it reports `NoDevice`
    /// (or a `StreamFailed` cause) instead of trapping. Either arm is
    /// a pass — what must never happen is a panic.
    #[test]
    fn device_open_degrades_typed_never_panics() {
        match RodioBackend::try_new() {
            Ok(backend) => {
                assert!(!backend.is_headless());
                assert_eq!(backend.source_count(), 0);
                assert!(
                    backend.listener_pose().0 == [0.0, 0.0, 0.0],
                    "a fresh device backend stores the default pose"
                );
            }
            Err(AudioError::NoDevice) => {}
            Err(AudioError::StreamFailed { .. }) => {}
            Err(other) => panic!("try_new must only fail as NoDevice/StreamFailed, got {other:?}"),
        }
    }

    /// Decode-only WAV proof: rodio's MIT/Apache decoder (hound, not
    /// Symphonia) feeds the fixture bytes with no device anywhere in
    /// the path — no `DeviceSinkBuilder`, no stream, no hardware.
    #[test]
    fn decode_only_wav_feeds_bytes_without_a_device() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/tone-mono-8k.wav");
        let bytes = std::fs::read(&path).expect("fixture bytes must read");
        let reference = load_sound(&path).expect("the asset loader must decode the same fixture");

        let decoder = rodio::Decoder::new_wav(std::io::Cursor::new(bytes))
            .expect("rodio must decode the WAV fixture without a device");
        let decoded: Vec<f32> = decoder.collect();

        assert_eq!(
            decoded.len(),
            reference.samples().len(),
            "rodio must feed exactly the loader's sample count"
        );
        assert!(
            decoded.iter().all(|sample| sample.is_finite()),
            "decoded samples must all be finite"
        );
        for (got, want) in decoded.iter().zip(reference.samples()) {
            assert!(
                (got - want).abs() < 0.05,
                "decoded value {got} must roughly match the loader's {want}"
            );
        }
    }

    /// Decode-only Vorbis proof: same shape as the WAV proof, through
    /// lewton instead of hound.
    #[test]
    fn decode_only_vorbis_feeds_bytes_without_a_device() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../canary-assets/tests/fixtures/tone-stereo-8k.ogg");
        let bytes = std::fs::read(&path).expect("fixture bytes must read");
        let reference = load_sound(&path).expect("the asset loader must decode the same fixture");

        let decoder = rodio::Decoder::new_vorbis(std::io::Cursor::new(bytes))
            .expect("rodio must decode the Vorbis fixture without a device");
        let decoded: Vec<f32> = decoder.collect();

        assert_eq!(
            decoded.len(),
            reference.samples().len(),
            "rodio must feed exactly the loader's sample count"
        );
        assert!(
            decoded.iter().all(|sample| sample.is_finite()),
            "decoded samples must all be finite"
        );
        assert!(
            decoded.iter().all(|sample| sample.abs() <= 1.0),
            "decoded samples must stay in PCM range"
        );
    }

    /// The headless full trigger path: the real system driving the
    /// real (headless) backend opens no device and still actuates
    /// play on transition and stop on transition — the CI proof that
    /// the scene works where no sound card exists.
    #[test]
    fn headless_full_trigger_path_without_a_device() {
        let sound = load_sound(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../canary-assets/tests/fixtures/tone-mono-8k.wav"),
        )
        .expect("WAV fixture must load");
        let mut store = AssetStore::new();
        let handle = store.insert(sound);

        let mut world = World::new();
        world.insert_resource(store);
        world.insert_resource(AudioConfig::default());
        world.insert_resource(Mutex::new(RodioBackend::headless()));
        world.insert_resource(AudioVoices::new());
        let entity = world.spawn();
        world
            .insert(entity, AudioSource::new(handle))
            .expect("source insert must succeed");

        let mut schedule = Schedule::new();
        register_audio_trigger::<RodioBackend>(&mut schedule);

        schedule.run(&mut world);
        assert_eq!(
            world
                .resource::<Mutex<RodioBackend>>()
                .expect("backend must exist")
                .lock()
                .expect("test mutex must not be poisoned")
                .source_count(),
            0,
            "stopped sources own no voices"
        );

        world
            .get_mut::<AudioSource>(entity)
            .expect("source must exist")
            .state = SourceState::Playing;
        schedule.run(&mut world);
        {
            let backend = world
                .resource::<Mutex<RodioBackend>>()
                .expect("backend must exist")
                .lock()
                .expect("test mutex must not be poisoned");
            assert_eq!(backend.source_count(), 1);
            assert!(backend.is_headless(), "no device was opened at any point");
        }
        assert_eq!(
            world
                .resource::<AudioVoices>()
                .expect("table must exist")
                .voice_count(),
            1
        );

        world
            .get_mut::<AudioSource>(entity)
            .expect("source must exist")
            .state = SourceState::Stopped;
        schedule.run(&mut world);
        assert_eq!(
            world
                .resource::<Mutex<RodioBackend>>()
                .expect("backend must exist")
                .lock()
                .expect("test mutex must not be poisoned")
                .source_count(),
            0,
            "stop frees the voice with no orphans"
        );

        // The system also works through the generic entry point the
        // same way the stub proof uses it.
        audio_trigger_system::<RodioBackend>(&mut world);
    }
}
