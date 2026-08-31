<p align="center">
  <!--
    Logo placeholder: Canary doesn't have one yet. Brand assets are a
    deliberate design decision; see CONTRIBUTING.md when contributing
    to the project's visual identity.
  -->
  <h1>🐤 Canary Engine</h1>
</p>

<p align="center">
  <strong>An open-source 2D and 3D game engine built from first principles.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <a href=".github/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/badge/CI-GitHub_Actions-2088FF.svg?logo=github-actions&logoColor=white"></a>
  <a href="rust-toolchain.toml"><img alt="Rust" src="https://img.shields.io/badge/language-Rust-orange.svg?logo=rust"></a>
  <a href="docs/roadmap/status.md"><img alt="Status" src="https://img.shields.io/badge/status-v0.0.2_archetype_ecs-yellow.svg"></a>
</p>

---

## What is Canary?

**[Canary Engine](docs/vision/project-goals.md) is an open-source 2D and 3D game engine built from first principles.**

Canary treats **2D and 3D as first-class parts of the same engine**, rather than making one a secondary layer on top of the other. The engine is designed for games first, while also providing the broader real-time capabilities commonly expected from a modern game engine: visualization, simulation, previz, and other interactive content.

The goal is not to reproduce Unreal, Unity, or Godot feature-for-feature. Canary is being designed around a different set of architectural constraints: explicit subsystem boundaries, replaceable implementations, sandboxed extensibility, versionable project state, and written architectural decisions.

See the [project goals](docs/vision/project-goals.md) for the reasoning behind those constraints.

## Current status

> **`v0.0.2` — Archetype ECS foundation**
>
> Canary currently has the core architectural foundation needed to begin building the engine around a stable set of boundaries:
>
> * Governance and contribution rules
> * A hardened plugin ABI
> * An archetype-based ECS
> * Cached ECS queries
> * Change detection
> * A documented architecture covering rendering, physics, audio, UI, networking, localization, and project state
>
> **Canary is not yet a production-ready game engine.** There is deliberately no finished editor, renderer demo, or complete playable game-development workflow yet.
>
> This release establishes foundations rather than pretending the engine is further along than it is.

See [`docs/roadmap/status.md`](docs/roadmap/status.md) for the canonical, itemized status of what is implemented, experimental, and planned.

For the details of this release, see [`v0.0.2.md`](docs/release-notes/v0.0.2.md).

---

## Why Canary?

Canary is built around a few principles that affect the architecture from the ground up.

### 🧩 Sandboxed, language-agnostic extensibility

Canary is designed around a two-tier plugin model:

* **WebAssembly Components** for sandboxed community and marketplace content
* **Trusted native plugins** through a native C ABI for performance-critical subsystem replacement

The goal is to make extensibility powerful without making every extension a trusted part of the engine.

See [`docs/architecture/plugin-system.md`](docs/architecture/plugin-system.md).

### 🔌 Subsystems bind through interfaces

Core systems are intended to depend on **Canary-defined interfaces**, not concrete implementations.

Rendering, physics, audio, UI, and other subsystems are designed so that implementations can sit behind explicit boundaries and be replaced without forcing unrelated parts of the engine to depend on a particular backend.

This keeps the architecture modular and makes implementation choices reversible where practical.

