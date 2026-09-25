# 0020. Pre-v0.3 architectural locks: what must be irreversible before the editor era

**Status:** Accepted

## Context

A September 2026 external dossier ("too late to fix later" architecture,
comparing Godot/Unity/Unreal) proposed ~16 foundational commitments
before v0.3.0: replaceable subsystems, a stable engine ABI, component
architecture, world-model separation, scheduler, threading, networking,
render graph, content-addressed assets, streaming, determinism,
reflection, plugin lifecycle, editor separation, and capabilities.

This ADR records the final decision on each, made as a full review:
repository investigation (crate graph, ECS, scheduler, render, physics,
assets, plugins, networking/reflection/editor-absence searches),
external research (Godot/Unity/Unreal entanglement and retrofit costs;
Bevy/Flecs/EnTT, Jolt/Rapier, Granite/bgfx/O3DE, content-addressed
pipelines, GGPO-style rollback prerequisites), and reversibility
analysis per proposal. The standard applied throughout: can this safely
become long-term architecture, or would it create debt that is extremely
expensive to remove later — and if the latter, is the *foundation*
needed now even when the implementation is not.

Two scoping facts constrain every decision below. First, Canary already
decided large parts of the dossier: server-authoritative replication as
an ECS concept over QUIC (ADR 0007), stable schema identity (ADR 0010),
versionable state graph + collab topology (ADR 0012/0013), change
detection as shared primitive (ADR 0014), replaceable-subsystem traits
for render/physics/plugins (ADR 0004→0016, physics backend trait, Tier
A/B loaders), and an explicit scheduler (v0.0.8). Second, the v0.1.0
plan explicitly defers prediction/rollback netcode, full asset cooking
with hot reload, and the editor — so several dossier items resolve to
"foundation now, implementation later" rather than new scope.

Definitions used below: **foundation** = a contract, invariant, or
identity rule other systems may depend on; **implementation** = working
code behind such a contract. Only foundations are locked here.

## Decision

### Accepted as already-true — locked by constitution (no new code)

1. **Replaceable subsystems.** The 13-crate graph holds: leaves
   (`canary-ecs`, `canary-platform`, `canary-render`, `canary-core`,
   `canary-loc`) depend on no other Canary crate; composition happens
   upward in `canary-runtime`. Locked with three pinned, contained
   exceptions: `glam` in graphics-facing signatures (design choice),
   `fluent`/`unic-langid` in `canary-loc`'s public API (a real lock —
   contained by keeping all other backend types out), `winit` error
   types behind the off-by-default `winit-backend` flag.
2. **Component/entity world model.** Archetype storage, generational
   `Entity` (u32 index / u64 generation), `Parent`-walk hierarchy with
   `Children` as external metadata, `SCHEMA_ID` vs `TypeId` split with
   erased overwrite-only access. Entity ID widths, the aliveness rule,
   and the schema-identity rule are frozen.
3. **World-model / gameplay separation.** The `World` contains entities,
   components, systems, and resources — no `GameObject`/`Actor`/`Node`
   with baked-in semantics. Built-ins arrive as components and systems,
   never as commandments.
4. **Scheduler conflict semantics.** Declared `SystemAccess`
   (component/resource split), greedy registration-order staging,
   solo-writer rule, `Read`/`Write` signature split. The semantics are
   frozen; packing optimality and representation are not.
5. **Threading contract.** `World: Send + Sync`, declared access,
   join-before-next-stage, writers-solo. The thread *pool* is an
   implementation detail and stays deferred (v0.0.8 deferral stands).
6. **Editor/runtime separation.** No editor exists; the runtime is
   headless-clean. The editor will be a client of engine APIs (built
   toward the plugin-host dogfood model), never a dependency of them.

### Accepted with modifications — foundation now, implementation later

7. **Stable engine ABI.** The dossier's full language-neutral runtime
   ABI is correctly sequenced *with* state serialization (v0.0.14) and
   networking (v0.0.15), not before them. Locked now: no Rust types
   across any boundary (existing `ABI_VERSION` vtable + WIT seams are
   the pattern), additive-only growth via extension queries. A full
   stable ABI over everything is deferred, not abandoned.
