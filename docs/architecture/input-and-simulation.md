# Input and Simulation Contract

This document summarizes the accepted input and simulation contracts in
[ADR 0021](../decisions/architecture-decision-records/0021-amendments-to-pre-v0-3-locks.md)
and [ADR 0022](../decisions/architecture-decision-records/0022-constitution-clarifications-and-red-team.md).
It records the implementation boundary and milestone prerequisites; it
does not prescribe an input-mapping API before a real consumer exists.

## Current implementation

`canary-platform` normalizes operating-system events into engine input
events. It does not yet map raw keys, buttons, or axes to gameplay intent.
There is no `InputAction`, mapping context, player-input layer, or
frame-tagged simulation input type. The application loop supplies elapsed
wall-clock duration to subsystems. The `canary-runtime` headless harness
advances its ECS tick once before each scheduled simulation run; a general
`RunContext`, fixed-step simulation runner, and presentation pacing policy
remain future runtime work.

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

The mapping implementation, device profiles, remapping UI, and action API
remain open until a game and UI consume them. The layer boundary is already
locked and must exist before `CanaryUI` and gameplay are accepted as sharing
one input path.

## Simulation time and execution

The runner owns a single logical tick per simulation run. It advances that
tick before the systems for the run execute; the scheduler only executes
work and never advances time. Wall-clock duration, presentation frame,
physics simulation time, and logical ECS tick remain distinct. One
presentation frame may contain no simulation step or multiple simulation
steps once a fixed-step runner exists.

The intended runtime context carries the logical tick, simulation time,
outer frame number, delta, and run identity. Its API and the fixed-step
accumulator are future implementation work. The current headless harness
uses a simpler direct `World::advance_tick` boundary, which is sufficient
to keep change detection and transform propagation meaningful there.

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
