// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Typed audio failures: every fallible [`AudioBackend`](crate::AudioBackend)
//! operation names its failure, never a bare bool.
//!
//! The taxonomy mirrors `canary-physics`-style backend
//! errors deliberately: validation failures carry the offending value so a
//! failing spawn logs *which* volume or handle was rejected, while
//! despawn-race conditions stay out of this enum entirely —
//! [`AudioBackend`](crate::AudioBackend)'s stop/pause/resume report
//! liveness as `bool` (skip semantics), not as errors, because an entity
//! dying mid-tick is normal ECS life, not a caller bug.

use crate::SourceHandle;

/// Typed audio failures.
///
/// `Clone` + `PartialEq` (not just `Debug`) so tests can assert exact
/// variants, including the offending value — the same assertability the
/// physics backend errors provide.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AudioError {
    /// No audio output device is available on this machine.
    ///
    /// Returned by [`RodioBackend::try_new`](crate::RodioBackend) when
    /// the OS reports no default output device (headless CI, servers,
    /// containers without sound). This is a *condition*, not a bug: the
    /// documented answer is the decode-only
    /// [`RodioBackend::headless`](crate::RodioBackend::headless)
    /// constructor, which degrades to a simulation clock instead of
    /// panicking.
    #[error("no audio output device available")]
    NoDevice,
    /// An output device exists but opening its stream failed (bad
    /// config, device lost mid-open, unsupported sample format).
    ///
    /// Carries the backend's `Display` text only — never a
    /// `rodio::`/`cpal::` type, which must not cross the public seam
    /// (see [`crate::AudioBackend`]'s leak-freedom contract).
    #[error("audio stream failed: {message}")]
    StreamFailed {
        /// The backend's human-readable failure description.
        message: String,
    },
    /// [`AudioBackend::set_volume`](crate::AudioBackend::set_volume) was
    /// called with a source handle the backend does not know (never
    /// issued, stopped, or recycled).
    ///
    /// Always a caller ordering bug — unlike
    /// [`AudioBackend::stop`](crate::AudioBackend::stop), where
    /// double-stop after a despawn race is expected — so it fails
    /// instead of returning a bool. Carries the rejected handle's raw
    /// parts for diagnostics, mirroring physics' `UnknownBody`.
    #[error("no live audio source for handle index {index} generation {generation}")]
    UnknownSource {
        /// The rejected handle's slot index.
        index: u32,
        /// The rejected handle's generation.
        generation: u64,
    },
    /// A per-voice volume was non-finite (NaN, infinite) or negative.
    ///
    /// Carries the rejected value: gameplay math can produce NaN gains
    /// (zero-distance divisions, uninitialized curves), and a NaN
    /// reaching the mixer would poison every voice it touches. The
    /// trigger system answers by keeping the voice's last good gain and
    /// skipping the update — never by failing the tick, never by
    /// applying the poison.
    #[error("audio volume must be finite and non-negative, got {volume}")]
    InvalidVolume {
        /// The rejected volume.
        volume: f32,
    },
}

impl AudioError {
    /// Builds the [`AudioError::UnknownSource`] error for a stale or
    /// forged [`SourceHandle`].
    ///
    /// Exists so every backend (rodio, stub, future custom engine)
    /// reports the same shape from the same parts, rather than each
    /// formatting its own.
    pub fn unknown_source(handle: SourceHandle) -> Self {
        Self::UnknownSource {
            index: handle.index(),
            generation: handle.generation(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_source_names_the_rejected_handle_parts() {
        let handle = SourceHandle::from_raw_parts(7, 3);
        let error = AudioError::unknown_source(handle);

        assert_eq!(
            error,
            AudioError::UnknownSource {
                index: 7,
                generation: 3,
            }
        );
        assert_eq!(
            error.to_string(),
            "no live audio source for handle index 7 generation 3"
        );
    }

    #[test]
    fn invalid_volume_names_the_rejected_value() {
        let error = AudioError::InvalidVolume { volume: f32::NAN };
        assert!(
            error.to_string().contains("finite and non-negative"),
            "the message must state the contract, not just the value"
        );
    }

    #[test]
    fn device_errors_state_the_condition_without_backend_types() {
        assert_eq!(
            AudioError::NoDevice.to_string(),
            "no audio output device available"
        );
        let error = AudioError::StreamFailed {
            message: "unit-test cause".to_owned(),
        };
        assert_eq!(error.to_string(), "audio stream failed: unit-test cause");
    }
}
