// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Game-facing audio components plus the [`AudioConfig`] resource.
//!
//! The trigger system (see [`crate::audio_trigger_system`]) reads these
//! and drives the backend; gameplay writes them. Ownership is split on
//! purpose: the game owns *intent* ([`AudioSource::state`]), the system
//! owns *voices* (backend handles live in the system's
//! [`AudioVoices`](crate::AudioVoices) table, never in components), so
//! no field is ever written from both sides in one tick —
//! `docs/architecture/execution-model.md`'s Ownership invariant, applied
//! rather than cited.

use canary_assets::{AssetHandle, Sound};

/// What the game wants a source to be doing: the trigger the system
/// actuates on.
///
/// The game writes this field; the trigger system reads it every tick
/// and reconciles backend voices with it (play on the stopped→playing
/// edge, pause/resume on the playing↔paused edges, stop on the
/// →stopped edge and on removal/despawn). The system never writes it
/// back — there are no finish events in this cut (one-shot completion
/// reporting arrives with the streaming milestone, not here), so the
/// field the game set is still the truth next tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceState {
    /// No voice (or a stopped voice): the system frees any tracked
    /// voice for this source. The initial state of every source.
    Stopped,
    /// A voice must be playing: the system plays (or resumes) the
    /// source's sound and refreshes its gain every tick.
    Playing,
    /// A voice must be paused: the system pauses the tracked voice,
    /// keeping its slot so resume continues where it left off.
    Paused,
}

/// A game-state-driven sound: which asset, what intent, how loud,
/// looping or one-shot.
///
/// Attach alongside a [`Transform`](canary_transform::Transform) for
/// positional playback (the system attenuates by the source–listener
/// distance) or standalone for a non-positional voice (full gain, no
/// attenuation). The asset handle resolves against the world's
/// [`AssetStore<Sound>`](canary_assets::AssetStore) each tick a voice
/// is needed — a handle that is not (yet) in the store simply yields
/// no voice until it is, which is the streaming-friendly retry the
/// trigger system documents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioSource {
    /// The sound to play, resolved against the world's
    /// [`AssetStore<Sound>`](canary_assets::AssetStore).
    pub sound: AssetHandle<Sound>,
    /// The game-owned playback intent; see [`SourceState`].
    pub state: SourceState,
    /// The source's linear gain (`0.0` silent, `1.0` unity), multiplied
    /// by the master's and the distance attenuation every tick.
    ///
    /// Expected in `0.0..=` (above-unity amplification is allowed and
    /// passed through); non-finite or negative values are refused at
    /// the backend boundary with a typed error and leave the voice's
    /// last good gain in place — a NaN from gameplay math must never
    /// reach the mixer, and never fails the tick.
    pub volume: f32,
    /// Whether the voice loops. Fixed at play time: changing this
    /// mid-play takes effect on the next play, never retroactively.
    pub looping: bool,
}

impl AudioSource {
    /// A stopped, unity-gain, one-shot source for `sound`.
    ///
    /// Stopped is the only honest initial state: construction must not
    /// imply playback before the trigger system has seen the entity.
    pub fn new(sound: AssetHandle<Sound>) -> Self {
        Self {
            sound,
            state: SourceState::Stopped,
            volume: 1.0,
            looping: false,
        }
    }

    /// Sets the source gain, returning the updated source for
    /// builder-style construction.
    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }

    /// Sets the loop flag, returning the updated source for
    /// builder-style construction.
    pub fn with_looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }
}

/// The ears of the scene: the pose attenuation is measured from.
///
/// A unit struct on purpose — the pose itself comes from the entity's
/// [`Transform`](canary_transform::Transform) (position) and rotation
/// (forward), so the component is only the marker that elects *which*
/// entity listens. The scene's first listener wins; a scene with no
/// listener attenuates from the default pose (origin, facing −Z — see
/// [`AudioListener::default_pose`]), so headless scenes without a
/// listener entity still behave deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AudioListener;

