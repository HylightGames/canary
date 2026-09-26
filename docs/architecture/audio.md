# Audio (`CanaryAudio`)

The v0.0.12 audio slice is implemented on `dev`: an `AudioBackend` trait,
game-facing source/listener components, a scheduler system, decoded sound
assets, and a rodio bootstrap backend. The custom Canary-built engine
remains the intended long-term default; this document covers that target
alongside the current implementation.

## Trait-based abstraction, custom default

```
Your Engine Audio API
          |
   Audio Abstraction Layer (AudioBackend trait)
          |
  -----------------------------------
  |                                 |
Canary Audio (default,          FMOD / Wwise
custom, in-house)                (bindable, opt-in)
```

- **Long-term default: a custom, in-house Canary audio engine.** Unlike
  rendering (native Vulkan through `ash`) and physics (bootstrapped on
  Rapier), audio's default is intended to be Canary's own, not a
  dependency this project leans on to move faster early. This is a
  deliberate exception to the usual bootstrap pattern, not an
  inconsistency in it — audio mixing, spatialization, and a DSP graph are
  a substantially smaller, more bounded effort than a competitive
  renderer or physics engine, and owning it avoids a licensing/runtime
  dependency (FMOD and Wwise are both commercial, royalty- or
  seat-licensed products in typical game-shipping configurations) that
  would sit awkwardly against an MIT-licensed, no-royalties engine.
  Building it later than `v0.1.0`'s target scope (see
  [`docs/vision/long-term-roadmap.md`](../vision/long-term-roadmap.md#release-cadence-one-focused-subsystem-per-00x-target-v010-as-substantially-feature-complete))
  is expected; it is not expected to be permanently deferred.
- **Bindable, opt-in: FMOD, Wwise, or similar.** Studios that already have
  FMOD/Wwise content pipelines, sound designers trained on them, or
  licensing already in place should be able to bind Canary to them
  without friction — the same Tier B native-binding pattern used for Jolt
  in [`physics.md`](physics.md#trait-based-abstraction-default-backend)
  applies here. "Easy to integrate" is a real design goal for this
  binding, not just a passive possibility.

Per [`docs/vision/design-philosophy.md`](../vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly),
`AudioBackend`'s public trait surface must not leak a concrete
third-party type (no `fmod::Sound`, no `wwise::...`) — whichever backend
is active, everything above the trait boundary only ever sees Canary's
own types.

## Why audio is architected now, even though it's built later

Two reasons this document exists ahead of any code, consistent with
every other subsystem doc in this project:

1. **The trait boundary has to exist before either the custom engine or
   a binding does**, and getting that boundary right — so a studio can
   genuinely drop in FMOD without fighting Canary's own assumptions about
   mixing, buses, or spatialization — is exactly the kind of design work
   worth doing deliberately rather than reactively.
2. **Audio, like physics and rendering, is downstream of the ECS.** Sound
   sources, listeners, and spatialization parameters are ECS components;
   the audio subsystem observes state (positions, emitted events) rather
   than being called into directly by gameplay code — the same
   "communicate through shared, observable state" discipline named in
   [`docs/vision/design-philosophy.md`](../vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly)
   for why physics doesn't call `canary-audio` directly to play an impact
   sound.

## Status in this foundation

`canary-audio` is implemented on `dev` for `v0.0.12` (not yet tagged):
the `AudioBackend` trait plus a **rodio bootstrap backend** (see [ADR
0023](../decisions/architecture-decision-records/0023-audio-bootstrap-rodio-behind-custom-trait.md)),
not the custom engine — the trait boundary this document demands comes
first, exactly as designed, and the custom default remains the
long-term direction. `AudioSource`/`AudioListener` components, an
`AudioConfig` resource, and a scheduler-registered trigger system
complete the milestone's game-facing surface.

The trigger is registered for a concrete backend type and ensures a
headless `Default` backend when the host has not inserted one. The
`AudioConfig.backend` field records the configured implementation; it
does not open an output device. A game that wants audible rodio playback
must construct `RodioBackend::try_new()` during startup and insert it as
`Mutex<RodioBackend>` before the first audio system run. Headless mode is
the intentional safe default. Audio reads propagated `GlobalTransform`
poses, so transform propagation must run before audio in the schedule.
