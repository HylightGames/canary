// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Canary Engine audio: game-state-driven playback over a swappable backend.
//!
//! See `docs/architecture/audio.md` for the full design and
//! [ADR 0023](https://github.com/HylightGames/canary/blob/dev/docs/decisions/architecture-decision-records/0023-audio-bootstrap-rodio-behind-custom-trait.md)
//! for the backend lineup this crate records: **rodio is the
//! explicitly-labeled bootstrap**, the custom in-house engine is the
//! long-term default, and FMOD/Wwise bindings are opt-in Tier B work.
//!
//! # What is implemented (v0.0.12: trigger + bootstrap backend)
//!
//! - [`backend`]: the object-safe, leak-free [`AudioBackend`] trait,
//!   opaque [`SourceHandle`] keys, and typed [`AudioError`] failures.
//! - [`RodioBackend`]: the private rodio implementor (module
//!   `rodio_backend` stays private; only the type is re-exported so
//!   scheduler access declarations and resource lookups can name it).
//!   Exact pin `rodio = "=0.22.2"`, `default-features = false` with
//!   only MIT/Apache-2.0 decoder features (`hound`, `lewton` —
//!   Symphonia is MPL-2.0 and stays out of the graph).
//! - [`components`]: [`AudioSource`] (asset handle + trigger state +//!   volume + loop flag), [`AudioListener`] (pose marker), and the
//!   [`AudioConfig`] resource (master volume plus `#[non_exhaustive]`
//!   backend selection).
//! - [`systems`]: Canary-owned [`attenuation_gain`]
//!   math, the [`AudioVoices`] table,
//!   [`audio_trigger_system`] (play on
//!   transition, stop on removal/despawn, per-tick gains), and
//!   [`register_audio_trigger`] —
//!   which MUST run AFTER the gameplay systems whose state transitions
//!   it actuates.
//!
//! # What is explicitly deferred (later releases own it)
//!
//! - DSP graph, buses and mixing policy beyond per-source volume,
//!   HRTF, Doppler, gapless-music guarantees, reverb zones (later
//!   audio milestones, none partially implemented here).
//! - Streaming audio: sources are fully decoded [`Sound`](canary_assets::Sound)
//!   values; long streams arrive with the asset-streaming milestone.
//! - One-shot completion events: voices stop on explicit stop or
//!   despawn, never on polled finish.
//! - FMOD/Wwise bindings and the custom in-house engine (the trait is
//!   shaped so neither fights mixer/bus/spatialization assumptions,
//!   but neither ships in this crate).
//! - WASM output proof: playback targets native devices in this
//!   release.
//!
//! # A note on dependencies
//!
//! This crate depends on `canary-ecs`, `canary-scheduler`, and
//! `canary-transform` (the trigger seams the system is built on),
//! `canary-assets` (the [`Sound`](canary_assets::Sound) values voices
//! play), `glam` for boundary math, `thiserror` for typed errors, and
//! `rodio` — which is pure safe Rust on the paths this crate uses, so
//! no `unsafe` blocks are expected anywhere in this crate.

pub mod backend;
pub mod components;
pub mod error;
pub mod systems;

mod rodio_backend;

pub use backend::{AudioBackend, SourceHandle};
pub use components::{AudioBackendName, AudioConfig, AudioListener, AudioSource, SourceState};
pub use error::AudioError;
pub use rodio_backend::RodioBackend;
pub use systems::{
    attenuation_gain, audio_trigger_access, audio_trigger_system, register_audio_trigger,
    AudioVoices, VoiceEntry,
};