impl AudioListener {
    /// The pose used when no listener entity exists: origin, facing −Z
    /// (the glam/camera forward convention), returned as
    /// `(position, forward)`.
    ///
    /// Kept next to the type so the fallback the trigger system
    /// applies cannot drift from the documented one.
    pub fn default_pose() -> ([f32; 3], [f32; 3]) {
        ([0.0, 0.0, 0.0], [0.0, 0.0, -1.0])
    }
}

/// Which backend implementation plays the scene's voices.
///
/// Only [`AudioBackendName::Rodio`] exists today, for the same reason
/// physics' backend enum has one variant: the custom engine arrives
/// post-`v0.1.0` as a new variant plus a backend behind the unchanged
/// [`crate::AudioBackend`] trait. `#[non_exhaustive]` forces downstream
/// `match`es through a wildcard arm today, so adding that variant
/// cannot break exhaustive matches outside this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AudioBackendName {
    /// The private rodio bootstrap backend (ADR 0023).
    Rodio,
}

impl AudioBackendName {
    /// The config-file spelling (`backend = "rodio"`): kept next to
    /// the type so serialization and docs cannot drift apart when the
    /// custom-engine variant arrives.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rodio => "rodio",
        }
    }
}

/// Global audio configuration, stored as a `World` resource.
///
/// A plain `Send + Sync + 'static` struct — the only three things
/// `canary-ecs` resources require — holding the master gain plus the
/// ADR 0023 backend selection. Systems read it via
/// `World::resource::<AudioConfig>()`; there is at most one per
/// `World`, matching the "one mix for the game" reality. The master
/// gain lives here (not per-scene, not per-backend-constructor) so
/// tuning it is data — set once at startup, hot-tweakable later —
/// never a backend reconstruction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioConfig {
    /// The master linear gain multiplied into every voice
    /// (`0.0` mutes the mix, `1.0` unity). Expected finite and
    /// non-negative; a bad value degrades per-voice to "keep last
    /// good gain" at the backend boundary rather than failing ticks.
    pub master_volume: f32,
    /// Which backend implementation to use. Only rodio exists yet; see
    /// [`AudioBackendName`].
    pub backend: AudioBackendName,
}

impl AudioConfig {
    /// Builds an explicit config. Prefer [`AudioConfig::default`]
    /// unless a field genuinely differs — the default is the
    /// documented v0.0.12 mix (unity gain, rodio bootstrap).
    pub fn new(master_volume: f32, backend: AudioBackendName) -> Self {
        Self {
            master_volume,
            backend,
        }
    }
}

impl Default for AudioConfig {
    /// Unity master gain, rodio backend.
    fn default() -> Self {
        Self {
            master_volume: 1.0,
            backend: AudioBackendName::Rodio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_constructor_is_stopped_unity_gain_one_shot() {
        let handle = AssetHandle::<Sound>::from_raw_parts(2, 0);
        let source = AudioSource::new(handle);

        assert_eq!(source.sound, handle);
        assert_eq!(source.state, SourceState::Stopped);
        assert_eq!(source.volume, 1.0);
        assert!(!source.looping);
    }

    #[test]
    fn source_builders_set_gain_and_loop_flag() {
        let handle = AssetHandle::<Sound>::from_raw_parts(0, 0);
        let source = AudioSource::new(handle)
            .with_volume(0.25)
            .with_looping(true);

        assert_eq!(source.volume, 0.25);
        assert!(source.looping);
        assert_eq!(source.state, SourceState::Stopped);
    }

    #[test]
    fn listener_default_pose_is_origin_facing_minus_z() {
        assert_eq!(
            AudioListener::default_pose(),
            ([0.0, 0.0, 0.0], [0.0, 0.0, -1.0])
        );
        let _listener = AudioListener;
    }

    #[test]
    fn backend_name_spelling_is_rodio() {
        assert_eq!(AudioBackendName::Rodio.as_str(), "rodio");
    }

    #[test]
    fn config_default_is_unity_gain_rodio() {
        let config = AudioConfig::default();
        assert_eq!(config.master_volume, 1.0);
        assert_eq!(config.backend, AudioBackendName::Rodio);
    }
}
