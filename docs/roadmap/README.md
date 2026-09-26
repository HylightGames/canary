# Roadmap and contributor handoff

This page is the entry point for continuing Canary. It does not duplicate
release detail: [`status.md`](status.md) is the live inventory,
[`v0.1.0-plan.md`](v0.1.0-plan.md) is the committed near-term sequence, and
[`future-roadmap.md`](future-roadmap.md) is the dependency-ordered direction
after that target. The historical milestone record is
[`milestones.md`](milestones.md).

## Where to continue now

`v0.0.12` (audio bootstrap) is implemented on `dev`, not yet tagged. The next
planned milestone is **`v0.0.13`: CanaryUI and interactive windowed
presentation**. Start with the current status and the complete milestone
definition in [`v0.1.0-plan.md`](v0.1.0-plan.md#v0013-canaryui--windowed-presentation).

Work through these prerequisites in order:

1. **Specify reusable runtime composition.** Replace the private-harness-only
   assumption with a documented, supported way for a game to compose the
   platform, simulation schedule, renderer, UI, and audio phases. Define who
   owns and advances `RunContext`/the ECS tick, phase order, errors and
   shutdown, and how a plugin receives safe scoped access to the active game
   `World` (R-34 and R-36). Record the chosen public boundary in an architecture
   document and an ADR before implementing it.
2. **Complete the window-to-renderer seam.** Follow ADRs 0020–0022: query
   renderer capabilities through Canary-owned types; create a surface from a
   platform `Window` without leaking backend types; handle format selection,
   resize, acquisition, and recoverable presentation errors.
3. **Implement shared input intent.** Carry raw platform input through
   mappings and actions to `PlayerInput` and deterministic `SimulationInput`.
   UI navigation and gameplay must consume one coherent input path, with an
   explicit focus/capture rule and a testable action-to-simulation boundary.
4. **Build the UI vertical slice on the shared runtime.** Implement the
   `CanaryUI` contract with the decided bootstrap backend and draw a HUD in a
   real window through the existing renderer path. The same sample must read
   gameplay actions, move a game entity, and show state derived from the live
   `World`; define when UI focus captures inputs and when pass-through is
   deliberate. Keep offscreen pixel tests as supporting coverage.
5. **Close the milestone with evidence and synced docs.** Add the smallest
   runnable consumer example and automated tests for the new seams, perform a
   live interactive run, update architecture/status/milestone docs, and record
   any newly discovered risk before beginning project-state implementation.

The exact API shape in step 1 is deliberately not prescribed here: the point
of the first task is to decide and document that boundary from the current
`App`, scheduler, platform, and plugin constraints, not to bless a guessed
crate or API in advance. Do not bypass `canary-runtime`'s eventual supported
consumer surface with another private example harness.

## Near-term order after the current milestone

The canonical plan defines acceptance for each step; this summary is only a
navigation aid.

| Step | Work | Must prove |
|---|---|---|
| `v0.0.13` | Runtime composition, window presentation, common input path, `CanaryUI` | A real interactive game/UI consumer uses the supported runtime entry point |
| `v0.0.14` | Authored project state and separate simulation snapshots | Stable authored identity, versioned codecs/migrations, and `snapshot`/`restore`/`checksum`/`step` without serializing presentation state |
| `v0.0.15` | Minimal server-authoritative networking | Separate server and client processes exchange canonical state and frame-tagged input, including removal/destruction; no prediction/rollback requirement |
| `v0.0.16` | First live collaboration slice | Shared edits use an explicit operation/history, authorization, conflict, and version-lineage model over the decided server authority |
| `v0.1.0` | Integrated small sample game | Physics, renderer, assets, audio, UI, plugins, state, and networking work together through supported consumer APIs |

If an integration proof exposes a primitive that cannot meet its acceptance
criteria, stop and redesign that primitive before layering more systems on it.
Do not turn the editor, visual scripting, marketplace, full cooking/hot reload,
3D physics, or prediction/rollback into prerequisites for `v0.1.0`.

## How to pick up a task

1. Read [`status.md`](status.md) for what exists in code, what is only
   documented, and current risks.
2. Read the relevant section of [`v0.1.0-plan.md`](v0.1.0-plan.md), then the
   linked architecture document and governing ADRs.
3. Check the current review decisions in
   [`docs/reviews/triage/`](../reviews/triage/) and the
   [`risk register`](../reviews/risk-register.md) before reopening settled
   questions.
4. Implement only the next dependency-ready slice. Update the affected
   architecture document, status, roadmap, and risk entry with the change.
   For a new cross-cutting decision, append an ADR; do not silently change a
   prior decision.

Dates and speculative version numbers are intentionally omitted from work
past `v0.1.0`. See [`future-roadmap.md`](future-roadmap.md) for the post-target
sequence and the design records to create when their triggers arrive.