8. **Networking foundations.** The ADR 0007 model stands unchanged.
   Locked before v0.3 (they gate v0.0.14/15): a removal/destruction
   log alongside change ticks (R-33 — `query_changed_since` cannot
   report a gone entity), whole-world save/load/checksum + headless
   `step(inputs)`, canonical iteration order + RNG-in-state, an
   input-vs-state command shape, and Tick-vs-causality resolution
   (R-32). Transport, prediction, reconciliation, and rollback are
   implementations and stay sequenced where the plan puts them.
9. **Render graph direction.** The 8-method RHI is frozen additive-only
   (it is a floor, not a ceiling). Locked now: the *direction* —
   pass I/O declaration (read/write/RMW) over named logical resources
   and a record-vs-submit split — must be the RHI's evolution path so
   a compiling graph never requires rewriting call sites (the
   Unity/Unreal rewrite cost). The graph compiler, optimizer,
   transient pooling, and second backend are deferred. No half-graph
   compat surface may be invented in the meantime.
10. **Renderer capability + surface seams.** An adapter/limits
    capability query and a `Window→surface` seam (returning no
    third-party types) are locked before v0.3: windowed presentation
    (v0.0.13) needs format negotiation, and a second backend inherits
    whatever the trait assumes. Full cross-subsystem capability
    framework deferred.
11. **Asset cook contract.** Content hash identity + generational
    handles are done. Locked now as a rule the cooker must obey:
    derived-artifact keys are source-hash + importer-version +
    dep-hashes, never paths; references are ID-based with a
    rebuildable path-hint table. The cooker, cache, and streaming
    system are deferred.
12. **Streaming posture.** Locked now: references by ID (done) plus a
    residency/lifetime vocabulary (loaded/active/dormant/streaming/
    unloaded) as data states on the asset/world side. The streaming
    implementation is deferred.
13. **Determinism posture.** Single-machine scope stays honestly
    labeled. Locked now: RNG-in-state, canonical iteration order, and
    snapshot/checksum hooks. Cross-platform bit-identity proof
    (conformance suite, Jolt-style flag at measurable cost) deferred —
    the evidence shows it is expensive and needs its own milestone.
14. **Reflection posture.** Unreal-style arbitrary reflection is
    explicitly rejected (again). Locked now: nominal stable identity
    + registry + erased access, with no gameplay class required to
    opt in. Editor/inspector metadata vocabulary is deferred to the
    editor era.
15. **Plugin lifecycle rules.** Additive-only ABI growth and
    version-first-field checks are locked. Manifest format, Tier B
    signing/provenance, and safe hot-unload with full reclamation
    stay deferred (hot-unload becomes load-bearing for scripting
    iteration and must precede the editor, not v0.3).
16. **Serialization codec rule.** Locked now: wire/disk formats use
    schema identity + versioned codecs with migration rules (ADR 0012
    scope); internal Rust types are never the permanent format by
    accident. The exact codec is chosen when `canary-state` is built
    (v0.0.14), constrained by this rule.
17. **Three-clock doctrine.** Locked now: `Tick` (ECS ordering),
    `SimulationTime` (physics integration), wall-clock (App loop) stay
    distinct and never conflated; the scheduler does not own the tick.

### Rejected

- **Full render graph implementation pre-v0.3.** No consumer needs it
  yet; a premature graph becomes compat surface. Rejected as scope,
  accepted as direction (§9).
- **Second renderer / physics / network backend pre-v0.3.** The
  replaceability seams exist precisely so backends arrive when a
  consumer needs them (windowed presentation, 3D, transport). Building
  one now to "prove" the seam spends budget with no forcing function.
- **Cross-platform determinism guarantee.** Rejected on cost evidence
  (~8% + conformance burden); the honest single-machine scope plus
  locked hooks (§13) preserve the future.
- **GameObject-compatible convenience runtime.** Unity's dual-world
  cost (duplicate stacks, non-reversible bake, dual netcode) is the
  evidence. The plugin API speaks ECS from day one; the editor is a
  baker, never a parallel runtime.
- **Arbitrary (UObject-style) reflection.** Rejected; §14 covers why.

