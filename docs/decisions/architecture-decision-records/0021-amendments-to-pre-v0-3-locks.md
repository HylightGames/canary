# 0021. Amendments to the pre-v0.3 locks: owner review corrections to ADR 0020

**Status:** Accepted

## Context

ADR 0020 recorded final pre-v0.3 architectural locks after a full
review. Owner review rated it ~85–90% correct and identified seven
places where it over-constrained Canary or missed a foundation that
becomes painful later. Per this repository's append-only convention
(ADR 0010's own rule: supersede or amend explicitly, never silently
edit), this ADR amends ADR 0020. ADR 0020 stands except where this
document explicitly overrides it; where the two conflict, this document
wins.

The governing sharpening behind every amendment below: **lock semantic
contracts, not concrete mechanisms.** "Rendering must have an explicit
pass/resource dependency model before multi-pass rendering" is a
foundation; "Canary must use a render graph before v0.3" guesses the
implementation. Each correction follows that pattern.

## Amendment 1 — Asset identity: stable logical IDs, not content-derived identity (OVERRIDES ADR 0020 §11 and Constitution rule 12)

**What 0020 said:** "Asset identity is content-derived," with
content-hash `AssetId` plus cook-key rules.

**Why it is wrong:** the current `AssetId` (SHA-256 over file bytes
plus `LOADER_VERSION`, self-described as provisional in
`engine/canary-assets/src/id.rs:46`) conflates three identities every
established engine keeps separate (Godot persistent resource UIDs vs
imported data; Unity asset GUIDs vs artifact IDs; Unreal stable asset
identity vs derived artifacts with redirects). Editing `player.glb`
changes its `AssetId`, so scenes, prefabs, materials, editor
selections, marketplace dependencies, and authored metadata would
reference a different identity after every content edit — forcing a
"rebuildable hints" layer to reconstruct stable identity after the
fact, exactly once the editor exists and it is too late.

**Corrected rule:** Canary uses three identities.
`LogicalAssetId` answers "what asset is this" — stable across content
mutation, renames, and moves; it is what scenes, prefabs, materials,
selections, marketplace packages, and version control reference.
`ContentHash` answers "what exact content is this" — immutable,
hash-addressed source and derived bytes (deduplication falls out).
`CookKey` answers "what derived artifact should exist" — derived from
content hash + importer version + dependency hashes + platform +
settings, never paths. The existing content-hash machinery is retained
as the `ContentHash` layer, not discarded.

**Status:** foundation now (the vocabulary and the rule that
authoring references use logical IDs); logical-ID allocation/registry
with the `canary-state` milestone (v0.0.14), before the editor era.

## Amendment 2 — Scheduler: conflict semantics are frozen, solo-write is implementation (NARROWS ADR 0020 §5 and Constitution rule 5)

**What 0020 said (effect):** the greedy solo-writer staging
(`schedule.rs:38`, `execution-model.md:187` — read-only stages
concurrent, any writer solo) reads as permanent semantic law.

**Correction:** the frozen invariant is **no two systems perform
conflicting mutable access concurrently or without explicitly
established ordering** — i.e. at most one *active* writer per mutable
location at a time, with multiple writes per tick legal under explicit
schedule ordering (ownership transfer: physics → gameplay correction →
animation). The current solo-writer behavior remains a correct and
safe *implementation*. This preserves today's scheduler while leaving
the door open to concurrent disjoint writes and ordered sequential
writes without a "semantic law" change later.

## Amendment 3 — Authority wording: authoritative vs predicted state (NARROWS ADR 0020 §7 and Constitution rule 10)

**What 0020 said:** "clients never write replicated state."

**Why it bites:** prediction requires the client to simulate and write
its *local predicted replica* (server snapshot 100, predicted 103),
then reconcile — Unreal's own model has clients simulate
approximations of authoritative state. The absolute wording would make
rollback/prediction architecturally awkward.

**Corrected rule:** authoritative replicated state has a declared
authority owner; clients may maintain and write predicted/local
replicas but cannot commit authoritative state without authority.
Server-owned truth and client-held prediction are different state with
different write rights, and the architecture names both.

## Amendment 4 — Input/action model foundation: new pre-v0.3 MUST (ADDS missing foundation)

