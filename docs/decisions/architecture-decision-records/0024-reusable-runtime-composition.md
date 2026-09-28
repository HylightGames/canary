# 0024. Reusable game-runtime composition and scoped plugin access

**Status:** Accepted in part for `v0.0.13` (2026-09-27). Items 1–5 are
implemented: composition lives in the `canary-runtime` library (no new
crate), `Runtime::drive_frame` owns one active world, phase order
(event pump → UI routing → tick → schedule), and tick ownership
(`begin_sim_pass` advances tick/`sim_time` only for real simulation
passes — R-38), proven by the migrated headless harness, the `ui-game`
sample, and the `frame_driver`/`headless` integration tests. Two parts
stay Proposed: item 6 lifecycle failure semantics (`drive_frame`
returns `DrivenFrame`, not a typed phase error — panics propagate; no
consumer needs failure contracts yet) and wiring the physics
fixed-step accumulator into a `.13` consumer (the accumulator is
preserved untouched in `canary-physics`; neither consumer runs physics
systems). The scoped R-34 runtime/library slice is implemented in `e256a61`; its narrower API
contract is accepted in
[`2026-09-r34-api-review.md`](../../reviews/2026-09-r34-api-review.md).

## Context

Canary has working subsystem traits, a scheduler, platform abstractions,
rendering and physics slices, a headless runtime binary, and a reusable
`canary-runtime` library foundation. The library owns one active `World`,
`RunContext`, scoped Tier A lifecycle calls, and teardown, but its current
`run` method does not execute the schedule or compose platform, input, UI,
audio, and rendering phases. The headless binary still owns a `World` and
`Schedule` in its private `EcsSubsystem`; a game cannot yet use the complete
phase driver as a supported runtime.

The runtime must own simulation time advancement and phase ordering without
making lower-level crates depend on the platform, renderer, audio, UI, or
plugin loader. It must also provide Tier A plugins scoped access to the
active game world. The R-34 implementation now scopes Tier A access to that
world at serialized `on_load`/`on_unload` boundaries; the scheduler's manual
`SystemAccess` declarations still cannot prove concurrent access safe (R-24).
`Subsystem` still offers only `init`, `tick`, and `shutdown`; richer lifecycle
transitions are a separate risk (R-36).

ADRs 0021–0022 already lock runner-owned ticks, the input/simulation
boundary, command/message semantics, and the simulation/presentation split.
This ADR defines the composition direction that applies those foundations
to the first consumer runtime. The R-34 API and ownership-loan mechanism are
settled by the linked implementation review. Exact public service and frame
driver APIs for the remaining platform/simulation/UI/render phases remain
open until the consumer loop is reviewed.

## Proposed decision

1. **Place reusable composition in the existing `canary-runtime` package.**
   Promote it from a binary-only harness package to a public library
   composition layer; the current headless executable may remain as a
   consumer of that library. Keep `canary-core` as a lower-level,
   subsystem-agnostic lifecycle primitive. Do not create a new
   `canary-app`/`canary-game` crate until a second real composition consumer
   shows that the boundary needs to split.
2. **Make the runtime the owner of one active world and its execution.**
   A consumer supplies game logic and selects optional subsystem backends.
   The runtime owns the active `World`, schedule execution, `RunContext`,
   phase order, and teardown from start through shutdown. A consumer may
   configure and populate its world before the run; during the run, access
   is mediated by the runtime's documented phase and capability boundaries.
3. **Centralize phase order, retain headless composition.** The runtime
   defines the sequence for platform event pumping, input routing,
   simulation, observation/effect processing, UI, render extraction,
   rendering/presentation, and shutdown. Presentation services are optional;
   a headless game/server/test uses the same simulation and lifecycle
   contract with those phases omitted.
4. **Keep time ownership in the runtime.** The runtime alone advances ECS
   `Tick`, exactly once immediately before each scheduled pass that may
   mutate the world. Schedules and systems consume context and do not
   advance the tick. Frame time, physics simulation time, and logical ECS
   ticks remain distinct. The first `v0.0.13` composition preserves one
   scheduled pass per outer frame and the existing physics fixed-step
   accumulator; a general fixed-step game runner is a separate design.