See the [subsystem boundary principle](docs/vision/design-philosophy.md#subsystems-bind-through-interfaces-never-call-each-other--or-a-third-party--directly).

### 🖥️ One UI system

Canary is designed around a single UI abstraction, **`CanaryUI`**, shared by the editor and games built with Canary rather than maintaining separate editor and runtime UI stacks.

See [ADR 0011](docs/decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md).

### 🗂️ Projects are data

A Canary project is intended to be **explicit, identifiable, serializable data**, not a collection of opaque editor files.

That architectural choice is intended to make things such as:

* Git-friendly project history
* structured serialization
* marketplace packages
* collaborative editing
* server-authoritative project workflows

natural consequences of the project model rather than bolt-on systems.

See [ADR 0012](docs/decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md) and [ADR 0013](docs/decisions/architecture-decision-records/0013-live-collaboration-server-authoritative-topology.md).

### 📐 Architecture is a first-class artifact

Canary does not treat architecture as something that exists only in source code.

Hard-to-reverse decisions are recorded as [Architecture Decision Records](docs/decisions/architecture-decision-records/), including the alternatives considered and why they were rejected.

That makes the reasoning behind the engine part of the project itself.

---

## Architecture at a glance

Canary is organized around explicit subsystem boundaries rather than one monolithic runtime.

```text
Canary
├── Runtime
│   ├── ECS
│   ├── Scheduling
│   ├── Serialization
│   └── Platform Abstraction
│
├── Engine Systems
│   ├── Rendering
│   ├── Physics
│   ├── Audio
│   ├── UI
│   ├── Networking
│   └── Localization
│
├── Extensibility
│   └── Plugin ABI
│
└── Project
    └── Versionable Project State
```

The diagram describes the intended architectural shape of Canary. The current implementation status of each subsystem is tracked separately in [`docs/roadmap/status.md`](docs/roadmap/status.md).

Detailed subsystem designs live in [`docs/architecture/`](docs/architecture/).

---

## What exists today

Canary is currently in the **foundation stage**.

| Area               | Status         |
| ------------------ | -------------- |
| Rust workspace     | ✅ Implemented  |
| Governance         | ✅ Implemented  |
| Plugin ABI         | ✅ Implemented  |
| Archetype ECS      | ✅ Implemented  |
| Cached ECS queries | ✅ Implemented  |
| Change detection   | ✅ Implemented  |
| Rendering          | 🧭 Architected |
| Physics            | 🧭 Architected |
| Audio              | 🧭 Architected |
| UI / `CanaryUI`    | 🧭 Architected |
| Networking         | 🧭 Architected |
| Localization       | 🧭 Architected |
| Project state      | 🧭 Architected |
| Editor             | 🧭 Planned     |

> **Legend:** ✅ implemented · 🧭 architected/planned

This table is intentionally conservative. The status document is the source of truth.

---

## Build from source

There are no binary releases yet. The current project is built directly from source.

```sh
git clone git@github.com:HylightGames/canary.git
cd canary

cargo build --workspace
cargo test --workspace
cargo run -p canary-runtime
```

These commands build and run the current runtime foundation. They do **not** launch a finished editor or a complete game-development environment.

See [`docs/development/build-system.md`](docs/development/build-system.md) for the full build and development command reference.

---

## Documentation

The repository is heavily documented because understanding *why* Canary is built a certain way is considered as important as understanding *how*.

| Document                                                                                         | Purpose                                                                    |
| ------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------- |
| [`docs/roadmap/status.md`](docs/roadmap/status.md)                                               | **Start here.** Canonical implementation, experimental, and planned status |
| [`docs/vision/`](docs/vision/)                                                                   | Project goals, philosophy, and non-negotiable constraints                  |
| [`docs/release-notes/`](docs/release-notes/)                                                     | Release notes for each project version                                     |
| [`docs/architecture/`](docs/architecture/)                                                       | Subsystem and system-level architecture                                    |
| [`docs/decisions/architecture-decision-records/`](docs/decisions/architecture-decision-records/) | Architectural decisions and rejected alternatives                          |
| [`docs/roadmap/`](docs/roadmap/)                                                                 | Release planning and scoped work                                           |
| [`docs/reviews/`](docs/reviews/)                                                                 | Critical self-audits and the living risk register                          |
| [`docs/development/`](docs/development/)                                                         | Build system, coding standards, and development workflow                   |

---

## Contributing

Canary is being developed in the open, but contributions are expected to follow the project's architectural rules.

Start with:

* [`CONTRIBUTING.md`](CONTRIBUTING.md)
* [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)
* [`GOVERNANCE.md`](GOVERNANCE.md)
* [`docs/development/git-workflow.md`](docs/development/git-workflow.md)

Development branches from `dev`, not `main`.

Before making a large architectural change, read the existing ADRs and design documentation first. New hard-to-reverse decisions should be documented as part of the change.

For security issues, see [`SECURITY.md`](SECURITY.md) rather than opening a public issue.

---

## License

Canary is released under the [MIT License](LICENSE).

There are no royalties, seat licenses, or runtime fees. The project is intended to remain freely usable without introducing commercial licensing requirements later as a condition of adoption.

See [`docs/vision/project-goals.md`](docs/vision/project-goals.md) for the reasoning behind this constraint.
