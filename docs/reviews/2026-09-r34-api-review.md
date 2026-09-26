# API Review: R-34 Scoped Tier A World Access (ADR 0024 Acceptance Gate)

**Status:** Accepted as the implementation contract for R-34 scoped
access. The four decisions, RunContext delivery, and generation ruling
below are binding on the implementation; deviations require an ADR
amendment, not a code comment.
**Scope:** Tier A (`wasm32-wasip2`, wasmtime-pinned) scoped access to the one active
game `World` at explicit runtime phase boundaries, covering the existing
`on_load` / `on_unload` callbacks only. No per-frame plugin hook, no concurrent
plugin/world access, single-threaded phase boundaries, minimal surface.
**Builds on (not re-litigated):** ownership-loan shape (`Option<World>` take() into
`Store<Slot>`, no borrow alive during `Func::call`, move home after); structural
capability withholding per interface; trap cleanup with limits intact; reentrancy
(nested reads safe, nested loans refused, scheduler overlap structurally `None`);
quiet-tick probe semantics (reads stamp nothing; same-tick writes need a baseline
predating phase advance; `insert`/`insert_resource` stamp unconditionally).
Settled rejections respected throughout: no raw pointers, no concurrent access,
no compat runtime.

## 0. Source ground truth (read fresh for this review)

- ADR 0024 (Proposed): composition in `canary-runtime` lib; runtime owns one active
  `World`, schedule execution, `RunContext`, phase order, teardown; tick exactly
  once immediately before each scheduled world-mutating pass; Tier A access only
  through granted host interfaces at explicit exclusive boundaries; fallible loader
  boundary must be designed before plugin loading is a required service.
- `docs/architecture/runtime-composition.md`: scoped-access + errors sections are
  binding on behavior; type names, builder shape, context delivery, and the safe
  Wasmtime host-state mechanism are open — this review closes them.
- ADR 0021 Amendment 7: `RunContext` (logical tick, simulation time, frame number,
  delta, run identity) owned/advanced by the runner, consumed by schedules. Note:
  **no `RunContext` type exists in code today** — it is vocabulary only. This
  review proposes the type (§5).
- ADR 0021 Amendment 8 + ADR 0022 Clarifications 1/2: `SimulationInput` (intent
  entering a step) vs `Command` (structural mutation at defined points) vs
  simulation messages (deterministic, may affect sim via command path) vs
  observation events (notify only, never authoritative). Plugin `set` writes are
  **none of these channels** — they are direct host-mediated overwrites at a
  lifecycle boundary (see Decision 1 for why that is contained).
- ADR 0022 Clarifications 4/5 (sim/presentation boundary; asset identities) cited
  in the task as "4/5/8" — there is no Clarification 7/8 in the accepted ADR 0022
  text; the "8" is read as Amendment 8 command/event semantics. Flagged, not
  blocking.
- `engine/canary-plugin-api/src/tier_a.rs`: `HostState { world: World, ... }` owned
  by value (deliberate simplification); `Plugin::on_load/on_unload` infallible with
  trap→`warn`; per-entry refuel; structural per-interface linking.
- `engine/canary-ecs`: `insert` stamps `current_tick` unconditionally (both
  overwrite and archetype-move paths); `get_mut`/`resource_mut` stamp
  unconditionally; `set_erased` overwrite-only, `false` on dead/missing, panics
  only on concrete-type mismatch (internal invariant); `advance_tick` owned by
  runner; `Entity::from_raw_parts(u32, u64)`, generation `u64` deliberate.
- `engine/canary-transform/src/propagation.rs`: probe watches exactly
  `Transform`/`Parent`/`Children`/`GlobalTransform` change ticks + membership
  counts; same-tick rerun never skips (`change_tick() <= baseline.last_tick`
  forces recompute); every recompute leaves a follow-up pass due.
- Manifest pins `wasmtime` major 49 (`engine/canary-plugin-api/Cargo.toml`);
  Cargo.lock pins 49.0.1 per spike (manifest re-verified here; lockfile not
  re-read — trust spike).
- WIT (`engine/canary-plugin-api/wit/plugin.wit`): `entity-handle { index: u32,
  generation: u64 }` — **already matches `Entity` exactly**; see §6 ruling.

## 1. DECISION 1 — Write semantics: uniform immediate (no per-capability split, no atomic batch)

