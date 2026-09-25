# 0022. Constitution clarifications and red-team amendments (second pass over ADR 0020/0021)

**Status:** Accepted

## Context

ADR 0020 locked pre-v0.3 foundations; ADR 0021 amended it in eleven
places after owner review. Owner review then requested final
bolt-tightening (six semantic clarifications) plus a red-team exercise:
assume the constitution correct, construct ~20 concrete v0.3–v1.0
features, and amend only on real contradiction. Four parallel red-team
lanes tested 21 features (rollback networking, dedicated servers,
replay/desync, editor collaboration, cross-play, streaming, prefabs,
runtime-generated assets, procedural worlds, async loading, save
migration, visual scripting, modding, marketplace, hot reload, GPU
particles, VR, mobile, console, AI agents, split-screen) against all 16
rules, grounded in the repo (no event/command channels, no input
mapping, no lifecycle hooks, no capability query or surface seam —
all verified absent; version constants already in separate,
never-compared domains; runtime/core presentation-free).

Result: 20 SURVIVE as written, 1 conditional amend. This ADR records
the six clarifications and the single earned carve-out, append-only:
ADRs 0020/0021 stand except where explicitly overridden here.

## Clarification 1 — SimulationInput vs Command (extends Amendment 4)

`SimulationInput` is player/external intent *entering* a simulation
step. A `Command` is not necessarily input: `SpawnEnemy`,
`DestroyEntity`, `ApplyDamage`, `EquipWeapon`, `Teleport`,
`SetDoorState` can originate from gameplay, networking, the editor,
replay, scripted systems, or AI. Flow: hardware/sources → actions →
`SimulationInput` → deterministic sim → commands/events. Rollback
replays inputs; it re-executes the commands they produce.

## Clarification 2 — Simulation Messages vs Observation Events (extends Amendment 8)

Two categories with different contracts. **Simulation messages** are
deterministic, ordered, replayable where needed, and may affect
simulation through the command path. **Observation events**
(`DamageTaken` for UI, `Footstep` for audio, editor diagnostics) merely
notify observers, are not authoritative state, are not recorded, do not
participate in rollback, and are not replicated. Rationale: without the
split, some future `EnemyDiedEvent` will secretly mutate gameplay and
create untraceable temporal coupling. Mechanism deferred; the split is
locked.

## Clarification 3 — RNG stability under scheduling (sharpens Amendment 6)

Owned streams are necessary but not sufficient: `A→rng.next();
B→rng.next()` still depends on execution order. Locked rule: random
outcomes that must remain stable under system scheduling derive from
explicitly owned deterministic streams or stable keys (e.g. loot RNG +
entity ID + loot event ID), and never from incidental iteration order.
Gameplay-critical rolls use key-derivation, not sequential draws.

## Clarification 4 — Simulation/Presentation boundary named (sharpens Amendment 5)

```text
Runtime
├── Simulation: World, SimulationState, Input, Commands,
│   deterministic systems — a portable computation that can exist
│   without presentation
└── Presentation: render world, audio, UI, platform
```

This fits the existing headless runtime (verified presentation-free)
and is what makes headless servers, replay, rollback, desync testing,
AI simulation, dedicated servers, spectator simulation, and editor
play mode the *same* capability rather than seven architectures.

## Clarification 5 — Four asset identities, not three (corrects Amendment 1)

`LogicalAssetId` (stable authoring identity) ≠ `ContentHash`
(immutable source/content) ≠ `CookKey` (which derived artifact should
exist: content + importer + deps + platform + settings) ≠ artifact
storage key. Cooked artifacts are functions of source content,
dependencies, importer and engine versions, platform, feature set, and
cook settings; the semantic identities stay distinct even where
implementations share hash machinery.

## Clarification 6 — Three versioning universes (extends Amendment 9)

Data/schema compatibility, plugin/API compatibility, and
engine/project compatibility are separate version domains
(`ABI_VERSION: u32`, `LOADER_VERSION`, `SCHEMA_ID` versions already
exist as never-compared constants — this rule codifies practice).
Locked principle: every long-lived boundary declares its own version
domain and compatibility policy; version numbers are never implicitly
interchangeable. This matters most when plugins, marketplace packages,
and project upgrades meet.

## Amendment — Rule 13 GPU-resident carve-out (the one earned change)

Red-teaming GPU particles exposed a genuine fault line: GPU-*derived*
particles (ECS-authoritative emitters, GPU buffers as derived cache)
survive cleanly, but GPU-*simulated* state (authoritative positions in
GPU memory) contradicts Rule 13 as written, whose boundary excludes
GPU resources while determinism assumes owned streams and canonical
order. Appended clause to Rule 13, all else unchanged:

> GPU-resident state is a derived cache outside the boundary by
> default; it may enter the boundary only through a declared,
> snapshottable, backend-neutral view with defined readback semantics,
> and gameplay-relevant effects of GPU-integrated state remain
> CPU-authoritative.

This forces the design choice deliberately before RHI compute grows,
instead of letting unsnapshottable authority accumulate by accident
(the GGPO-prerequisite failure mode). Companion sequencing lock (not a
wording change): the minimal capability-query shape lands before the
first optional backend feature, so particle-driven RHI additions do
not accrete ad-hoc compat surface.

## Red-team record (21 features, 20 SURVIVE / 1 conditional)

Survive outright: rollback networking, dedicated servers, replay,
editor collaboration, cross-play (mechanism promised, compat-class
scoping deferred), streaming, prefabs, runtime-generated assets,
procedural worlds, async loading, save migration, visual scripting,
modding, marketplace (multi-version coexistence noted as prerequisite,
not contradiction), hot reload, VR, mobile, console, AI agents,
split-screen. Two near-amends deliberately declined (recorded, not
adopted): glossing Rule 10 as "family of primitives" (already in ADR
0014) and scoping Rule 13 with compat classes (premature before the
compat-class design exists).

Named prerequisites for future ADRs (not amendments): tombstone +
op-log vocabulary; frame-tagged command shape; RNG stream-derivation
vocabulary; determinism-compatibility classes; capability/surface
seams; `LogicalAssetId` registry; prefab override-precedence + bake
contract; ephemeral asset namespace + provenance; cross-thread
staging-ownership + cancellation; save-profile matrix + migrator
composition; camera/view component; player-slot indexing; observation
contract; app-lifecycle machine; GPU device-loss rule; NDA backend
policy; reload serialization contract (R-16).

## Consequences

- The constitution's effective rule set is now 16 rules plus the
  Rule 13 carve-out and Clarifications 1–6, all binding at review
  time. Per owner direction, ADRs 0020 + 0021 (+ this document) are
  the architectural baseline; future work proves compliance rather
  than reopening foundations.
- v0.0.14 inherits: logical-ID registry, migration machinery,
  prefab override/bake contract, reload serialization design.
- First optional backend feature is gated on the capability-query
  shape; first multi-pass renderer on the RenderGraph data model;
  first runtime-mint asset API on the ephemeral namespace rule.

## Implementation status

Rule-level only, except the carve-out's sequencing lock, which binds
the next RHI change. No code changes in this ADR.

## Revisit conditions

- If deterministic GPU execution (or verified GPU/CPU equivalence
  for a bounded op set) becomes practical, revisit the carve-out's
  CPU-authority sentence — with proof, not optimism.
- If the compat-class design lands, Rule 13 gains its scoping
  companion as an amendment, not an edit.
