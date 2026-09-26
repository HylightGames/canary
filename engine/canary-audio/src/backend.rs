// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! The object-safe, leak-free [`AudioBackend`] seam.
//!
//! Engine and gameplay code program against [`AudioBackend`]; concrete
//! players (the private rodio bootstrap in Task order, the custom engine
//! post-`v0.1.0`, or a Tier B FMOD/Wwise binding — `docs/architecture/audio.md`
//! makes the trait a public seam, not a first-party menu) sit behind it.
//! "Leak-free" means no third-party type appears in any public signature
//! here: sounds cross as [`canary_assets::Sound`] plus plain data, voices
//! cross as crate-owned [`SourceHandle`] keys, and whatever the player
//! requires internally (rodio `Player`s, cpal streams, symphonia-free
//! decoders) stays private. `cargo doc` plus a grep for
//! `rodio`/`cpal`/`symphonia` in public signatures re-verifies that on
//! every change.

use crate::AudioError;
use canary_assets::Sound;

/// An opaque, generational key for a voice owned by an [`AudioBackend`].
///
/// The shape mirrors [`canary_ecs::Entity`](https://github.com/HylightGames/canary/blob/dev/engine/canary-ecs/src/entity.rs)
/// (`index` + `generation`) deliberately: bare indices alias after
/// stop-plus-replay (slot 2 freed, then handed to a new voice, makes an
/// old handle to slot 2 drive the wrong voice), and that aliasing class
/// is exactly what `Entity`'s generation field already solved for this
/// workspace. Reusing the proven shape means the handle/store invariant
/// reasoning transfers wholesale.
///
/// Handles are plain keys; the backend owns everything. Equality and
/// hashing consider `index` and `generation` only. Forging a handle via
/// [`SourceHandle::from_raw_parts`] is safe by construction: every
/// method checks liveness itself and reports stale handles as
/// `false`/`Err`, never as another voice's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceHandle {
    index: u32,
    generation: u64,
}

impl SourceHandle {
    /// The slot index this handle points at. Never a stable identifier
    /// on its own: slots are recycled and only
    /// [`SourceHandle::generation`] disambiguates reuses. Exposed for
    /// debugging, diagnostics, and boundaries that serialize handles.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The generation of the slot this handle was issued for. A handle
    /// is live only while the backend's slot still carries this exact
    /// generation; any mismatch (stopped, or recycled for a new voice)
    /// makes the handle stale, and stale handles resolve to
    /// `false`/[`AudioError::UnknownSource`], never to another voice's
    /// state.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Reconstructs a handle from raw `index` then `generation` parts,
    /// matching [`SourceHandle::index`]/[`SourceHandle::generation`]'s
    /// order.
    ///
    /// Exists for boundaries that cannot pass an opaque handle through
    /// directly (serialization, tests) and must rebuild one from bits.
    /// Like `Entity::from_raw_parts`, this verifies nothing: feeding the
    /// result to any [`AudioBackend`] method is exactly as safe as
    /// passing a genuinely stale handle, because the backend checks
    /// liveness itself. This is also the constructor external
    /// (out-of-crate) backends use to mint handles from their own slot
    /// bookkeeping.
    pub fn from_raw_parts(index: u32, generation: u64) -> Self {
        Self { index, generation }
    }
}

