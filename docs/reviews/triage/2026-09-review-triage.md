# External architecture review triage (September 2026)

Two external AI architecture reviews of the `dev` branch were commissioned
by Cloudy (review #1: execution model; review #2: 27 follow-up additions).
This is the project-lead triage pass over both, done after CI was brought
back to green rather than mixed into that work. Every recommendation is
marked **Accept**, **Accept (deferred)**, **Partially accept**, or
**Reject**, with the reasoning and, where relevant, what in the current
codebase was actually checked rather than assumed.

Both reviews are unusually good — reviewer #1's own closing line ("the
danger now isn't bad architecture, it's letting the scheduler grow around
the ECS without first defining what a system is allowed to read/write")
and reviewer #2's own closing line ("I would not implement all 27 of these
now... lock the important contracts, implement only what the next
milestone needs") are both exactly right, and this triage follows that
spirit: most "Accept" items below are *design-now, build-later*, not
*build-now*.

## How to read this

- **Accept** — agree, and it's actionable soon.
- **Accept (deferred)** — agree with the recommendation, but there's
  nothing to build yet (the subsystem it applies to doesn't exist, or a
  prerequisite hasn't landed) — noted for whoever scopes that work later.
- **Partially accept** — the underlying concern is real; the specific
  proposed shape isn't quite right for this project, or is already
  addressed differently.
- **Reject** — disagree, with reasoning.
- **Already true** — the review is describing something the repo has
  since fixed or already had; noted so nobody re-does it.

---

## Review #1 (execution model)

| # | Recommendation | Verdict |
|---|---|---|
| 1 | Multi-component queries before the scheduler | **Accept** |
| 2 | Command buffers before parallel execution | **Accept (deferred)** |
| 3 | `Tick` u32 → u64 | **Accept** |
| 4 | First-class time types (`WallClock`/`FrameTime`/...) | **Accept (deferred)** |
| 5 | Typed `Resource` storage | **Accept** |
| 6 | Don't over-optimize archetype storage yet | **Accept** (no action — already the status quo) |
| 7 | Archetype explosion / `Enabled<T>` | **Accept (deferred)** |
| 8 | Separate structural vs. persistent identity | **Partially accept** |
| 9 | Plugin manifest + declarative capabilities | **Already true** (already an acknowledged, explicitly-scoped-out gap) |
| 10 | Keep the RHI tiny | **Accept** (no action — already the status quo) |
| 11 | Asset system built around handles/`AssetId` | **Partially accept** |
| 12 | Start using `tests/` for real integration seams | **Accept** |
| 13 | Xvfb was right; Windows/macOS live-window CI eventually | **Accept (deferred)**, agree with reviewer's own "don't block on it now" |
| 14 | CI branch triggers say `[main, dev]`, should be `[dev, stable]` | **Already true** (fixed — `ci.yml` already reads `[stable, dev]`) |
| 15 | Stale `v0.0.1-pre1`/version-number language in docs | **Accept** |
| 16 | New `docs/architecture/execution-model.md` | **Accept** — the single highest-value item in either review |

### Detail on the ones worth explaining

**#1 and #16 are the load-bearing items.** Verified directly against
`engine/canary-ecs/src/world.rs`: `World::query<T>()` is exactly what the
review describes — one component type per call, no `(&Position,
&Velocity)`-style joins, no `Without`/`Changed` filters as a query
shape (change detection exists, but as a separate method,
`query_changed_since`, not composed into a general filter system). This
isn't a surprise regression — `docs/architecture/core-runtime.md`'s own
"Threading & the job system" section already defers the scheduler
specifically because "a job system designed before the ECS's real
data-access declarations exist would likely need redesigning anyway once
those declarations land." The review's sequencing (queries + resources +
access model *before* scheduler) is exactly what that existing paragraph
already implies — it just hadn't been written down as its own contract.
That's what `execution-model.md` should be: not a new idea, but the
existing implicit sequencing made explicit and load-bearing, covering
queries, resources, commands, tick/time, and shutdown ordering in one
document instead of scattered implications.

**#2 (command buffers) is correctly sequenced by the review, but there is
nothing to defer *from* yet** — `canary-runtime` is single-threaded today
(confirmed: no thread pool, no parallel system execution exists at all),
so there's no live race for a command buffer to prevent right now. Accept
the recommendation for when the scheduler work starts, not as standalone
work today; building it in isolation risks guessing at an API shape the
actual access-model design (#1) should determine.

**#3 (`Tick` width) is real and cheap** — confirmed `pub struct
Tick(u32)` with `#[derive(..., PartialOrd, Ord)]` and a plain `tick >
since` comparison in `query_changed_since`. The review's math is right.
Unlike most items here, this doesn't need the execution-model design
first — it's a mechanical, low-risk change (two crates touch `Tick`
directly: `canary-ecs` and its `query_changed_since` caller in
`canary-plugin-api`'s tests). Worth doing as its own small PR rather than
bundling it into the larger execution-model work, so it doesn't get lost
in a bigger review.

**#8, I'd push back on slightly.** The review frames this as still
outstanding, but `canary-ecs` already ships a first cut of exactly this
separation for one identity pair: [ADR
0010](architecture-decision-records/0010-component-identity-across-language-boundary.md)'s
`CanaryComponent::SCHEMA_ID` deliberately keeps `TypeId` host-internal and
gives components a separate, stable, string-based identity for the
plugin/language boundary — which is real, implemented, and has a real
consumer (`canary-plugin-api`'s Tier A loader resolving a WASM guest's
`schema-id` through it). The review's broader point — that `AssetId`,
`PluginId`, `ProjectId`, `WorldId`, `SceneId` shouldn't collapse into one
generic ID type either — is correct and worth keeping as a standing
constraint, but it's not a gap in current work so much as a principle to
not violate in future work, since `canary-state`/`canary-assets` don't
exist yet to violate it in. Filed as a constraint for whoever scopes
those crates, not a task now.

**#11, similarly:** `docs/architecture/asset-system.md` already commits
to content-addressed identity for the *cook/cache* layer ("cooked assets
are identified and cached by a content hash... not by file path alone") —
the review's core complaint (don't make `PathBuf` the identity) is
already the design for that layer. What's genuinely still open is the
*runtime-facing* ergonomic API — an actual `AssetHandle<T>` type game
code holds in an ECS component — which the content-addressing decision
doesn't by itself specify. Accept the recommendation for that specific,
still-open surface; it's not starting from zero.

---

## Review #2 (27 additions)

Grouping by verdict rather than listing all 27 in the original order,
since several cluster naturally.

### Accept (real gap, design-now on the execution-model doc, build when its milestone arrives)

- **#1 Events/messages** (`EventWriter`/`EventReader` distinct from
  `Commands`) — genuinely missing, genuinely useful vocabulary. Belongs
  in `execution-model.md` alongside queries/resources/commands.
- **#2 Lifecycle/shutdown states** (`Created`→...→`Stopped`/`Failed`,
  formal shutdown ordering) — real gap; `canary-runtime`'s current
  `App`/`Engine` bootstrap has an ad hoc start/stop, not a state machine.
  Matters more once plugins + a job system + a renderer all have
  resources to tear down in a specific order.
- **#8 Engine-owned seeded RNG** — real, cheap-to-regret-later gap. No
  `rand::thread_rng()` calls exist yet in engine code (checked), so
  there's no live violation to fix, but the *pattern* should be
  established before gameplay code starts landing, not after.
- **#15/#16 Machine-verifiable dependency direction + architecture
  tests** — this is the standout of the whole second review. It fits the
  project's own stated values (`tooling-enforced architecture rather than
  documentation-only standards`) better than almost anything else
  suggested in either document. `cargo-deny`'s `bans`/graph checks (or a
  small custom xtask lint) could enforce "canary-ecs cannot depend on
  canary-render" etc. as a real CI gate.
- **#26 "One authoritative owner per mutable resource"** and **#23
  authoritative vs. derived state** — both good, both cheap to state as
  a written rule now (they belong in `execution-model.md` too) even
  though there's little to enforce yet with only one real subsystem
  (rendering) consuming ECS state so far.

### Accept (deferred — right idea, no current subsystem for it to attach to)

- **#5 Named arena types** (`FrameArena`/`ScratchArena`/`CommandArena`) —
  `core-runtime.md`'s existing "Memory management philosophy" section
  already commits to the underlying *policy* (arena/pool allocators for
  steady-state high-frequency allocation, global allocator for
  long-lived data) — this review adds concrete names, which is a nice
  refinement to fold in once there's an actual frame loop or command
  buffer to give an arena to. Not new architecture, just naming.
- **#6/#7 `RuntimeMode` enum + deterministic replay** — genuinely good,
  genuinely premature. No physics, no networking, no fixed-timestep loop
  exists yet for "deterministic" to mean anything concrete against.
- **#9/#10 Schema-driven serialization + migrations** — `canary-state`
  doesn't exist yet; `docs/architecture/state-and-versioning.md` already
  frames project state as "a versionable graph" per [ADR
  0012](architecture-decision-records/0012-project-state-as-a-versionable-graph.md),
  which is directionally the same concern. Revisit when that crate is
  actually scoped.
- **#11 Virtual filesystem** (`project://`, `cache://`, ...) — good idea,
  blocked on the asset system existing at all.
- **#12 Asset dependency graph** — same.
- **#13 Universal hot reload** — assets and scripts already share one
  hot-reload pattern (`asset-system.md` cross-references
  `scripting-system.md#hot-reload` explicitly); generalizing that same
  pattern to plugins/localization/UI/project-data is a good target once
  those subsystems exist, not a new idea so much as "keep applying the
  pattern already chosen."
- **#14 Capability graph** (principal → capability → resource →
  operation, generalized beyond plugins) — good long-term shape, but
  `canary-plugin-api`'s current boolean-ish capability set is
  appropriately minimal for what it has to authorize today (ECS
  read/write). Revisit once a second capability consumer exists (e.g.
  filesystem or network capabilities actually get implemented, per
  `plugin-system.md`'s own "advisory, not yet structurally enforced"
  list) — designing the generalized graph against one consumer risks
  guessing wrong.
- **#19/#20 Fuzzing + golden fixtures** — genuinely good, "start earlier
  than most engines" is right given how much of this project is exactly
  the kind of surface fuzzing is good at (ABI boundaries, serialization,
  archetype storage). Deferred only because it's better added once
  there's a stable-ish target (the WASM ABI, component identity) rather
  than fuzzing a surface still actively changing.
- **#21/#22 Editor-via-introspection + reflection/`TypeInfo`** — correct
  and important, but there is no editor yet at all; premature to design
  reflection against a consumer that doesn't exist.
- **#24 Design for rollback without implementing it** — reasonable
  standing constraint for whoever writes the networking/physics
  execution path; nothing to change today.
- **#27 Engine-wide task graph** (unify ECS/asset/physics/render
  scheduling into one dependency graph) — right instinct, but this is
  the kind of unification that goes *better* after there's more than one
  real scheduler to unify (today there's zero). Revisit once the ECS
  scheduler and a render-extraction stage both exist.

### Partially accept

- **#3 Unified `EngineError` hierarchy.** The underlying goal
  (distinguish recoverable/fatal/content/developer/backend errors) is
  right and worth adding. The specific shape — one `EngineError` enum
  wrapping every subsystem — cuts against a decision already made and
  implemented: `core-runtime.md`'s "Error handling conventions" commits
  to *per-crate* typed errors (`PluginError`, future `EcsError`, etc.),
  each independently `match`-able, specifically so callers aren't forced
  to depend on every subsystem's error type through one shared enum.
  `canary-plugin-api::PluginError`'s own doc comment goes further,
  treating "never leak a third-party type" as load-bearing. A single
  top-level `EngineError` would need to either wrap all of those (fine)
  or replace them (a real regression against a decision already made and
  shipped). Accept a *severity/category trait* (`is_recoverable()`,
  `is_content_error()`, etc.) that every per-crate error type can
  implement, rather than a single sum-type hierarchy — same benefit
  (uniform handling at call sites that want it), without collapsing the
  per-crate boundary.
- **#4 Observability as a first-class subsystem.** Agree with the goal;
  "every subsystem uses the same instrumentation model" is good.
  `core-runtime.md` already has structured logging via `tracing` — the
  gap is metrics/profiling specifically, not logging, which the review's
  framing slightly overstates as a bigger gap than it is. Revisit once
  there's a frame loop worth profiling (right now, profiling
  single-threaded `canary-runtime` boot wouldn't show much).
- **#17 Feature-flag discipline** (flags select implementations, never
  fundamentally alter semantics). Agree with the rule as stated, and the
  project already follows it in the one real precedent that
  exists — `winit-backend` selects an implementation, doesn't create a
  parallel `World` semantics. Nothing to fix; worth writing down
  explicitly (in `execution-model.md` or `coding-standards.md`) as a rule
  future sessions should keep following, since nothing currently
  enforces it besides review.
