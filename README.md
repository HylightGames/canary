<p align="center">
  <!--
    Logo placeholder: Canary doesn't have one yet. Brand assets are a
    real design decision worth making deliberately rather than
    defaulting to whatever gets generated first -- see CONTRIBUTING.md
    if you'd like to help design one. When one exists, it replaces this
    comment as a centered <img> here, matching this same layout.
  -->
  <h1>🐤 Canary Engine</h1>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <a href=".github/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/badge/CI-GitHub_Actions-2088FF.svg?logo=github-actions&logoColor=white"></a>
  <a href="rust-toolchain.toml"><img alt="Rust" src="https://img.shields.io/badge/language-Rust-orange.svg?logo=rust"></a>
  <a href="docs/roadmap/status.md"><img alt="Status" src="https://img.shields.io/badge/status-v0.0.1_foundation-yellow.svg"></a>
</p>

## 2D and 3D game engine, built from first principles

**[Canary Engine](docs/vision/project-goals.md) is an open-source game
engine, built from first principles rather than as a clone of Unreal,
Unity, or Godot.** It's branded and marketed as a game engine — 2D and 3D
both, first-class, neither bolted onto the other — with the same broader
applicability (previz, visualization, simulation, general real-time
content creation) that "game engine" already means in practice for the
engines it's learning from. See
[why](docs/vision/project-goals.md#2d-and-3d-games-and-beyond).

> **Current status: `v0.0.2` — Archetype ECS, released.** Governance, a
> hardened plugin ABI, an archetype-based ECS with cached queries and
> change detection, and a full documented architecture (rendering,
> physics, audio, UI, networking, localization, project state) — none of
> it a rendering/physics/editor demo yet, on purpose. See
> [`docs/roadmap/status.md`](docs/roadmap/status.md) for a precise,
> itemized answer to "what actually works right now," and
> [`RELEASE_NOTES_v0.0.2.md`](RELEASE_NOTES_v0.0.2.md) for the full story.

## Free, open source, and no strings attached

Canary is MIT-licensed. No royalties, no seat licenses, no runtime fee
that can change after adoption — see
[`docs/vision/project-goals.md`](docs/vision/project-goals.md) for why
that's treated as non-negotiable rather than a detail to revisit later,
and [`GOVERNANCE.md`](GOVERNANCE.md) for how project decisions get made
and how that scales as more maintainers join.

## What makes it different

- **Language-agnostic, sandboxed plugins and mods** — a two-tier
  architecture: WebAssembly Components for community/marketplace content,
  a trusted native C ABI for performance-critical subsystem replacement.
  See [`docs/architecture/plugin-system.md`](docs/architecture/plugin-system.md).
- **Nothing calls a third party, or another Canary subsystem, directly.**
  Rendering, physics, audio, and UI are all traits with swappable
  backends (`wgpu`, Rapier/Jolt, a custom in-house audio engine, `egui`)
  — see [`docs/vision/design-philosophy.md`](docs/vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly).
- **One native UI system**, `CanaryUI`, shared by the editor and every
  game built with Canary — not two parallel UI stacks. See
  [ADR 0011](docs/decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md).
- **The project itself is versionable data**, not a folder of opaque
  files — explicit, identifiable, serializable state so Git-friendliness,
  marketplace packages, and collaborative editing (server-authoritative,
  self-hostable — see
  [ADR 0013](docs/decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md))
  are consequences of the architecture, not separate features. See
  [ADR 0012](docs/decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md).
- **Decisions are written down.** Every hard-to-reverse architectural
  choice gets an [ADR](docs/decisions/architecture-decision-records/)
  explaining what it rejected, not just what it kept.

## Getting the engine

No binary downloads yet — there's nothing to ship. Build from source:

```sh
git clone git@github.com:HylightGames/canary.git
cd canary
cargo build --workspace
cargo test --workspace
cargo run -p canary-runtime
```

See [`docs/development/build-system.md`](docs/development/build-system.md)
for the full command reference.

## Documentation

| | |
|---|---|
| [`docs/roadmap/status.md`](docs/roadmap/status.md) | **Start here** — precise, itemized: what's built vs. planned |
| [`docs/vision/`](docs/vision/) | What Canary is, and what it refuses to compromise on |
| [`docs/architecture/`](docs/architecture/) | Every subsystem's design, built or not |
| [`docs/decisions/architecture-decision-records/`](docs/decisions/architecture-decision-records/) | The numbered decision log |
| [`docs/roadmap/`](docs/roadmap/) | What's scoped for which release |
| [`docs/reviews/`](docs/reviews/) | Periodic critical self-audits + a living risk register |
| [`docs/development/`](docs/development/) | Build system, coding standards, git workflow |

## Community and contributing

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md) and the
[Code of Conduct](CODE_OF_CONDUCT.md) — branch from `dev`, not `main` (see
[`docs/development/git-workflow.md`](docs/development/git-workflow.md)).
[`GOVERNANCE.md`](GOVERNANCE.md) covers how decisions get made. Security
issues: see [`SECURITY.md`](SECURITY.md), not a public issue.

## License

[MIT](LICENSE).
