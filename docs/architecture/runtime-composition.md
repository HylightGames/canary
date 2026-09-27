# Runtime Composition and Consumer Lifecycle

**Status:** The R-34 scoped-access foundation is implemented in
`canary-runtime` (commit `e256a61`) and its API review is accepted for that
scope. The full game frame driver and subsystem composition described here
remain proposed work for `v0.0.13`. [ADR 0024](../decisions/architecture-decision-records/0024-reusable-runtime-composition.md)
remains Proposed for the broader composition contract; the narrower R-34
implementation contract is in
[`2026-09-r34-api-review.md`](../reviews/2026-09-r34-api-review.md).

## Purpose

A game needs one supported way to compose Canary's platform, input,
simulation, plugins, audio, UI, rendering, and shutdown phases. The same
simulation path must remain usable by an interactive game, a headless
server, and tests. A game chooses its content and backend implementations;
the runtime owns the order in which their work touches the active world.

This is a higher-level composition concern. `canary-core` stays unaware of
platform windows, render devices, audio devices, plugins, and the scheduler.
`canary-core::App` remains the lightweight lifecycle runner for registered
subsystems. The proposed reusable consumer surface belongs in the upward
composition package, `canary-runtime`.

## What exists today

- `canary-core::App` initializes registered `Subsystem`s in registration
  order, calls each subsystem's `tick(dt)` in that order, and shuts down
  initialized subsystems in reverse order. `run_for` supplies a fixed
  duration; `run` measures wall-clock duration and accepts a caller-provided
  stop condition. It does not define phases within a frame or own an ECS
  tick.
- `canary-runtime` now also exposes a reusable library. Its `Runtime` owns
  one active `World`, a `RunContext` resource, Tier A plugin registrations,
  scoped lifecycle calls, and teardown. `run` currently loads plugins,
  advances outer-frame context for a requested frame count, then unloads
  plugins. It does **not** run a `Schedule`, pump a window, route input, or
  render. The pre-existing headless binary still owns its private
  `EcsSubsystem` and has not yet migrated to the library. That migration and
  the complete consumer frame driver are `.13` close-out work.
- `canary-scheduler::SystemAccess` is manual metadata. The current scheduler
  serializes writers and allows compatible read-only stages to run
  concurrently, but it cannot verify what a closure accesses (R-24).
- Tier A scoped calls can now operate on the active runtime `World` through
  the accepted ownership-loan boundary. The world is moved into a store slot
  for one synchronous guest call and moved back on success or trap; no Rust
  reference or pointer crosses the call. The first implementation covers
  `on_load`/`on_unload` only. It adds no per-frame hook and permits no overlap
  with schedule execution. See the API review for trap cleanup and
  reentrancy details.
- The runtime's Tier A registration path now uses a fallible scoped-loader
  result: required failures abort with typed errors and optional failures
  are recorded as skipped. This does not imply that all future platform,
  renderer, audio, UI, and schedule services have been composed or tested.
- `Window::poll_events` is synchronous. The current `winit` integration pumps
  events from that call, and its event loop has main-thread constraints. The
  runtime contract must allow the platform backend to be pumped on its
  required thread.

These are implementation facts, not the consumer contract proposed below.

## Proposed ownership boundary

The game assembles the runtime from its game logic and selected platform,
render, UI, audio, and plugin backends. The runtime package exposes the
supported application entry point and owns the active `World`, schedule
execution, run context, phase order, and teardown for the duration of a run.
The consumer may populate and configure its world before starting; once the
run begins, the runtime is its single owner.

This gives each concern one clear owner:

| Concern | Owner during a run |
|---|---|
| OS window and raw platform events | Selected `canary-platform` implementation |
| Active ECS `World` and its logical tick | Runtime composition layer |
| System ordering inside a simulation pass | The composed `Schedule` |
| Game content, gameplay systems, and action bindings | Game/consumer |
| GPU/audio resources and backend-specific handles | Their selected subsystem backend |
| Cross-subsystem phase order and shutdown | Runtime composition layer |

