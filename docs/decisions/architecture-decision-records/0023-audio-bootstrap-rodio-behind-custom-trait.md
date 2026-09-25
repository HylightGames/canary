# 0023. v0.0.12 audio bootstrap: rodio behind a custom-first trait

**Status:** Accepted

## Context

`docs/architecture/audio.md` states the long-term default is a custom
in-house audio engine, with third-party code only as bootstrap or
opt-in bindings — but never assigned audio a release, while
`docs/roadmap/v0.1.0-plan.md` (newer, owner-directed) assigns v0.0.12 a
"real backend (rodio)" without reconciling the two. The plan's "per
audio.md" citation overclaims: audio.md never authorizes rodio. A
September 2026 research pass (rodio 0.22.2 status, kira/oddio/fundsp/
cpal-direct alternatives, custom-engine cost estimate) resolved the
conflict. This ADR records the resolution.

## Decision

v0.0.12 ships **rodio as an explicitly-labeled bootstrap backend**
behind a custom-engine-first `AudioBackend` trait — the same posture
as Rapier2D ("canonical 2D" pending Jolt 3D, ADR 0019), not a
repudiation of the custom default, which is retained as the long-term
direction. Concretely:

- New crate `canary-audio` (one-crate-per-subsystem): object-safe,
  leak-free `AudioBackend` trait (no `rodio::`/`cpal::`/`symphonia::`
  types in public signatures — note rodio publicly re-exports cpal,
  so do not re-export it), game-facing ECS components (`AudioSource`,
  `AudioListener`), `AudioConfig` resource with `#[non_exhaustive]`
  backend selection, typed errors.
- **Private rodio backend**, pinned exact version (rodio 0.22.x line;
  it keeps a breaking cadence with an UPGRADE guide — pin, don't
  float). **MIT/Apache-only decoder features by default**; Symphonia
  (MPL-2.0, file-level copyleft) is opt-in, never default, per this
  engine's no-entanglement values. Own the (small) distance
  attenuation/pan math rather than depending on `SpatialPlayer`
  (open upstream bug with speed+looped decoders).
- Bus/mixer/spatialization concepts are **Canary types**, so a later
  custom engine or FMOD/Wwise Tier B binding does not fight rodio's
  `Sink`/player assumptions.
- Sound data arrives as `AssetHandle`s through the v0.0.10 pipeline
  (WAV + Ogg Vorbis coverage for the milestone bar); a scheduler
  system triggers sources from game state; headless verification uses
  rodio's decode-only (`playback`-less) build.
- Explicit non-goals for v0.0.12: DSP graph, buses, HRTF/Doppler,
  gapless-music guarantees, streaming, FMOD/Wwise bindings, WASM
  output proof.

## Alternatives considered

**kira:** best game-mixer of the alternatives, but heavier abstraction
to wrap for zero gain at this bar, single maintainer, WASM-limited,
no better spatial. Rejected.

**oddio:** philosophically closest to a custom engine (sans-I/O,
wait-free, Doppler + propagation delay) but stale (~3 years, 0.7.4)
and decode-yourself. Rejected as backend; kept as design reference
for the eventual custom engine.

**fundsp:** DSP graph, not a playback engine (no device sink).
Rejected as backend; noted as future DSP-substrate candidate.

**cpal + hand-rolled decoders/mixer:** is starting the custom engine
under another name — over-scoped for v0.0.12. Rejected.

**Start the custom engine now:** mixer + resampler + decoders +
spatializer + DSP + sinks is a multi-milestone effort, and even
"custom" still sits on cpal for OS I/O. It delays the v0.1.0
sample-game goal for zero verification payoff. Rejected for v0.0.12;
retained as the long-term default.

## Consequences

- `rodio 0.22.x` (exact pin) joins the trusted-core set; `libasound2`
  becomes a Linux CI dependency (same class as the existing
  Wayland/Vulkan ICD CI deps).
- `audio.md` is updated to record the bootstrap (status section,
  release assignment); its custom-default direction stands.
- A future custom engine replaces the private backend crate-side,
  never the trait or components — that replaceability is acceptance
  criteria for the v0.0.12 review, mirroring the Box2D-style
  alternative analysis where cheap.

## Implementation status

Pending — assigned to v0.0.12. Docs updated in the same change as this
ADR (`audio.md` status + release assignment); code to follow.

## Revisit conditions

- If rodio maintenance lapses (no release in >12 months, unaddressed
  soundness issues) before v0.1.0, re-run the alternatives table
  rather than floating versions hoping for fixes.
- If the custom engine starts, this ADR's bootstrap framing is what
  makes that a backend swap instead of an architecture change — no
  new ADR needed for the swap itself unless the trait must grow.