**Gap verified:** no `InputAction`/`InputMap`/mapping concept exists
anywhere (`grep` over `engine/` and `docs/architecture/` returns
nothing); the platform layer normalizes OS input into engine events
but raw press and gameplay intent (`MoveForward = +1`) are the same
undifferentiated thing. Godot (`InputMap`) and Unreal (Enhanced Input:
Actions, Mapping Contexts, Modifiers, Triggers) both separate these
layers — and Canary needs the separation *more*, because replay,
rollback, network commands, input recording, AI-driven players,
headless tests, split-screen, and controller remapping must all
consume one logical input representation.

**Locked now (vocabulary + boundary, not a system):**
`RawInput` → `InputMapping` → `InputAction` → `PlayerInput` →
`SimulationInput`, with a defined crossing point into the simulation
so frame-tagged commands (Amendment 7's input-vs-state shape) carry
actions, never scancodes. Device drivers, remapping UI, and gesture
layers are deferred; the layering is not.

## Amendment 5 — Simulation snapshot contract, not generic save/load (SHARPENS ADR 0020 §8)

**What 0020 said:** "whole-world save/load/checksum."

**Correction:** rollback/replay need a formally defined **simulation
state boundary**, not world serialization in general:

```text
SimulationState: ECS entities/components, deterministic resources,
RNG state, simulation clocks, relevant subsystem state,
schema/version information.
Explicitly excluded: GPU resources, window/OS handles, audio device
state, editor state, thread-pool internals, filesystem handles,
temporary caches.
```

Contract shape: `snapshot()` / `restore()` / `checksum()` /
`step(input)` — one foundation under rollback, replay, save games,
desync detection, server snapshots, networking, and testing. (The
"how do I serialize a World containing a Vulkan device handle"
problem disappears once the boundary, not the whole `World`, is the
unit.) Save-game UX remains a separate, later concern.

## Amendment 6 — RNG stream ownership invariant (SHARPENS ADR 0020 §13)

**Why "RNG-in-state" is insufficient:** with concurrent systems,
`A→rng.next(); B→rng.next()` yields different sequences under
different execution orders. The locked invariant is therefore about
*ownership*, not mere presence: **randomness consumed by deterministic
simulation comes from explicitly owned deterministic streams, never
from ambient/global RNG state** — e.g. `WorldRng` plus named streams
(`SystemRng("physics")`, …) or streams derived from
`WorldSeed + StableSystemId + Tick (+ EntityId)`. The exact mechanism
waits for the scheduling-parallelism work; the invariant does not,
because it constrains that work's design space.

## Amendment 7 — Tick ownership: the runner owns advancement, the scheduler consumes context (SHARPENS ADR 0020 §17)

**Rejected:** any reading under which `Schedule` owns simulation time.
A scheduler executes work; with future fixed-simulation, extraction,
editor, networking, loading, and worker schedules — some running
multiple times per outer frame — time ownership inside the executor
would give every schedule its own incompatible "tick."

**Accepted:** single tick owner per simulation run, made concrete as a
`RunContext` (logical tick, simulation time, frame number, delta, run
identity) owned and advanced by the App/simulation runner and consumed
by schedules. ADR 0020 §17 already stated the scheduler does not own
the tick; this amendment makes the positive shape explicit so future
schedules are designed against it.

## Amendment 8 — Command/event semantics promoted to MUST (PROMOTES part of ADR 0020 §8)

Command/event channels are broader than networking prerequisites:
parallel ECS structural mutation, spawn/despawn, UI→gameplay,
physics→gameplay→audio/particles, network→simulation, and
editor→runtime all need the same contract (Bevy's deferred commands +
message infrastructure is the precedent for convergent evolution, not
a template). Locked now as **semantic contract**, not mechanism:

```text
Command  — structural mutation (applied at defined points)
Event/Message — transient communication (never persistent state)
Resource — persistent shared state
```

with defined answers for: same-tick vs next-tick delivery, ordering,
multi-producer/multi-consumer, determinism, retention, droppability.
Mechanism and syntax wait; semantics do not.

## Amendment 9 — Schema migration policy (EXTENDS ADR 0020 §16)

"Versioned" alone degrades into "a number next to the blob." Before
the editor and project-state ecosystem exist, Canary locks the
migration vocabulary: `Schema ID + Schema Version + Encoding Version
+ Migration Path`, with decided behavior for field added / removed /
renamed, type changed, component split / merged, and unknown vs
missing fields — across components, assets, scenes, projects,
networking, plugins, saves, and editor data. Concrete migrators are
written per subsystem as formats stabilize; the policy is what must
not be invented five different ways later.

## Amendment 10 — Boundary rule restated: stable-external strict, internal ergonomic (NARROWS ADR 0020 §7 and Constitution rule 7)

**What 0020 said:** "nothing crosses a boundary as a Rust type,
everywhere, always" — elegant, but it would force serialization
layers around ordinary internal calls and contradicts Canary's own
intentional `glam` boundary.

**Corrected rule:** stable external boundaries (plugin ABI, network
protocol, persistent file formats, project serialization, long-lived
asset formats) must not depend on unstable or private implementation
types. Internal engine crate APIs freely use ergonomic Rust types
(`Vec3`, `Transform`, `Result`, `&World`) where those types are part
of the intended contract. Strictness follows boundary stability, not
philosophy.

## Amendment 11 — Component lifecycle contract: foundation now (ADDS missing foundation)

**Gap verified:** no lifecycle hooks or observers exist in
`canary-ecs` (search returns nothing). The removal log (ADR 0020 §8)
covers *notification*, but the larger question is ownership: when a
`PhysicsBody`, `Renderable`, `NetworkIdentity`, or `AudioEmitter`
appears, who creates the associated external resource, and when a
component is removed or its entity despawned, who destroys it.

**Locked now:** the lifecycle contract — defined behavior for entity
spawned, component added/changed/removed, entity despawned, resource
inserted/removed, world reset/cloned/restored — stating for each
transition who owns associated external state and when it is
created/destroyed. Observer machinery itself is deferred; the
contract it will implement is not, because physics, rendering,
audio, and networking components all need sidecar management and
would otherwise invent four incompatible answers.

## Amendment 12 — RenderGraph data model before multi-pass (AGREED, conditioned)

Agreement with ADR 0020 §9 plus one condition: before the first
genuinely multi-pass renderer, lock the RenderGraph **data model**
(`Pass`, `Resources`, `Read`/`Write`, `Dependency`, `Queue`) — not
the optimizer, aliasing, barriers, async-compute scheduling, or frame
compiler. Progression: RHI now → thin pass declaration with v0.1 →
graph with v0.2 → compilation later. This is safer than either
extreme (giant compiler now vs. nothing until rewrite pressure).

## Consequences

- ADR 0020's Constitution rules 5, 7, 10, 12 are superseded by
  Amendments 2, 10, 3, 1 respectively; new constitutional rules
  follow from Amendments 4 (input layering), 5 (snapshot boundary),
  6 (RNG streams), 8 (command/event/resource semantics), 9
  (migration policy), 11 (lifecycle contract), and 7 (RunContext).
- v0.0.14 (`canary-state`) inherits two more prerequisites: logical
  asset IDs (Amendment 1) and the migration policy (Amendment 9).
- The input/action vocabulary (Amendment 4) constrains the audio/UI
  milestones (positional triggers, HUD input) and all of §8's
  command shapes — it should be written alongside the next
  simulation-adjacent milestone, not left for the editor era.
- Nothing in this ADR changes code, milestones, or shipped behavior;
  it changes what future code is allowed to assume.

## Implementation status

All eleven amendments are rule-level: no implementation milestone is
created here. Allocation guidance: Amendments 1, 4, 8, 9, 11 belong
with `canary-state`/simulation-adjacent work (pre-editor);
Amendments 2, 3, 6, 7 constrain scheduler/networking work already
sequenced; Amendments 5, 10, 12 are wording-level corrections
effective immediately at review time.

## Revisit conditions

- If logical-ID allocation proves unworkable without the editor's
  asset browser, the *registry mechanism* may be deferred — but the
  three-identity rule itself may only be revisited by a new ADR with
  counter-evidence from a shipped content workflow, not by
  implementation convenience.
- If RunContext proves insufficient for some future schedule kind,
  extend the context shape; do not move time ownership into the
  executor.