/// The playback seam: what engine and game code may ask of ANY audio
/// backend, with no backend crate's types visible.
///
/// # Object safety (why these exact shapes)
///
/// Backends are user-swappable (`Box<dyn AudioBackend>` behind
/// configuration, per ADR 0023), so every method here must be
/// dispatchable through a vtable: no generic type parameters, no
/// `Self`-returning constructors, no associated types. Adding a method
/// later is additive for the trait but breaking for external
/// implementors; that is the known cost of a public seam, and the reason
/// this cut stays minimal (every method here has a v0.0.12 consumer:
/// play/stop/pause/resume/set-volume on source handles, listener pose
/// update, per-tick pump).
///
/// # Leak freedom (why Canary types plus primitives only)
///
/// rodio speaks its own `Player`/`Source` vocabulary and publicly
/// re-exports cpal; a future custom engine or Tier B binding will speak
/// another. If either appeared in these signatures, every game crate
/// would transitively depend on that player's vocabulary and swapping
/// backends would move game-facing API — the exact future ADR 0023
/// forbids. So: sounds cross as [`canary_assets::Sound`] (the asset
/// pipeline's own playback-agnostic PCM), poses as `[f32; 3]` arrays,
/// gains as `f32`, and voices as crate-owned [`SourceHandle`] opaques.
/// Bus/mixer/spatialization concepts stay Canary types for the same
/// reason: a later backend must not fight rodio's `Sink`/player
/// assumptions.
///
/// # Stale handles are signals, not errors (why `bool` next to `Result`)
///
/// Despawn racing a tick is normal ECS life, not a caller bug, so the
/// lifecycle methods report liveness the way `World` does — "missing is
/// `false`, not a distinct error case": [`AudioBackend::stop`],
/// [`AudioBackend::pause`], and [`AudioBackend::resume`] return `false`
/// for stale handles and the trigger system skips. Creation-adjacent
/// calls keep `Result` because bad input there IS worth naming loudly:
/// [`AudioBackend::play_sound`] fails typed on backend-internal
/// failures, and [`AudioBackend::set_volume`] fails typed on unknown
/// handles ([`AudioError::UnknownSource`], always a caller ordering
/// bug) and on non-finite or negative gains
/// ([`AudioError::InvalidVolume`], a NaN that must never reach the
/// mixer).
///
/// # `Send` (why the supertrait)
///
/// The backend lives in the `World` as a `Mutex<B: AudioBackend>`
/// resource (see [`crate::audio_trigger_system`]), and ECS resources
/// require `Send + Sync`. `Mutex` supplies `Sync` from `Send`, so the
/// trait demands `Send` and every implementation proves it — pinned by
/// the `backend_is_send_where_claimed`-style assertions in each
/// backend's own tests.
pub trait AudioBackend: Send {
    /// Decodes `sound` into a backend-owned voice slot and starts
    /// playback, returning the opaque handle all later calls use.
    ///
    /// `sound` arrives by reference (not by value) even though most
    /// backends copy the samples in: if a later backend streams from
    /// the store instead of copying, no game-facing signature moves.
    /// `looping` fixes the voice's loop mode at play time — changing
    /// the component's loop flag mid-play takes effect on the next
    /// play, never retroactively.
    ///
    /// [`Sound`]'s documented invariants (nonzero
    /// rate, one or two channels, nonzero frames) are the precondition:
    /// only the asset loaders can build a `Sound`, and they enforce
    /// all three, so a backend may treat violations as impossible
    /// rather than as typed errors.
    fn play_sound(&mut self, sound: &Sound, looping: bool) -> Result<SourceHandle, AudioError>;

    /// Stops playback and frees the voice slot. Returns whether a live
    /// voice was stopped: `false` for a stale or unknown handle.
    ///
    /// `false`-on-stale is load-bearing, not lenient: despawn racing a
    /// tick means double-stop is routine ECS life, and failing it would
    /// turn every teardown race into a spurious error. Stopping bumps
    /// the slot's generation, so pre-stop handles stay stale forever
    /// and can never alias a later voice recycled into the slot.
    fn stop(&mut self, source: SourceHandle) -> bool;

    /// Pauses a live voice, keeping its slot (resume continues where it
    /// left off). Returns `false` (pausing nothing) for a stale
    /// handle, for the same skip-semantics reason as
    /// [`AudioBackend::stop`]. Idempotent: pausing a paused voice
    /// reports `true`.
    fn pause(&mut self, source: SourceHandle) -> bool;

    /// Resumes a paused voice. Returns `false` (resuming nothing) for
    /// a stale handle. Idempotent: resuming a playing voice reports
    /// `true`.
    fn resume(&mut self, source: SourceHandle) -> bool;

