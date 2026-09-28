# Input and Simulation Contract

This document summarizes the accepted input and simulation contracts in
[ADR 0021](../decisions/architecture-decision-records/0021-amendments-to-pre-v0-3-locks.md)
and [ADR 0022](../decisions/architecture-decision-records/0022-constitution-clarifications-and-red-team.md).
The layer boundary is accepted; the detailed first-consumer contract below
is accepted in
[ADR 0025](../decisions/architecture-decision-records/0025-deterministic-input-actions-and-ui-capture.md)
and proven by the `.13` game/UI consumer (`examples/ui-game`).

## Current implementation

`canary-platform` normalizes keyboard transitions, pointer position
(logical pixels) and buttons, pointer-leave, and focus-loss events; it
does not map physical events to gameplay intent. The `canary-input`
crate maps those normalized events to named gameplay actions and
produces frame-tagged `SimulationInput` (one local player, digital
actions only — see ADR 0025). `canary-runtime` consumes it through
`drive_input_frame` (UI-first capture routing, in-mapper per-binding
pass-through) and `Runtime::drive_frame` (tick stamping, `SimulationInput`
publishing, then the scheduled pass). The `canary-runtime` library owns
`RunContext` as an ECS resource alongside the initial scoped-plugin
runtime slice; `begin_sim_pass` advances tick/`sim_time` only on
simulation passes (R-38 mitigated). A general fixed-step simulation
runner remains future runtime work.

## Input path

Gameplay and deterministic simulation consume logical actions rather than
device-specific key codes:

```text
RawInput → InputMapping → InputAction → PlayerInput → SimulationInput
```

- **RawInput** is normalized platform input, such as a key transition or
  controller axis sample.
- **InputMapping** converts device input to named logical actions and owns
  remapping/context policy.
- **InputAction** represents intent, such as `MoveForward = 1.0` or
  `Jump = pressed`.
- **PlayerInput** associates actions with a player slot.
- **SimulationInput** is the frame-tagged input crossing into one
  deterministic simulation step. Replay and networking record this layer,
  not raw device events.

### Proposed `.13` behavior

The input-action layer belongs in a new `canary-input` crate above
`canary-platform`; keep platform code responsible for OS normalization and
keep UI/backend types out of the gameplay mapping API. The game declares its
own logical action identifiers and physical bindings. The first sample uses
one local player and digital actions only. Controller/analog input, remapping
screens, serialized control profiles, and multiplayer player assignment are
later work.

For each simulation pass, the runtime supplies an immutable, deterministically
ordered `SimulationInput` snapshot. It identifies the player and simulation
frame/tick, and carries logical action state (`down`, `pressed`, `released`)
rather than physical key codes. Multiple physical bindings to one action are
combined: the action becomes pressed only on the aggregate up-to-down
transition and released only on the aggregate down-to-up transition. OS key
repeat does not produce another press edge. Exact action-ID and value
representations remain API-review details; any representation that reaches
replay or networking must have a stable, versioned encoding.

The UI gets the first opportunity to consume the ordered raw event stream.
Its backend-neutral routing result identifies which keyboard/pointer input is
captured for the current UI state. The mapper sees only unconsumed events,
unless a game explicitly opts a binding into pass-through. Capture is
per-event/category behavior, not a global `egui` flag exposed to a game. If a
window loses focus or capture begins while a gameplay control is held, the
mapper emits the corresponding release/cancel before clearing that control;
no held action can stick. A UI callback returns intent and cannot mutate the
`World` while a schedule is running.

The initial platform slice therefore needs normalized keyboard transitions,
pointer position/buttons, and focus-loss notification. The event order
observed by the backend is preserved. Text editing, IME, wheel/gesture
bindings, analog devices, and platform-specific scancodes do not enter this
first gameplay input contract.

The first mapping is supplied by the game/sample at runtime, not authored
project data. `.14` persistence may later store user control preferences, but
must not make physical bindings part of the deterministic simulation
snapshot. Record and network `SimulationInput`, not `RawInput` or UI capture
metadata.

## Simulation time and execution

The runner owns a single logical tick per simulation run. It advances that
tick before the systems for the run execute; the scheduler only executes
work and never advances time. Wall-clock duration, presentation frame,
physics simulation time, and logical ECS tick remain distinct. One
presentation frame may contain no simulation step or multiple simulation
steps once a fixed-step runner exists.

`RunContext` is implemented in `canary-runtime` as a resource with run ID,
outer frame index, ECS tick, frame delta, simulation time, and simulation-step
duration. [`Runtime::begin_frame`](../../../engine/canary-runtime/src/lib.rs)
writes the frame-open state: `tick` names the most recent scheduled pass (the
tick stands still on event/presentation-only frames), `sim_time` is untouched,
and `sim_step` reads as zero. When the frame runs a simulation pass,
[`Runtime::begin_sim_pass`](../../../engine/canary-runtime/src/lib.rs) folds
the step into `sim_time`, advances the ECS tick exactly once (via the
`advance_tick_for_pass` entry the R-34 review accepted), and re-stamps the
resource — so `tick` always names the pass about to run, and systems observe
the documented value (R-38, resolved). The frame driver stamps the snapshot's
own `tick` from that context and publishes `SimulationInput` plus `UiIntents`
before the schedule runs. Simulation time therefore advances only for actual
simulation passes; plugin lifecycle phases and event/presentation frames never
move the tick. The legacy `Runtime::run` entry point is still the scoped-access
slice, not yet the complete game frame loop. See
[`runtime-composition.md`](runtime-composition.md) and the
[R-34 API review](../reviews/2026-09-r34-api-review.md) for the implemented
boundary.

## Commands, messages, and observations

Per ADR 0021 Amendment 8 and ADR 0022 Clarification 2:

- A **Command** requests structural or state mutation and is applied at a
  defined point in simulation execution.
- A **Simulation message** is deterministic and ordered; it can affect
  simulation through the command path and may need replay.
- An **Observation event** notifies UI, audio, diagnostics, or tooling. It
  is not authoritative state and is not recorded for rollback or replicated.
- A **Resource** is persistent shared state owned by the simulation.

The queue and delivery APIs are not implemented. Before building them,
define same-run versus next-run delivery, ordering, producer/consumer
behavior, determinism, retention, and droppability.

## Snapshot boundary

Simulation snapshots cover ECS entities and components, deterministic
resources, explicitly owned RNG state, simulation clocks, and schema/version
information. They exclude GPU resources, window and operating-system
handles, audio-device state, editor state, worker-pool internals, and
temporary caches. The contract is `snapshot` / `restore` / `checksum` /
`step(SimulationInput)`.

Project-authored data is a separate product. It uses stable authored IDs,
versioned codecs, and migrations; it does not serialize every runtime
resource. Networking can reuse codecs and snapshot contracts, while adding
authority, removal history, and canonical ordering.

## Determinism prerequisites

- Canonical ordering is required at snapshot and wire-format boundaries;
  ordinary ECS query order is not a persistence guarantee.
- Random values that affect simulation use explicitly owned deterministic
  streams or stable-key derivation, never ambient global RNG or incidental
  query order.
- Component and asset schema identities remain separate from runtime
  handles and content hashes.
- Removal and destruction require durable records alongside mutation
  change detection.

The v0.1.0 sequence assigns action mapping to the windowed UI milestone,
the authored identity and snapshot contracts to project state, and removal
history plus canonical replication to networking. Those are acceptance
gates, not optional follow-up polish.