**Proposed rule:** every authorized `ecs-write.set` applies **immediately** to the
loaned `World` via the existing `set_erased` overwrite-only path, and is visible
to (a) subsequent host calls within the same guest invocation and (b) the next
runtime phase after the world moves home. Failures return `false` and change
nothing; there is no partial application because there is no multi-step
operation — `set` is a single overwrite. This rule is **uniform across the whole
Tier A surface**, not per-capability.

**Reasoning.**

- The only write op in scope is `set`, and it is already overwrite-only
  (no archetype moves, no spawn/despawn, no structural commands). Immediate
  application is therefore already atomic at the op level; a transaction layer
  would wrap single ops in ceremony with no new guarantee.
- `on_load` runs after world + codecs are ready but **before simulation starts**
  (before tick 0 / first `advance_tick`); `on_unload` runs after the final pass
  during teardown. Neither callback sits inside a schedule, beside a change-detection
  baseline, or on the command/event path (Amend 8 / Clarif 1–2), so immediate
  writes cannot cut across a delivery barrier or dirty an in-flight probe.
- Immediate-visibility within one invocation is required for guest coherence
  (`set` then `get` must read back the write); anything else would need a
  write-behind cache whose flush point is a new design for zero benefit.

**Alternatives rejected.**

- *Per-capability write modes* (e.g. immediate for some grants, deferred for
  others): rejected — there is exactly one write capability and one write op;
  the distinction would be dead surface.
- *Atomic batch / commit-at-return* (buffer all `set`s, apply on clean return,
  discard on trap): rejected — discards are already near-total (a trapped
  invocation's earlier `set`s surviving is the only divergence, and those
  writes went through the same authorized overwrite path, so they are
  policy-clean if data-dirty; the guest authoring contract should state
  "writes before a trap persist" rather than buy a journal). Revisit only if a
  future multi-op structural write (spawn/remove-component) enters Tier A scope.
- *Deferring plugin writes into the command channel*: rejected — commands are
  for in-simulation structural mutation with delivery ordering; lifecycle-phase
  overwrites pre/post-simulation have no ordering problem to solve.

## 2. DECISION 2 — Host-panic policy: catch at the host-function boundary, convert to trap, reclaim the loan; never propagate, never abort

**Proposed rule:** every Tier A host implementation (`ecs-read`, `ecs-write`,
and any future interface) wraps its body in `catch_unwind`; a caught panic
becomes a guest-visible trap error returned from `Func::call`, fuel/memory
limits stay armed on the `Store`, and the runtime reclaims the loaned `World`
from the `Store<Slot>` through the same normal-or-trap cleanup path. The trap
is then reported through the fallible loader boundary (§4): required plugin →
startup/shutdown fatal error; optional plugin → `warn` + continue unloaded.
The process never aborts for a plugin-phase panic, and a Rust panic never
unwinds through Wasmtime call frames.

**Reasoning.**

- wasmtime 49 propagates a host-function panic outward through the `Func::call`
  frame. Letting it propagate couples loan cleanup to unwinding through
  Cranelift/host frames — exactly the "explain trap cleanup" burden ADR 0024 /
  runtime-composition.md places on this review. Catching **inside** the host fn
  (before any Wasmtime frame) keeps the failure a plain `Err`, so the proven
  trap-cleanup path (limits intact, `Store` still owned by the runtime, world
  moved home) applies unchanged.
- Abort-on-panic would violate the binding teardown contract (reverse-order
  cleanup, primary failure preserved, cleanup continues) for a failure that is
  by construction recoverable: the `World` is owned by value in the `Store`,
  not borrowed, so a panicked host fn leaves no aliasing to be afraid of —
  only a value to move home.
- This composes with the existing code posture: host fns already avoid
  `unwrap` on guest-influenced paths (`set_erased` returns `bool`; unknown
  schema → `None`/`false`); the `catch_unwind` is a backstop for genuine bugs,
  and the one intentional panic (`set_erased` concrete-type mismatch =
  codec-registry bug, not guest input) becomes a loud trap rather than an
  unwind.

**Alternatives rejected.**

- *Abort the process on host panic*: rejected — denies reverse-order cleanup
  and contradicts the runtime-composition error contract for a survivable
  condition.
- *Let the panic unwind through Wasmtime (document and rely on 49's
  propagation)*: rejected — relies on version-specific unwinding through
  generated frames and makes loan reclamation depend on unwind-table behavior
  instead of ownership; brittle across wasmtime upgrades.
- *`panic = "abort"` build posture for the host*: rejected — same as abort,
  plus it would punish all engine users for one boundary's hygiene.

Proposed signature (host side, in `canary-plugin-api`):