    /// Sets a voice's linear gain (`0.0` silent, `1.0` unity;
    /// above-unity amplification is allowed and left to the backend).
    ///
    /// Fails with [`AudioError::UnknownSource`] for a stale handle
    /// (caller ordering bug — contrast [`AudioBackend::stop`]) and
    /// with [`AudioError::InvalidVolume`] for non-finite or negative
    /// gains, applying nothing in both cases. Distance attenuation is
    /// computed Canary-side by the trigger system (never by a backend
    /// spatial player); this method applies the final product.
    fn set_volume(&mut self, source: SourceHandle, volume: f32) -> Result<(), AudioError>;

    /// Updates the listener pose games attenuate against: world-space
    /// `position` plus a `forward` direction (both need not be
    /// normalized; backends normalize or ignore per their own math).
    ///
    /// Infallible and total by design: listener updates ride every
    /// tick, and a pose write must never be the thing that fails a
    /// tick. Backends that cannot spatialize (this cut's rodio
    /// bootstrap, which applies Canary-computed gains instead) store
    /// the pose for future backends and round-trip it for tests.
    fn set_listener(&mut self, position: [f32; 3], forward: [f32; 3]);

    /// Services the backend once per tick: advances simulation clocks,
    /// reaps whatever the implementation must reap, surfaces device
    /// health.
    ///
    /// The trigger system calls this every tick and deliberately does
    /// NOT propagate failures (a failed pump must not fail the game
    /// tick — gameplay survives sound failure, not the reverse). The
    /// `Result` exists because creation-time health is not the whole
    /// story for every backend: a future device backend reports
    /// mid-run device loss here, and the seam must already have room
    /// for it.
    fn pump(&mut self) -> Result<(), AudioError>;

    /// How many live voices the backend currently owns. Exists for
    /// tests, debug overlays, and the trigger system's own
    /// no-orphans invariant — not for gameplay logic, which reasons
    /// about entities, never backend internals.
    fn source_count(&self) -> usize;
}

/// The backend call journal: every voice-lifecycle call a test backend
/// observed, in order.
///
/// Asserting on this journal (rather than on audible output — CI has no
/// ears) is what makes the game proof a proof: a scripted game-state
/// change must produce exactly the expected call sequence, no more.
#[derive(Debug, Clone, PartialEq)]
pub enum StubEvent {
    /// [`AudioBackend::play_sound`] issued a voice.
    Play {
        /// The handle the backend minted for the new voice.
        handle: SourceHandle,
        /// The loop mode the play request carried.
        looping: bool,
    },
    /// [`AudioBackend::stop`] freed a live voice.
    Stop {
        /// The stopped voice.
        handle: SourceHandle,
    },
    /// [`AudioBackend::pause`] paused a live voice.
    Pause {
        /// The paused voice.
        handle: SourceHandle,
    },
    /// [`AudioBackend::resume`] resumed a live voice.
    Resume {
        /// The resumed voice.
        handle: SourceHandle,
    },
    /// [`AudioBackend::set_volume`] applied a gain to a live voice
    /// (validation failures apply nothing and journal nothing).
    SetVolume {
        /// The voice the gain was applied to.
        handle: SourceHandle,
        /// The applied linear gain.
        volume: f32,
    },
    /// [`AudioBackend::set_listener`] stored a listener pose.
    SetListener {
        /// The stored world-space position.
        position: [f32; 3],
        /// The stored forward direction.
        forward: [f32; 3],
    },
    /// [`AudioBackend::pump`] ran.
    Pump,
}

