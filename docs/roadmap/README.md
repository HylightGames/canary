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

`v0.0.16` is the latest tagged release. The active pre-cut target is
**`v0.1.0`: integration proof** (untagged draft).
The dependency-ordered release plan is [`v0.1.0-plan.md`](v0.1.0-plan.md);
the live inventory is [`status.md`](status.md); the remaining gates are
[`RELEASE_CHECKLIST.md`](../reviews/RELEASE_CHECKLIST.md). The
`v0.0.13`–`v0.0.16` milestone roadmaps are completed historical handoffs —
do not work from their "continue here" sections.

Work through these closure gates in order:

1. **Keep the audit-findings fixes green on `dev`.** Runtime multi-step
   passes (`SimPasses`), loopback-only networking, physics structural
   reconciliation, the localized collectathon HUD, Vulkan `DeviceId`
   provenance (fatal RHI creation documented in ADR 0029), and the
   hardened release workflow are implemented with tests. Hold them
   through the four gates (`build`, `test`, `clippy -D warnings`,
   `fmt --check`) plus `cargo doc` with warnings denied.
2. **Collect candidate-level integration evidence.** On the exact release
   candidate commit: collectathon headless, server/client replication,
   two-client collaboration, Tier A guest probe, snapshot determinism,
   Xvfb windowing, llvmpipe Vulkan/render-ECS legs, and the rustdoc gate.
   Windowed/audio live evidence is recorded (2026-10-21 Arch Linux Vulkan
   run, one qualified underrun); re-record only if the candidate differs
   behaviorally.
3. **Cut the release.** dev-to-stable promotion, tag `v0.1.0` matching the
   workspace version, `docs/release-notes/v0.1.0.md` published by the
   release workflow. `v0.1.0` is not releasable until every gate above
   passes on the candidate.

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