```rust
/// Outcome of one guarded host-function body.
pub(crate) fn guard_host_call<T>(
    plugin: &str,
    op: &'static str,
    body: impl FnOnce() -> T + std::panic::UnwindSafe,
) -> Result<T, wasmtime::Error>;
```

(runtime maps the `Err` to `PluginError::GuestTrap`, §4; exact error type
kept behind `PluginError` per the no-third-party-types rule.)

## 3. DECISION 3 — Tick ownership at plugin phases: plugin phases get NO `advance_tick`

**Proposed rule:** the runtime advances the ECS tick **exactly once immediately
before each scheduled world pass**, and **never** at a plugin phase. Concretely:

- `on_load`-with-world runs after world + codecs are ready, **before** the
  first `advance_tick`. Its `set` overwrites stamp the pre-first-tick
  (`change_tick()` at world creation); they are therefore visible to the first
  pass's `query_changed_since` baselines captured at/after tick 1 — no
  invisibility hazard.
- `on_unload`-with-world runs during teardown, after the final pass, with no
  tick advance. Its writes are visible only to subsequent teardown observers;
  no schedule runs after it, so no probe can miss them.
- The scoped-invoke entry point **debug-asserts** `world.change_tick()` is
  identical before the loan and after the world moves home (writes may stamp
  component ticks; the tick *counter* must not move). A plugin boundary never
  calls `advance_tick`; a guest cannot call it (no such host function is
  linked, under any capability).

**Reasoning (quiet-tick constraint).** The transform probe treats
`change_tick() <= baseline.last_tick` as dirty-forces-recompute, and every
recompute leaves a follow-up pass due. Granting a plugin phase its own tick
would mint a tick with no schedule pass attached: the recompute-then-follow-up
accounting would see a phantom advance, and the "one tick per scheduled world
pass / no tick on event or presentation-only frames" invariant (ADR 0024 §4,
runtime-composition.md time-ownership section) would break at the first
headless run that loads a plugin but schedules nothing. The spike's
quiet-tick finding cuts the same way: a same-tick write needs a baseline that
predates the phase advance — with no advance at all, plugin writes simply
stamp the current tick and the *next real pass* (with its own advance +
follow-up discipline) observes them through the normal `>` probe. No special
casing required.

**Alternatives rejected.**

- *One `advance_tick` per plugin phase ("plugin ticks")*: rejected — phantom
  ticks with no pass; breaks the per-pass tick identity the fixed-step future
  depends on; forces every change-detection consumer to reason about
  non-simulation ticks.
- *Advance only around write-granted invocations*: rejected — tick would then
  depend on granted capabilities, making tick identity a function of plugin
  configuration rather than schedule execution; same phantom-tick harm, less
  predictably.
- *Letting plugins request a tick via a host call*: rejected — time ownership
  leaves the runner; contradicts Amend 7 outright.

## 4. DECISION 4 — Fallible loader boundary: keep `Plugin` infallible, add a runtime-side fallible service seam

**Proposed rule:** `Plugin::on_load/on_unload` stay infallible (Tier B C-ABI
cannot report failure; changing the trait would leak a Tier A concern into the
native tier). The runtime does **not** call those hooks directly for required
services. Instead, `canary-plugin-api` gains a small fallible seam used by the
runtime's plugin-loading service, and `PluginError` gains one variant. Optional
plugins degrade to `warn` + continue; required plugins fail startup (or stop
the loop during teardown-time unload failures per the fatal-phase rule).

**Reasoning.** runtime-composition.md errors section is explicit: "before
registering plugin loading as a required runtime service, decide how a
loader-level error reaches the runtime and how optional plugins are reported."
The current trap→`warn` exists because the *trait* is infallible, not because
swallowing is desired policy. The fix is a second entry point with a `Result`,
not a trait change — Tier B keeps working untouched, and the runtime gets a
typed failure it can feed into reverse-order cleanup without inventing a new
error taxonomy.

**Alternatives rejected.**

- *Make `Plugin::on_load -> Result<(), PluginError>`*: rejected — Tier B's C
  vtable entry cannot produce that error; every native plugin and the ABI
  contract would churn for a Tier A need.
- *Keep trap→warn for required plugins too*: rejected — a required service
  that silently fails to initialize violates "required-service initialization
  failure aborts startup" outright.
- *Panics for loader failures*: rejected — panics are not typed failures and
  bypass the primary-vs-cleanup-failure accounting.

Proposed signatures (new, in `canary-plugin-api`; `canary-runtime` consumes):