/// Minimal in-memory [`AudioBackend`] proving the trait's contracts
/// without any player: a generational slot map plus a call journal.
///
/// The same type serves two masters: the trait-contract tests below
/// (stale-handle behavior, typed errors, generation recycling) and the
/// trigger-system game proof (exact call-order assertions). The journal
/// records only *applied* calls — rejected inputs (`UnknownSource`,
/// `InvalidVolume`) change nothing observable except their `Err`, which
/// is exactly the backend-owns-validation contract the rodio backend
/// must also honor.
#[cfg(test)]
pub(crate) struct StubBackend {
    slots: std::collections::HashMap<u32, StubSlot>,
    next_index: u32,
    live: usize,
    listener: ([f32; 3], [f32; 3]),
    /// Every applied call, in order. Tests assert exact sequences.
    pub(crate) journal: Vec<StubEvent>,
    /// When set, [`AudioBackend::pump`] fails with [`AudioError::NoDevice`],
    /// proving the trigger system survives pump failure without failing
    /// the tick.
    pub(crate) fail_pump: bool,
}

#[cfg(test)]
struct StubSlot {
    generation: u64,
    alive: bool,
    paused: bool,
    volume: f32,
}

#[cfg(test)]
impl StubBackend {
    pub(crate) fn new() -> Self {
        Self {
            slots: std::collections::HashMap::new(),
            next_index: 0,
            live: 0,
            listener: ([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            journal: Vec::new(),
            fail_pump: false,
        }
    }

    /// The last successfully applied gain for a live voice, or `None`
    /// for a stale handle. Exists so volume tests assert *applied*
    /// state, not journal shape, where that reads better.
    pub(crate) fn applied_volume(&self, handle: SourceHandle) -> Option<f32> {
        self.slots
            .get(&handle.index())
            .filter(|slot| slot.alive && slot.generation == handle.generation())
            .map(|slot| slot.volume)
    }

    fn live_slot_mut(&mut self, handle: SourceHandle) -> Option<&mut StubSlot> {
        self.slots
            .get_mut(&handle.index())
            .filter(|slot| slot.alive && slot.generation == handle.generation())
    }
}

#[cfg(test)]
impl Default for StubBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl AudioBackend for StubBackend {
    fn play_sound(&mut self, _sound: &Sound, looping: bool) -> Result<SourceHandle, AudioError> {
        let index = self.next_index;
        self.next_index = self
            .next_index
            .checked_add(1)
            .expect("test stub ran out of voice indices");
        self.slots.insert(
            index,
            StubSlot {
                generation: 0,
                alive: true,
                paused: false,
                volume: 1.0,
            },
        );
        self.live += 1;
        let handle = SourceHandle::from_raw_parts(index, 0);
        self.journal.push(StubEvent::Play { handle, looping });
        Ok(handle)
    }

    fn stop(&mut self, source: SourceHandle) -> bool {
        match self.slots.get_mut(&source.index()) {
            Some(slot) if slot.alive && slot.generation == source.generation() => {
                slot.alive = false;
                slot.generation = slot.generation.wrapping_add(1);
                self.live -= 1;
                self.journal.push(StubEvent::Stop { handle: source });
                true
            }
            _ => false,
        }
    }

    fn pause(&mut self, source: SourceHandle) -> bool {
        match self.live_slot_mut(source) {
            Some(slot) => {
                slot.paused = true;
                self.journal.push(StubEvent::Pause { handle: source });
                true
            }
            None => false,
        }
    }

    fn resume(&mut self, source: SourceHandle) -> bool {
        match self.live_slot_mut(source) {
            Some(slot) => {
                slot.paused = false;
                self.journal.push(StubEvent::Resume { handle: source });
                true
            }
            None => false,
        }
    }

    fn set_volume(&mut self, source: SourceHandle, volume: f32) -> Result<(), AudioError> {
        if !volume.is_finite() || volume < 0.0 {
            return Err(AudioError::InvalidVolume { volume });
        }
        match self.live_slot_mut(source) {
            Some(slot) => {
                slot.volume = volume;
                self.journal.push(StubEvent::SetVolume {
                    handle: source,
                    volume,
                });
                Ok(())
            }
            None => Err(AudioError::unknown_source(source)),
        }
    }

    fn set_listener(&mut self, position: [f32; 3], forward: [f32; 3]) {
        self.listener = (position, forward);
        self.journal
            .push(StubEvent::SetListener { position, forward });
    }

    fn pump(&mut self) -> Result<(), AudioError> {
        self.journal.push(StubEvent::Pump);
        if self.fail_pump {
            return Err(AudioError::NoDevice);
        }
        Ok(())
    }

    fn source_count(&self) -> usize {
        self.live
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time proof the trait is object-safe: if any method grew
    /// a generic parameter, returned `Self`, or added an associated
    /// type, this function (and the `Box` below) would stop compiling —
    /// which is exactly the tripwire user-swappable backends need.
    fn assert_dyn_compatible(_: &dyn AudioBackend) {}

    #[test]
    fn trait_is_object_safe_through_dyn() {
        let mut backend: Box<dyn AudioBackend> = Box::new(StubBackend::new());
        assert_dyn_compatible(&*backend);

        let store_sound = canary_assets::load_sound(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../canary-assets/tests/fixtures/tone-mono-8k.wav"),
        )
        .expect("WAV fixture must load for the dyn proof");
        let handle = backend
            .play_sound(&store_sound, false)
            .expect("valid play must succeed");
        backend
            .set_volume(handle, 0.5)
            .expect("valid volume must apply");
        assert!(backend.pause(handle));
        assert!(backend.resume(handle));
        assert!(backend.stop(handle));
        assert_eq!(backend.source_count(), 0);
    }

    #[test]
    fn stopped_handles_stay_stale_across_every_method() {
        let mut backend = StubBackend::new();
        let store_sound = canary_assets::load_sound(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../canary-assets/tests/fixtures/tone-mono-8k.wav"),
        )
        .expect("WAV fixture must load");
        let handle = backend
            .play_sound(&store_sound, false)
            .expect("valid play must succeed");
        assert!(backend.stop(handle));

        assert!(!backend.stop(handle));
        assert!(!backend.pause(handle));
        assert!(!backend.resume(handle));
        assert_eq!(
            backend.set_volume(handle, 0.5),
            Err(AudioError::UnknownSource {
                index: handle.index(),
                generation: handle.generation(),
            })
        );
        assert_eq!(backend.source_count(), 0);
    }

    #[test]
    fn forged_handles_never_resolve_to_live_voices() {
        let mut backend = StubBackend::new();
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
    }

    #[test]
    fn nonfinite_or_negative_volumes_apply_nothing_and_name_the_value() {
        let mut backend = StubBackend::new();
        let store_sound = canary_assets::load_sound(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../canary-assets/tests/fixtures/tone-mono-8k.wav"),
        )
        .expect("WAV fixture must load");
        let handle = backend
            .play_sound(&store_sound, false)
            .expect("valid play must succeed");

        for volume in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5] {
            // NaN never equals itself, so the NaN arm asserts the
            // variant (not the value); the rest assert exact equality
            // including the offending value.
            if volume.is_nan() {
                assert!(
                    matches!(
                        backend.set_volume(handle, volume),
                        Err(AudioError::InvalidVolume { .. })
                    ),
                    "NaN volume must fail typed"
                );
            } else {
                assert_eq!(
                    backend.set_volume(handle, volume),
                    Err(AudioError::InvalidVolume { volume }),
                    "volume {volume} must fail typed"
                );
            }
        }
        assert_eq!(
            backend.applied_volume(handle),
            Some(1.0),
            "rejected gains must leave the last good gain in place"
        );
        assert!(
            backend
                .journal
                .iter()
                .all(|event| !matches!(event, StubEvent::SetVolume { .. })),
            "rejected gains journal nothing"
        );
    }

    #[test]
    fn pause_and_resume_are_idempotent_on_live_voices() {
        let mut backend = StubBackend::new();
        let store_sound = canary_assets::load_sound(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../canary-assets/tests/fixtures/tone-mono-8k.wav"),
        )
        .expect("WAV fixture must load");
        let handle = backend
            .play_sound(&store_sound, false)
            .expect("valid play must succeed");

        assert!(backend.pause(handle));
        assert!(backend.pause(handle));
        assert!(backend.resume(handle));
        assert!(backend.resume(handle));
    }
}