### Deferred with prerequisites stated

- Persistent scheduler thread pool (needs ADR-level decision; contradicts
  documented `thread::scope` design — prerequisite: pool-vs-scope
  measurement on real game systems, not microbenches).
- Spawn-with-components batching API (~28× measured; prerequisite: API
  design that doesn't disturb archetype invariants).
- Manifests, signing, hot-unload reclamation, cooker/DDC, rollback
  transport, visual scripting, marketplace — each sequenced in existing
  plans; none needs pre-v0.3 foundation beyond what is locked above.

## Evidence

- Repository: 15-crate graph audit (zero dependency-direction
  violations; zero third-party types in backend public signatures
  except pinned `glam`/`fluent`/`winit`-behind-flag); ECS audit
  (generational IDs, Tick(u64), `Send+Sync`, tick-preserving moves,
  Miri-clean unsafe); render audit (zero-dep RHI, `ash` confined to
  `pub(crate)`, error erasure, extraction bridge, no graph/passes/
  materials/swapchain, no capability query, no surface seam);
  physics/assets/plugins audit (leak-free traits, content-hash IDs,
  budgets, confinement, versioned vtable + WIT, fuel/memory bounds);
  absence searches (no networking code, no reflection system, no
  editor code).
- External: Godot scene-tree main-thread lock-in + server opacity;
  Unity GameObject/DOTS dual-world + dual-netcode + non-reversible
  bake costs; Unreal UObject/GC/reflection/replication entanglement +
  Epic's late-multiplayer warning; Bevy tick-pair + `last_run` +
  opt-in reflection + `RemovedComponents` event pattern;
  Flecs/EnTT relationship-as-data; Jolt/Rapier determinism scopes;
  Granite/bgfx/O3DE handle + pass-declaration boundaries; Unity
  GUID/sidecar + hash-keyed derived cache; GGPO save-state +
  headless-step + fixed-quanta prerequisites.
- Benchmarks (kept load-bearing, not re-argued): quiet-tick
  ~113–200×, bake reuse −31%, scope-spawn ~16–19µs/thread,
  archetype-move vs query ~130× gap, churn-retention cleared as
  allocator warming.

## Alternatives considered

- **Lock everything now (dossier maximalism).** Rejected: several
  🔴 items (full ABI, full graph, second backends, x-platform
  determinism) have no pre-v0.3 consumer; locking implementations
  without consumers creates compat surface, not safety.
- **Lock nothing beyond current code.** Rejected: tombstone logs,
  snapshot/restore, RNG discipline, pass-declaration direction,
  capability/surface seams, and cook-key rules genuinely lose
  cheapness once the editor, state serialization, and replication
  are built on top.
- **Adopt a compat runtime (GameObject-style) for approachability.**
  Rejected on Unity dual-world evidence.

## Consequences

- The Architectural Constitution (review report §7) becomes the PR
  review standard; violations require an ADR, not a comment thread.
- v0.0.14 (`canary-state`) and v0.0.15 (networking) inherit hard
  prerequisites from §8/§16 — they may not be descoped without
  revisiting this ADR.
- The RHI may grow only additively until the graph-direction design
  (§9) exists as a reviewed proposal.
- No `winit`/windowing type may enter a `Window`-trait signature;
  no backend-native type may enter a trait public signature; both
  are now constitution-level, not convention-level.

## Implementation status

Foundations §1–§6 shipped (v0.0.1–v0.0.11 + hardening). Foundations
§7–§17 are rules constraining v0.0.12–v0.0.16 and the editor era;
none is a new implementation milestone by itself. Missing pieces are
enumerated in the v0.3 milestone (review report §8), not here.

## Revisit conditions

- If a second renderer becomes necessary before windowed
  presentation, revisit §9–§10 sequencing (not the direction).
- If `canary-state` finds the codec rule (§16) unimplementable
  within its milestone, revisit the rule rather than silently
  shipping unversioned formats.
- If server-authoritative topology (ADR 0013) is challenged by a
  shipped genre need, revisit §8 with it — the foundations here
  (tombstones, snapshots, RNG) survive any authority model.