```rust
/// Which lifecycle callback trapped. Reported, never used for control flow
/// by the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginPhase { OnLoad, OnUnload }

/// How the runtime registered this plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRequirement { Required, Optional }

/// Fallible scoped-invoke outcome for one plugin callback.
impl WasmComponentPlugin {
    /// Invoke `phase` with the loaned active world (see §5).
    /// Returns `Ok(())` on clean return; `Err(GuestTrap{..})` on guest
    /// trap or caught host panic (§2). Never swallows: no logging here —
    /// the caller decides warn vs abort by `PluginRequirement`.
    pub fn call_scoped(
        &mut self,
        world: &mut Option<World>,
        grant: &ScopedGrant,
        phase: PluginPhase,
    ) -> Result<(), PluginError>;
}

/// Loader-level reporting for one plugin registration.
#[derive(Debug)]
pub enum PluginOutcome {
    Loaded { name: String },
    /// Optional-only: trap/logged cause retained, run continues.
    SkippedOptional { name: String, cause: PluginError },
}

impl WasmPluginLoader {
    pub fn load_scoped(
        &self,
        name: impl Into<String>,
        capabilities: &HashSet<Capability>,
        requirement: PluginRequirement,
    ) -> Result<PluginOutcome, PluginError>;
    // Required + trap → Err(GuestTrap) propagates to runtime startup abort.
    // Optional + trap → Ok(SkippedOptional{..}) + tracing::warn!.
}
```

`PluginError` addition (boxed source preserved, no wasmtime type leak):

```rust
#[error("Tier A plugin `{plugin}` trapped during `{phase:?}`: {source}")]
GuestTrap {
    plugin: String,
    phase: PluginPhase, // or &'static str to avoid cross-crate enum churn — review pick
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
},
```

Open pick left for implementation review: `phase: PluginPhase` (typed) vs
`phase: &'static str` (avoids exporting a new enum). Prefer the enum; it is
three lines and matches the typed-errors convention.

## 5. Concrete types + builder shape (minimal surface)

Ownership backbone (spike-proven; named here for the first time):

```rust
/// Per-invocation host view. Owns the loaned world for exactly one guest call.
/// `world` is `Some` only between loan and reclaim; the runtime's `Option`
/// is `None` for that window, so no schedule can start (there is nothing to
/// schedule against) and no second loan can issue (nested loan finds `None`
/// and is refused). No borrow exists across `Func::call` — only owned values
/// moved in and home again.
pub(crate) struct WorldSlot {
    world: Option<World>,       // loaned active world; None when home in runtime
    codecs: Arc<CodecRegistry>, // fixed at loader construction; never per-invocation
    grant: ScopedGrant,         // capability subset actually linked for this instance
    limits: StoreLimits,        // memory limiter, re-armed per store
    fuel: u64,                  // per-entry budget, re-armed before each call
    /// Reentrancy + tick guard state.
    depth: u32,                 // >0 inside a host call; nested loan refused while >0 for writes
    tick_before: Option<Tick>,  // debug-asserted equal after reclaim (§3)
}
```

Capability views (structural withholding retained — the linker links per
interface exactly as today; these types are the host-side documentation of
what each linked interface may touch):

```rust
/// What this instance may do with the loaned world. Decided once at
/// `load_scoped`; enforced structurally by linker wiring, mirrored here
/// so host fns and audits read the same truth.
#[derive(Debug, Clone)]
pub struct ScopedGrant {
    pub capabilities: HashSet<Capability>, // subset of {ReadEcsWorld, WriteEcsWorld}
    pub budget: ResourceBudget,            // fuel + memory, re-armed per entry
}

impl ScopedGrant {
    pub fn can_read(&self) -> bool;
    pub fn can_write(&self) -> bool;
}
```

Invoke entry point (single choke point; serializes against the schedule by
construction — the caller holds `&mut Runtime`, and the schedule is not
re-entered while it runs):

```rust
impl Runtime {
    /// Invoke one plugin lifecycle callback with scoped active-world access.
    /// Exclusive with schedule execution: takes `&mut self`, loans
    /// `self.world` (leaving `None`), runs the guest call to return-or-trap
    /// (limits armed, host panics caught per §2, tick counter asserted per §3),
    /// moves the world home, and maps failure per `PluginRequirement` (§4).
    /// No world borrow or handle escapes: host fns hand out only values
    /// (u32 counts, bools, owned `ComponentValue`s), never references.
    pub fn call_plugin_scoped(
        &mut self,
        plugin: &mut WasmComponentPlugin,
        phase: PluginPhase,
    ) -> Result<(), RuntimeError>;
}
```

