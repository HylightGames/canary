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

`v0.1.0` is released (tagged and published 2026-11-15), and the **`v0.1.0`:
integration proof** milestone is complete. The live inventory is
[`status.md`](status.md); the release note is
[`docs/release-notes/v0.1.0.md`](../release-notes/v0.1.0.md); the historical
plan is [`v0.1.0-plan.md`](v0.1.0-plan.md). The `v0.0.13`–`v0.0.16` milestone
roadmaps are completed history — do not work from their "continue here"
sections.

The closure gates are complete: the audit fixes are green, candidate evidence
was collected, and the tag was published through the approval-gated workflow.
Do not re-run the closure sequence unless the release tree changes.

Next is the proposed [`v0.2.0` direction](v0.2.0-roadmap.md); it is pending
maintainer approval and a re-audit against the released tree. No `v0.2.0`
implementation has started.

## Release order through `v0.1.0` (complete)

All steps below are released; the table is retained as the dependency record.
The canonical plan in [`v0.1.0-plan.md`](v0.1.0-plan.md) holds the acceptance
detail.

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

## Planning after `v0.1.0` (proposed 2026-10-24)

A final `v0.1.0` review recorded a release recommendation and a proposed `v0.2.0`
direction. The direction is **pending maintainer approval** and was re-audited
against the released tree on 2026-11-15; closed work is annotated in the roadmap
so it is not planned again. No `v0.2.0` implementation has started.

- [`capability-roadmap-principles.md`](capability-roadmap-principles.md): every release
  must grow what developers can build, with reliability growing alongside; seven
  capability pillars, four maturity levels, three connected roadmaps.
- [`v0.2.0-roadmap.md`](v0.2.0-roadmap.md): a dependency-ordered, capability-first
  proposal for the first breadth release.
- [`../reviews/2026-10-24-v0.1.0-final-review.md`](../reviews/2026-10-24-v0.1.0-final-review.md)
  and its [findings register](../reviews/2026-10-24-v0.1.0-final-review-findings.md).

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