- **#18 Compatibility policy before 1.0.** Right instinct, wrong urgency:
  a `compatibility.md` written *now*, at `v0.0.6`, would be mostly
  speculative — the actual set of "version surfaces" (Plugin ABI, WASM
  ABI, component schemas, project format...) is still being discovered
  one crate at a time. Worth drafting once there are 2-3 real version
  surfaces with actual bumps behind them to generalize from (Tier A's
  ABI version, already versioned per ADR 0009, is the first candidate),
  not from zero.

### Reject / no action

None of review #2's 27 items are rejected outright — everything proposed
is a reasonable engine feature *eventually*. The only real pushback is
the `EngineError` shape above (partially accept, not reject) and the
general note, already stated by both reviewers themselves, that
attempting more than a handful of these in the current milestone would
be a mistake.

---

## The "five invariants" (review #2's closing framing)

Ownership, Access, Time, Identity, Side-effects. All five are worth
adopting as literal, written rules in `execution-model.md` rather than
implicit values — they're consistent with what's already built (ADR
0010's identity separation, the plugin capability model's access
enforcement) and would have made at least one of this session's own
findings (the `Tick` wraparound bug) more likely to be caught by design
review rather than an external audit. No disagreement here; this is the
one piece of either review I'd adopt closest to verbatim.

## What this triage actually changes right now

Per Cloudy's instruction, nothing in either review gets implemented as
part of this triage pass. Concretely, the next pieces of work this
unblocks (for a future, separate session/PR, not bundled here):

1. `docs/architecture/execution-model.md` — the highest-value single
   artifact from either review, covering queries, resources, commands,
   events, tick/time, the five invariants, and shutdown ordering as one
   contract, before the scheduler is implemented.
2. `Tick(u32)` → `Tick(u64)` — small, mechanical, worth its own PR ahead
   of the larger execution-model work.
3. A documentation pass on `core-runtime.md`'s stale `v0.0.1-pre1`/
   version-number language (review #1, #15) — cheap, independent of
   everything else here.
4. Dependency-direction enforcement (review #2, #15/#16) is worth
   scoping as real xtask/CI work relatively soon — it's cheap relative to
   its payoff and fits the project's existing "tooling-enforced, not
   documentation-only" value directly.

Everything else above is filed as **accepted-but-deferred** against the
milestone it actually belongs to, so it isn't rediscovered from scratch
later.