The runtime is the composition point; it does not make a backend mandatory.
Headless consumers omit presentation services while using the same
simulation and lifecycle contract. Concrete backend types remain behind
their subsystem boundaries, consistent with
[`engine-overview.md`](engine-overview.md#layering) and the subsystem ADRs.

The proposed public library surface is part of the existing `canary-runtime`
package. Its current headless binary can remain a smoke-test consumer of
that library. This proposal does not add a second `canary-app` or
`canary-game` crate before there is a consumer need that justifies that
boundary.

## Run context and time ownership

The runtime is the sole owner that creates and advances run context. A
schedule and its systems consume a read-only context; they do not advance
time. The context needs to distinguish:

- a run identity, unique to one runtime execution;
- an outer-frame index, incremented once for each outer runtime-loop
  iteration, including headless runs;
- the ECS `Tick` assigned to a scheduled world pass;
- elapsed wall-clock frame time; and
- simulation time and simulation-step duration where a simulation runner
  provides them.

Frame time and simulation time have different units of meaning and must not
share an ambiguous `dt` field. Each scheduled pass that may write the world
receives one tick: the runtime advances it immediately before that pass,
and the scheduler executes without advancing it. Event-only or
presentation-only work does not advance `Tick`. If a future fixed-step
runner executes zero or several scheduled simulation passes in one outer
frame, each actual pass gets its own tick while retaining that frame's
index.

For the initial `v0.0.13` composition, preserve the current one-schedule-pass
per outer frame behavior. The physics subsystem may consume that frame time
through its existing bounded fixed-step accumulator. Physics `SimulationTime`
advances per completed physics substep and remains distinct from ECS `Tick`;
the runtime contract does not imply a world tick per physics substep. A
general fixed-step game runner is a separate design and is not required to
compose the first interactive consumer.

`RunContext` is currently an ECS resource owned and updated by
`canary-runtime`. `begin_frame` increments the outer-frame index and updates
frame/simulation durations without advancing the ECS tick;
`advance_tick_for_pass` advances exactly once for a scheduled pass. The full
game frame driver that calls these at the correct phase boundaries remains
unimplemented. The current library slice uses a fixed requested frame count
and a caller-provided frame duration; it is not yet a wall-clock window loop.
In the current scaffold, `begin_frame` also accumulates `sim_time` before a
scheduled pass and snapshots the current world tick into the resource before
`advance_tick_for_pass` is called. The accepted R-34 API review describes the
context tick as the current or most recent pass and places this resource
update before advancement. The full frame driver must preserve and document
that meaning, or record an explicit review/ADR amendment before changing it.
Outer-frame metadata may advance without simulation, but simulation time and
ECS `Tick` must not (R-38).

## Proposed phase order

The runtime owns these phase boundaries. A phase may be omitted when its
service is absent, as in headless operation.

1. **Initialize.** Validate the selected services and game configuration;
   initialize services before the loop. If an existing plugin load callback
   is granted active-world access, run it only after the runtime world and
   schema codecs are ready and before simulation starts. If initialization
   fails, tear down everything that initialized successfully in reverse
   order.
2. **Pump platform events.** Poll the platform on its required thread,
   update window state, and collect normalized raw input. A close request is
   observed before starting another simulation pass.
3. **Route input.** Give UI input routing the opportunity to report
   consumption, then map remaining input through
   `RawInput → InputMapping → InputAction → PlayerInput → SimulationInput`.
   The exact focus/capture rule is specified with the input/UI seam in
   `input-and-simulation.md`; this document establishes its place in the
   frame order.
4. **Run simulation.** Advance the world tick once before each schedule
   invocation. Apply simulation input and run the ordered simulation
   schedule. Structural commands and deterministic messages follow their
   own declared barriers and delivery rules from ADRs 0021–0022. UI-originated
   changes enter at a defined simulation boundary, never by mutating the
   world in the middle of a running schedule.
5. **Process observations and audio effects.** Consume observation events
   produced by simulation for audio, diagnostics, and other presentation
   effects. These events do not become authoritative simulation state.
6. **Update UI and extract presentation state.** The UI reads the current
   published game state and produces UI output. Rendering extracts an owned,
   read-only snapshot from ECS state; renderer submission does not retain a
   live `World` borrow.
7. **Render and present.** Submit the scene and UI through the selected RHI
   path, then present through the selected platform surface. Surface
   capability, recreation, and recoverable present behavior are specified
   separately under ADRs 0020–0022.
8. **Wait or begin the next outer frame.** Frame pacing belongs to the
   runtime/platform integration and does not change simulation time or tick
   ownership.

The input work package may refine how UI routing and action mapping share
events. The surface work package may refine how the platform window is
connected to the renderer. Neither changes the runtime's responsibility to
make those phase boundaries explicit.

## Scoped access for Tier A plugins

The runtime owns the one active game `World`. A Tier A plugin accesses it
only through its declared host interfaces (`ecs-read` and/or `ecs-write`),
with the existing capability and value-codec checks enforced. A plugin
receives no raw Rust `World`, pointer, or reference that can outlive one
synchronous host invocation, and it cannot retain a second owned world as
the apparent game state.

The runtime invokes world-capable plugin callbacks at explicit exclusive
boundaries, outside a running schedule and on the runtime thread. The
callback's access scope ends when that guest call returns or traps. Plugin
callbacks are serialized with schedule execution; a callback cannot run
concurrently with a system that may read or write the same world. Changes
made through an authorized write interface are visible to the next runtime
phase that reads the world. A later design may relax this only with an
enforceable access model; current manual `SystemAccess` declarations are
not sufficient evidence for concurrent plugin/world access.

The accepted implementation uses an ownership loan rather than a borrowed
view: the active `World` is temporarily moved into a host-state slot before
`Func::call` and moved back after normal return or trap cleanup. The slot
prevents nested loans; exclusive `&mut Runtime` phase calls prevent overlap
with schedule execution. No raw pointer or unsafe aliasing scheme is used.
The implementation contract and proofs are recorded in the linked API
review; this design does not generalize that mechanism into concurrent plugin
or system access.

This first runtime contract covers scoped access during callbacks the
plugin interface already defines. It does not create a new per-frame plugin
hook or an editor-panel API. Tier B remains the explicitly trusted native
extension tier and does not gain an implicit `World` pointer through this
decision.

## Errors, stop requests, and shutdown

- A required service initialization failure aborts startup. The runtime
  shuts down all successfully initialized services in reverse initialization
  order and returns the original typed failure with any cleanup failures
  retained as secondary context.
- An unhandled fatal runtime-phase error stops the loop, starts teardown,
  and is returned after cleanup. A subsystem handles a recoverable condition
  locally and reports it through its documented status/diagnostic contract;
  it does not silently turn a failed operation into a successful frame.
- A user stop request or observed window-close request prevents the next
  simulation pass from starting. The runtime invokes shutdown exactly once
  for each successfully initialized service, in reverse order.
- The active `World` remains valid through any authorized plugin unload
  callback and service teardown that needs ECS state. The runtime drops the
  world only after those callbacks and services finish.
- Cleanup attempts continue after an individual cleanup failure. Cleanup
  failures do not replace the original startup or runtime failure.
- Panics are not converted into success. The existing `App` behavior—attempt
  shutdown of all initialized subsystems before resuming a panic—remains the
  minimum panic-safety guarantee.
- The R-34 runtime library surfaces scoped Tier A load/unload failures as
  typed results and records optional-plugin skips. The broader service
  contract for platform, schedule, audio, UI, and renderer initialization is
  still proposed and must preserve the primary-error/cleanup-error rules
  above.

The R-34 plugin-load path has typed errors and optional-plugin outcomes.
Exact service/error hook shapes for the full platform, schedule, input, audio,
UI, and rendering frame driver remain implementation-review details; they
must preserve the primary-failure and cleanup guarantees above.

## Lifecycle scope and explicit deferrals

For `v0.0.13`, the lifecycle is one initialization, a run of outer frames,
then one shutdown. Runtime ownership establishes the boundary needed by a
game and by the first plugin/world proof. It does not add pause/resume,
restart-in-place, hot reload, subsystem replacement, editor play mode, or
world cloning. Those transitions remain R-36 design work before the editor,
hot-reload tooling, or richer live-collaboration lifecycle needs them.

This proposal also does not promise multiple windows, concurrent plugin
callbacks, parallel mutable systems, renderer/device recovery policy beyond
the renderer's own typed contract, or a generic fixed-step gameplay
schedule. Each needs its own consumer evidence and design.

## `v0.0.13` review and acceptance evidence

The R-34 portion has been reviewed and implemented. Before the full `.13`
frame driver is treated as accepted, review or amend the broader proposal and
demonstrate:

- a game consumer and the headless harness use the same public composition
  library;
- the phase order and stop behavior are observable and documented;
- the runtime, rather than a subsystem or schedule, advances the ECS tick
  exactly once per scheduled world pass;
- event-only and presentation-only frames do not advance the ECS tick;
- startup failure, fatal runtime failure, normal close, and panic each
  attempt reverse-order cleanup without hiding the primary failure;
- a Tier A plugin reads and writes the active game `World` only through
  granted host operations, and denied capabilities remain structurally
  unavailable;
- no plugin invocation overlaps a schedule run, and no world borrow or
  handle escapes the invocation scope; and
- a live consumer demonstrates the chosen platform thread and surface
  ordering alongside the input/UI phase placement.

See the [`v0.0.13` plan](../roadmap/v0.1.0-plan.md#v0013-canaryui--windowed-presentation)
for the complete milestone sequence and live-window acceptance bar.
