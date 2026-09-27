# Roadmap and contributor handoff

This page is the entry point for continuing Canary. It does not duplicate
release detail: [`status.md`](status.md) is the live inventory,
[`v0.1.0-plan.md`](v0.1.0-plan.md) is the canonical near-term sequence, and
[`future-roadmap.md`](future-roadmap.md) is the dependency-ordered direction
after that target. The near-term plan now gives each remaining milestone
through `v0.1.0` ordered work packages and exit evidence; use those gates
before starting the next implementation stage. The historical milestone
record is [`milestones.md`](milestones.md).

## Where to continue now

`v0.0.12` (audio bootstrap) is implemented on `dev`, not yet tagged. The next
milestone is **`v0.0.13`: CanaryUI and interactive windowed presentation**.
The detailed handoff is [`v0.0.13-roadmap.md`](v0.0.13-roadmap.md); the
dependency-ordered release plan through `.1.0` is
[`v0.1.0-plan.md`](v0.1.0-plan.md).

Work through these prerequisites in order:

1. **Keep the completed R-34 foundation distinct from full composition.**
   The `e256a61` commit implements the reusable runtime library's active-World
   ownership, `RunContext`, and scoped Tier A lifecycle calls. ADR 0024 still
   needs review for the complete composition contract; the runtime does not
   yet run the schedule or platform/presentation frame phases. Resolve R-38's
   `RunContext.tick`/`sim_time` boundary as part of that driver.
2. **Finish and review window presentation.** Surface/swapchain code is in
   the current working tree. Owner-reported evidence is 5/5 presented frames;
   document platform/device conditions and close error, resize, minimize, and
   destruction checks before treating the seam as landed.
3. **Implement shared input intent.** Review proposed
   [ADR 0025](../decisions/architecture-decision-records/0025-deterministic-input-actions-and-ui-capture.md),
   then carry normalized raw events through mappings/actions to deterministic
   `SimulationInput`, with an explicit capture and focus-loss release rule.
4. **Build the UI vertical slice.** Use the accepted ADR 0011 backend-neutral
   boundary and egui bootstrap; render a HUD in the same game window and RHI
   path, with UI intent entering at a defined simulation boundary.
5. **Complete the public consumer loop and close out.** Migrate the headless
   proof and add a small interactive consumer using the same supported
   `canary-runtime` library for schedule, input, UI, render, and shutdown.
   Record live run conditions, update status/risk/changelog/release notes, and
   meet the exact gates in [`v0.0.13-roadmap.md`](v0.0.13-roadmap.md).

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
