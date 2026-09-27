# 0025. Route deterministic gameplay actions through one UI-aware input path

**Status:** Proposed for `v0.0.13`; review before implementation.

## Context

ADR 0021 Amendment 4 locks the semantic boundary
`RawInput → InputMapping → InputAction → PlayerInput → SimulationInput`.
ADR 0022 Clarification 1 says `SimulationInput` is player/external intent
entering a simulation step, not a command. The current platform layer emits
keyboard press/release events only. It has no action mapping, pointer events,
window-focus reset, player-input snapshot, or UI/gameplay capture contract.

The first `CanaryUI` game consumer needs UI and gameplay to observe the same
physical events without a click or key press accidentally activating both.
Replay and the later server path also need the exact logical input consumed
by each simulation pass; raw device events and UI capture state are not a
portable simulation protocol.

## Proposed decision

1. **Keep OS input and gameplay intent in separate crates.**
   `canary-platform` normalizes operating-system input. A new `canary-input`
   crate owns game action identifiers, bindings, player assignment, and
   `SimulationInput`. Runtime composition connects platform events, the UI
   capture result, mapping, and the next simulation pass. No `winit`, `egui`,
   scancode, or platform-specific type appears in gameplay input APIs.
2. **Create one immutable logical input snapshot per simulation pass.**
   The snapshot carries a player identity, the simulation frame/tick identity,
   and deterministically ordered action states. The first sample supports
   digital actions with `down`, `pressed`, and `released` semantics. Multiple
   physical bindings to one action are aggregated before edge state is
   derived; a repeated OS key-press while already down does not create a new
   `pressed` edge. `pressed`/`released` edges appear only on the aggregate
   transition pass, never on steady-held passes. Actions sort in game-declared
   action-schema declaration order, which is the canonical order for
   replay and wire encodings. Game-owned action identities are stable
   within that game's declared action schema. Their concrete Rust
   representation remains open for API review; replay/network
   representations must be explicitly encoded under the data/schema
   version universe (ADR 0022 Clarification 6), never timestamps, raw
   events, or capture state.
3. **Give UI routing first refusal on raw events.** The UI adapter receives
   normalized raw events and reports the keyboard/pointer events captured by
   its current focus and interaction state. The mapper receives the full
   ordered event stream annotated with order-preserving per-event capture
   flags, and drops consumed events — unless a game-declared pass-through
   binding covers that event, evaluated per binding inside the mapper
   (capture can only be judged against bindings at mapping time, so the
   filter lives there, not in a pre-filter). This is a behavior contract;
   UI capture is not exposed as an `egui`-specific flag. Mapping contexts
   are not a separate layer: capture plus named pass-through is the
   context switch, and a future contexts design extends — never replaces
   — this rule.
4. **Never retain a stuck input across focus or capture changes.** Window
   focus loss clears all held controls and produces logical release/cancel
   edges for actions that were down. Pointer-leave-while-held is treated
   the same way (synthesized release), unless pointer capture is held for
   the drag — a captured pointer cannot "leave" mid-gesture. If UI begins capturing an already-held
   physical control, gameplay receives a release before the mapper forgets
   it. Duplicate and repeat events cannot synthesize extra press edges.
5. **Keep simulation input separate from UI intent.** UI callbacks return
   intent. Any resulting game change is applied through the declared runtime
   command/input boundary before or during the next simulation pass; a UI
   callback cannot mutate the live `World` during a schedule run.
6. **Bound the first slice to one local player and digital actions.** The
   platform contract for `.13` adds normalized pointer position/button and
   focus-loss events needed for a real UI, alongside existing keyboard
   transitions. Pointer positions are reported in logical pixels (physical
   `surface_extent` stays a separate, documented seam — never conflate the
   two). Player identity is a small `Copy` slot id with `0` reserved for
   the local player; per-slot profile instances (split-screen) and direct
   snapshot injection (AI agents, headless tests) ride on the same type
   later without reshaping it. Controller/analog actions, text/IME editing, remapping UI,
   persisted control profiles, multiplayer player assignment, and zero-or-
   multiple simulation passes per outer frame remain later work.

The runtime records and transmits `SimulationInput`, never physical
`RawInput`, input timestamps, or the UI capture result. Tick identity is
stamped by the runtime immediately before the scheduled pass (the mapper
stamps `frame_index` at route time, which precedes the tick advance);
tests assert the observed pair, never an assumed one. `SimulationInput`
travels as a per-frame-overwritten ECS resource — like `RunContext`,
read by consumers and written only by the runtime/input phase — so it
never trips the quiet-tick probe the way component data would. `.13`
uses one simulation pass per outer frame; a later fixed-step runner must define how
queued events and held action state map to zero or multiple simulation steps
before that behavior is added.

## Alternatives considered

**Let each gameplay system read `InputSource` directly.** Rejected. Systems
would couple themselves to device events, duplicate mapping and focus policy,
and make replay/networking record platform-specific history rather than the
intent that actually entered simulation.

**Let UI and gameplay poll raw events independently.** Rejected. Both
consumers can react to one physical event, and there is no single place to
enforce capture, focus loss, or deterministic event ordering.

**Put action mapping in `canary-platform`.** Rejected. Platform normalization
belongs at the OS boundary; game action names, player slots, and simulation
input are higher-level engine concerns. Growing the platform crate into a
second subsystem would also violate the one-crate-per-subsystem rule.

**Expose `egui`'s capture flags as the gameplay contract.** Rejected. It
would make gameplay behavior change with the selected UI backend and tie the
input layer to the bootstrap implementation ADR 0011 is intended to hide.

**Add axes, device remapping, player assignment, and saved profiles in the
first input slice.** Deferred. Current platform support is keyboard-only;
the `.13` game proof needs a small digital action set and pointer events for
UI. The richer device model has no consumer evidence yet and would not improve
the focus/capture proof.

## Consequences

- A new `canary-input` crate depends upward from normalized platform events;
  `canary-platform` remains unaware of game action names and UI backends.
- Gameplay systems consume a deterministic snapshot that can be injected in
  headless tests and later recorded/replayed or sent by the networking
  subsystem.
- UI and gameplay share one event order and a single capture decision; focus
  loss and capture transitions explicitly release held gameplay actions.
- The first API is digital and single-player. Analog action encoding and
  serialized user bindings need separate consumer evidence before they are
  added.
- `SimulationInput` is still a different channel from structural commands,
  deterministic simulation messages, and presentation observations, per
  ADRs 0021–0022.

## Revisit conditions

- A fixed-step runner that performs zero or multiple simulation passes per
  outer frame needs an explicit event-consumption and held-state sampling
  amendment before implementation.
- Controller/analog or multi-player consumers may justify richer action-value
  and device-assignment semantics.
- A second UI backend may test whether the backend-neutral capture contract
  is sufficient; it must not silently introduce backend-specific gameplay
  behavior.
