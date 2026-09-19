# External architecture review triage — Review #4 ("pre-v0.2.0")

A fourth external AI architecture review ("Canary Engine Architecture
Review (pre-v0.2.0)"), commissioned by Cloudy, triaged the same way as
[Review #3](2026-09-review-3-triage.md): checked against the actual repo,
not taken on faith.

## Headline finding: this review is working from a stale, incomplete view of the repo

Two concrete, checkable errors, not just differences of opinion:

- **"Currently Canary uses wgpu."** This is wrong, and was already wrong
  before this review was written. [ADR 0004](architecture-decision-records/0004-rendering-abstraction-strategy.md)
  did originally choose `wgpu` as a bootstrap — but its own status line
  says outright that this sub-decision is
  **"superseded by [ADR 0016](architecture-decision-records/0016-native-rendering-backends.md)."**
  ADR 0016 drops `wgpu` entirely in favor of one native crate per graphics
  API (`canary-render-vulkan` via `ash`, with `-metal`/`-dx12`/`-gl`
  named as later crates), specifically because "no privileged built-ins"
  should apply to rendering backends the same way it already applies to
  physics. This isn't a documentation gap the review is pointing out —
  it's the review not having read (or the repo it saw not yet having)
  ADR 0016. The entire "Rendering/RHI Decoupling" section's recommendation
  ("create a `RenderBackend` trait... default implementation can wrap
  wgpu") describes work that's not just done, but done in the *opposite*
  direction the review assumes: there is no `wgpu` to wrap, by design.
  ADR 0016 also already did the exact "verify, don't assume" check this
  section calls for in spirit — `cargo tree -p canary-render` was checked
  directly and shows zero dependencies, so a backend crate structurally
  cannot leak into a build that doesn't depend on it.
- **Lifecycle/tick section describes `v0.0.1`-era state** ("everything
  runs on the main thread in v0.0.1," "there's no job scheduler") as if
  current. `canary-scheduler` (a real `Schedule`/`SystemAccess` job
  system) landed at `v0.0.8`; real delta-time and a wall-clock `App::run`
  loop landed as part of `v0.0.9`, in progress when this review was
  written. `dev` is not at `v0.0.1`.

Everything else below is evaluated on its merits regardless of this, but
it's worth being explicit: this review's specific recommendations are
less reliable where they describe *current state* than where they
describe *general principle* — the general principles it restates are
sound (and, per the pattern already established in
[Review #3](2026-09-review-3-triage.md), largely already Canary's stated
architecture), but several of its "current status" claims should not be
taken at face value.

One more small, low-stakes but worth-naming error: the "Migration
Strategies" section's example — "ADR 0011: Introducing persistent Entity
IDs" — collides with a real, already-existing, unrelated ADR 0011
(CanaryUI bootstrapped on `egui`). Not a substantive issue (it's a
hypothetical example number, not a real proposal), but another sign this
review didn't check the current ADR log before writing.

## Points, briefly (most map directly onto Review #3's points — cross-referenced rather than re-argued)

| Point | Verdict | Note |
|---|---|---|
| Enforce module boundaries / no third-party leakage | **Already true** | Same finding as [Review #3 point 1](2026-09-review-3-triage.md#1--elevate-replaceability-to-a-project-wide-law); this review's own subsystem table (Rendering/Physics/Audio/UI/Networking/Assets/Scripting) matches `design-philosophy.md` and `engine-overview.md` almost exactly |
| Clarify runtime vs. persistent entity identity | **Already true** | [`state-and-versioning.md`](../architecture/state-and-versioning.md#two-identities-that-must-not-be-conflated)'s "two identities that must not be conflated" section already makes exactly this distinction, in more depth (a full near/medium/long-term layering) than proposed here |
| Decouple rendering/RHI from wgpu | **Reject as stated; already true in substance** | See headline finding above — the premise is factually wrong, but the underlying principle (RHI trait, no privileged backend) is not just satisfied, it's satisfied more strongly than proposed (zero-dependency `canary-render`, verified via `cargo tree`) |
| Abstract physics/audio/UI behind backend traits | **Already true** | Matches [`physics.md`](../architecture/physics.md), [`audio.md`](../architecture/audio.md), [`ui-toolkit.md`](../architecture/ui-toolkit.md) directly; naming (`PhysicsBackend`, `AudioBackend`) already matches what this review proposes |
| Asset pipeline (content-hash IDs, cook step, plugin importers) | **Already true** | Matches [`asset-system.md`](../architecture/asset-system.md) closely — including the specific "Tier A vs. Tier B by author-time-vs-runtime trust tradeoff" classification guidance this review's Plugin Model section separately asks for, which already exists ([`asset-system.md#importers-as-plugins`](../architecture/asset-system.md#importers-as-plugins)) |
| Plugin Tier A/B classification guidance | **Already true** | See above; also overlaps [Review #3 point 11](2026-09-review-3-triage.md#11--extension--replacement--override-as-distinct-plugin-concepts)'s genuinely-open finding (conflict resolution when multiple plugins claim the same replacement role) — that gap is real and already filed as R-35; this review's specific ask (which subsystems are Tier A vs. B) is not the same gap and is already answered |
| World/serialization strategy, delta-sync, versioning | **Already true** | [`state-and-versioning.md`](../architecture/state-and-versioning.md) and [ADR 0006](architecture-decision-records/0006-versioning-scheme.md) already cover this; the specific "delta-sync via `query_changed_since`" idea is already tracked with more precision as risk-register R-33 (removal/destruction has no signal yet — a sharper version of the same concern) |
| Lifecycle/tick semantics, fixed vs. variable update hooks | **Partially accept** | The specific `on_fixed_update`/`on_update` hook split is a reasonable idea not yet decided either way (fixed-timestep physics ticking is a real open question for `v0.0.11`); the "no job scheduler, main-thread only" framing is stale (see headline finding) — filed as a note for whenever physics's fixed-timestep design is actually settled, not urgent now |
| Build/dependency boundaries, unused-subsystem elimination | **Already true** | Same as [Review #3 points 2/3/10](2026-09-review-3-triage.md); ADR 0016's zero-dependency `canary-render` proof is a stronger instance of exactly this than anything currently in code when review #3 was triaged |
| Capability detection | **Accept (deferred)** | Identical idea to [Review #3 point 15](2026-09-review-3-triage.md#15--capability-detection-instead-of-platform-stereotypes); no new reasoning, same verdict |
| API vs. implementation stability policy | **Partially accept** | Same substance as [Review #3 point 5](2026-09-review-3-triage.md#5--api-stability-vs-implementation-stability-phased-by-version); same verdict |
| Escape hatches for advanced users | **Already true**, with one small new idea | Matches [Review #3 point 14](2026-09-review-3-triage.md#14--explicit-escape-hatch-levels); the one genuinely new specific suggestion — a deliberate, documented way to step outside the WASM sandbox for *debugging* Tier A plugins specifically (distinct from just using Tier B) — isn't currently named anywhere. Low priority; worth a one-line note in `plugin-system.md`'s future-work section whenever Tier A tooling/debugging is actually built, not now |
| Migration strategies / ADR-per-breaking-change | **Already true** | This is what the ADR log + [ADR 0001](architecture-decision-records/0001-record-format.md)'s own process already does; no new mechanism proposed beyond what exists |
| Calendar-dated Gantt roadmap (specific months/years through 2030 for GI, virtual texturing, etc.) | **Reject** | Conflicts directly with this project's own stated planning philosophy — [`future-roadmap.md`](../roadmap/future-roadmap.md) deliberately avoids assigning fake specificity to distant, research-heavy work, and [`v0.1.0-plan.md`](../roadmap/v0.1.0-plan.md) is dependency-ordered, not calendar-ordered, by design. A generic Gantt chart with invented 2027–2030 dates for GI/Nanite-style/rollback netcode isn't a plan this project can adopt as-is; it reads as templated rather than derived from Canary's actual sequencing |

## What this changes right now

Nothing new to implement or file beyond what [Review #3's triage](2026-09-review-3-triage.md)
already resolved — this review's genuinely-new content (the stale-state
corrections above, the WASM-sandbox-debugging escape-hatch note) is
either already captured by an existing risk-register entry (R-35) or too
low-priority to act on before its trigger condition arrives. No new
risk-register entries needed. Docs-only, per the same standing
instruction as Review #3's triage.