Builder + pre-run configuration window (exact wording proposed for the
runtime docs):

> **Configuration window.** The consumer assembles everything the run needs
> **before** `start`: populate the `World` (spawn, insert, register components,
> insert resources including `RunContext`'s initial value — see below),
> register codecs with the `WasmPluginLoader`, declare each plugin's
> `ScopedGrant` + `PluginRequirement`, and select platform/render/audio/UI
> backends. `RuntimeBuilder::build(world, config)` **moves** the world in;
> after `build` returns, the runtime is the world's single owner. During the
> run, consumer code touches the world only through phase callbacks the runtime
> invokes (simulation systems, scoped plugin calls, teardown observers). After
> `run` returns (or unwinds through cleanup), `Runtime::reclaim_world` hands
> the world back for inspection/testing.

```rust
pub struct RuntimeBuilder { /* world + backends + plugin registrations */ }

impl RuntimeBuilder {
    pub fn new() -> Self;
    /// Pre-run only: staged into the world before the runtime takes ownership.
    pub fn with_world_populator(mut self, f: impl FnOnce(&mut World)) -> Self;
    pub fn with_plugin(mut self, name: &str, grant: ScopedGrant,
                       requirement: PluginRequirement) -> Self;
    // … one `with_*` per optional backend (platform/render/audio/ui) …
    /// Moves the world in. After this call the runtime owns it.
    pub fn build(self, world: World) -> Result<Runtime, RuntimeError>;
}

impl Runtime {
    pub fn run(&mut self) -> Result<RunReport, RuntimeError>;
    pub fn reclaim_world(self) -> World; // after run / after failed build cleanup
}
```

`RuntimeError` shape (typed, primary-vs-cleanup accounting per the errors
contract; sketch — full variant list at implementation review):

```rust
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("required service `{service}` failed to initialize: {source}")]
    ServiceInit {
        service: &'static str,
        #[source] source: Box<dyn std::error::Error + Send + Sync>,
        /// Cleanup failures retained as secondary context; never replace `source`.
        #[source] cleanup: Vec<Box<dyn std::error::Error + Send + Sync>>,
    },
    #[error("fatal error in {phase:?}: {source}")]
    FatalPhase { /* …same secondary-cleanup shape… */ },
    #[error("plugin `{plugin}` trapped during `{phase:?}`: {source}")]
    PluginFailed { plugin: String, phase: PluginPhase, #[source] source: PluginError },
}
```

### RunContext delivery: **ECS resource (read-only by convention), NOT thread-local, NOT a schedule-run argument**

```rust
/// Owned and advanced by the runtime; consumed (read) by schedules/systems.
/// Inserted before the first pass; overwritten once per outer frame BEFORE
/// `advance_tick`. Systems and scoped plugin host fns read it via
/// `World::resource::<RunContext>()`; nothing but the runtime writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunContext {
    pub run_id: u64,        // unique per `Runtime::run` execution
    pub frame_index: u64,   // outer-loop iterations incl. event/presentation-only frames
    pub tick: Tick,         // ECS tick of the current (or most recent) scheduled pass
    pub frame_dt: Duration, // wall-clock outer-frame time; NOT simulation dt
    pub sim_time: Duration, // accumulated simulation time (physics substeps fold in here)
    pub sim_step: Duration, // simulation-step duration where a runner provides one
}
```

**Justification, including the quiet path.** Thread-local delivery is rejected:
hidden per-thread global state, untestable without running the loop, and a
reentrancy trap the moment a host call nests (the exact hazard §2 contains).
Schedule-run-argument delivery is rejected *for v0.0.13*: it churns every
system-fn signature (`fn(&mut World)` → `fn(&mut World, &RunContext)`) for a
benefit (compile-time presence) that a resource read already provides at one
call site per consumer. Resource delivery preserves the quiet path **provably**:
the transform probe watches exactly `Transform`/`Parent`/`Children`/
`GlobalTransform` change ticks plus membership counts — a per-frame
`insert_resource::<RunContext>` stamps the *resource's* tick unconditionally,
which no probe reads (resource change ticks are only visible to
`resource_changed_since::<RunContext>`, which nothing in the quiet path
queries). Frame/event-only passes overwrite the resource and advance
`frame_index` without touching the tick counter, satisfying "no tick on
event/presentation frames" while keeping `frame_index` monotonic for
diagnostics. Migration path noted: if a future schedule kind needs the context
where no `World` is at hand, add the argument *alongside* the resource (writer:
runtime; readers: either) — no contract break.

## 6. Generation-truncation ruling: **no truncation exists; keep it that way**

The task premise ("WIT handles are (u32,u32)") does not match the file on
disk: `wit/plugin.wit` declares `entity-handle { index: u32, generation: u64 }`,
exactly mirroring `Entity { index: u32, generation: u64 }`, and
`from_wit_entity` passes both through without cast. **Ruling:** retain the
`(u32, u64)` WIT shape permanently. `generation: u64` is deliberate
(architecture review Finding 4.2: u32 wrap on hot-recycled slots is a real
aliasing risk over server uptimes); narrowing WIT to `u32` would reintroduce
exactly that fault line at the trust boundary, where a wrapped generation
could alias a live entity — a soundness-adjacent staging bug no capability
check can catch. Any future handle compaction (e.g. packing for a dense wire
format) must be a *separate, explicitly versioned encoding* with
widening-on-decode, never a narrowing of `entity-handle`. No code change
required; record this ruling in the implementation PR as the
entity-handle compatibility note.

## 7. Acceptance-evidence mapping (what the implementation must show)

Each item traces to ADR 0024 / runtime-composition.md §"review and acceptance
evidence", with the mechanism this review assigns:

| Evidence | Mechanism in this review |
|---|---|
| Game + headless share the library | `canary-runtime` lib + `RuntimeBuilder`; headless binary becomes a consumer (§5); presentation phases omitted when backends absent |
| Observable phase order | `run` executes Initialize → Pump → Route → Simulate → Observe → UI/Extract → Render/Present → Pace; order logged/documented; stop prevents next simulate pass |
| Runtime advances tick once per pass | `advance_tick` called only in Simulate, immediately before `Schedule::run`; §3 asserts no tick at plugin phases |
| No tick on event/presentation frames | `frame_index` advances, `tick` does not; `RunContext` resource records both (§5) |
| Reverse-order cleanup, primary preserved | `RuntimeError::{ServiceInit,FatalPhase}` secondary-cleanup shape; `App::shutdown_all` panic-safety as minimum (§4 sketch) |
| Stub-ordered plugin proof (scoped R/W, denial structural) | `call_plugin_scoped` + `ScopedGrant`; today’s instantiation-denial tests extend to loaned-world `get`/`set` with grant matrices |
| No overlap; no escape | `Option<World>` loan leaves `None`; single `&mut Runtime` choke point; host fns return owned values only; nested-write-loan refused via `depth` |
| Live consumer thread/surface ordering | game consumer pumps platform on its required thread; surface ordering alongside input/UI placement demonstrated live |

## 8. Deviations from ADR 0024 (flagged, none silent)

1. **None on binding behavior.** Phase order, time ownership, exclusive
   boundaries, single-threaded phase edges, no per-frame hook, failure
   semantics — all retained verbatim.
2. **Terminology note (not a deviation):** the task's "ADR 0022 Clarifications
   4/5/8" does not exist as numbered — accepted ADR 0022 has Clarifications
   1–6. This review maps the intent to Clarif 1 (input chain), Clarif 4
   (snapshot/simulation boundary), Clarif 2 + Amend 8 (command/event
   semantics), and proceeds. If the task meant a different "8", that mapping
   needs correction before implementation.
3. **WIT premise correction (not an ADR change):** per §6, no (u32,u32)
   truncation exists on disk; the ruling is "keep (u32,u64)", which *upholds*
   the entity-design intent rather than changing it.
4. **One deliberate concretization where ADR 0024 stays open:** ADR 0024 §5
   says scoped access "applies to the existing `on_load`/`on_unload`
   callbacks" — this review confirms that binding and additionally rules that
   **no new callback (no per-frame hook, no editor-panel API)** ships in R-34,
   so the `PluginPhase` enum is closed to `{OnLoad, OnUnload}` until a later
   ADR opens it. That is a "smallest usable API" reading of the ADR, not an
   extension of it; expanding the enum later requires an ADR per the revisit
   conditions.

## 9. Explicit non-goals restated (R-34 ships without)

Per-frame plugin hooks; concurrent plugin/world execution; pause/resume,
restart-in-place, hot reload, subsystem replacement; multi-window; generic
fixed-step game runner (physics keeps its accumulator); renderer recovery
policy beyond the renderer's typed contract; Tier B world pointers; raw-pointer
or mutex-based loan internals (ownership loan only).