5. **Scope Tier A access to synchronous host invocations.** The active game
   world is exposed only through granted Tier A host interfaces and the
   existing schema codecs. Plugin calls happen at explicit exclusive
   runtime boundaries, outside a running schedule, one at a time. No raw
   `World` reference, pointer, or independently owned substitute world
   escapes as the game state. This first scope applies to the existing
   `on_load`/`on_unload` callbacks; it does not add a per-frame plugin hook.
   The implementation must prove scoped access and trap cleanup safe
   without relying on unverified scheduler metadata.
6. **Define basic lifecycle failure semantics now.** A required-service
   initialization failure aborts startup and cleans up successfully
   initialized services in reverse order. An unhandled fatal phase failure
   stops the loop, cleans up, and returns a typed error. Normal close stops
   before the next simulation pass. Cleanup continues after individual
   failures and does not hide the primary failure; panics remain panics after
   cleanup is attempted. Pause/resume, hot reload, replacement, and restart
   in place are not part of this first lifecycle. These guarantees apply to
   errors surfaced by the runtime service contract. The legacy `Plugin`
   trait hooks remain infallible; the R-34 scoped Tier A runtime path has a
   fallible loader result for required failures and optional-plugin skips.

The phase order, time semantics, scoped-access invariant, and lifecycle
failure behavior are proposed for the complete runtime. R-34's ownership
loan, `RunContext` resource delivery, and scoped plugin APIs have been
implemented and reviewed. Remaining type names, builder shape, service
registration syntax, and phase-driver API still need consumer-shaped review.

## Alternatives considered

**Expand `canary-core::App` into the game runtime.** Rejected. `canary-core`
is a lower-level crate and intentionally has no platform, schedule, render,
audio, UI, or plugin dependencies. Teaching it those concepts would reverse
the composition direction and make headless core use pay for higher layers.

**Keep composition in a binary or ask every game to wire subsystems
manually.** Rejected. A private binary cannot be a supported game API, and
duplicated phase order would make tick ownership, error handling, and plugin
access vary by application.

**Add a new `canary-app` or `canary-game` crate now.** Deferred. It would
create a second public layer before a second consumer has shown what should
live there. The existing `canary-runtime` package already occupies the
upward composition position and can expose its behavior as a library.

**Give each Tier A instance a separately owned or copied persistent
`World`.** Rejected. That cannot mutate or observe the active game state,
which is the purpose of R-34. The implementation's temporary ownership loan
is the same active world, reclaimed after each synchronous guest call.

**Run a plugin concurrently with schedule systems using declared
`SystemAccess`.** Rejected for the first runtime. The declarations are
manual and unchecked (R-24), so they cannot prove that the plugin and
systems access disjoint data. Exclusive, synchronous phase boundaries are
the current safe contract.

**Store a raw pointer or unchecked borrowed reference to `World` in
Wasmtime state.** Rejected as an architectural contract. A plugin's
authority and lifetime must be bounded at the host-call boundary; the
implementation must choose a safe mechanism and demonstrate cleanup on
guest traps before it can ship.

## Consequences

- The existing `canary-runtime` package now has a library role while
  `canary-core::App` remains usable for simple subsystem-driven programs and
  tests. The full schedule/platform/presentation frame driver remains pending.
- Game, server, and test consumers share lifecycle and tick ownership. Their
  selected backends and optional presentation phases remain explicit.
- The runtime takes responsibility for cross-subsystem ordering and
  `RunContext`; subsystem registration alone is no longer treated as a
  complete game-frame contract.
- Tier A can affect the active game world only through capability-checked,
  synchronous host calls. This does not add a per-frame plugin callback or
  an editor extension API.
- The first implementation remains single-threaded at phase boundaries.
  Existing concurrent read-only scheduler stages continue within the
  scheduler; runtime/plugin overlap and concurrent writers are excluded.
- Runtime-level fixed-step execution, pause/resume, reload, replacement,
  multiple windows, and concurrent plugin calls remain later design work.

## Revisit conditions

- A real second consumer that cannot use `canary-runtime` without an
  unnatural abstraction may justify a separate application-composition
  crate.
- A proven typed system-access mechanism may justify relaxing exclusive
  plugin/world phase boundaries.
- A consumer that needs runtime pause, restart, replacement, or hot reload
  must define those lifecycle transitions in a new ADR before they are
  added to the public contract.
