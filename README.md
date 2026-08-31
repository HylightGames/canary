<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="misc/logo/icon-white.svg">
    <img src="misc/logo/icon-black.svg" width="400" alt="Canary Engine logo">
  </picture>
</p>

<h2 align="center">2D and 3D game engine built in Rust</h2>

<p align="center">
  <strong>
    An open-source game engine built from first principles, with extensibility at its core.
  </strong>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <a href=".github/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/badge/CI-GitHub_Actions-2088FF.svg?logo=github-actions&logoColor=white"></a>
  <a href="rust-toolchain.toml"><img alt="Rust" src="https://img.shields.io/badge/language-Rust-orange.svg?logo=rust"></a>
  <a href="docs/roadmap/status.md"><img alt="Status" src="https://img.shields.io/badge/status-v0.0.2-yellow.svg"></a>
</p>

> **Early development:** Canary is currently at `v0.0.2` and is not yet
> production-ready. The engine foundation and extensibility systems are under
> active development.

## Current status

Implemented:

- Archetype-based ECS
- Cached ECS queries and change detection
- Native C-ABI plugins
- Sandboxed WebAssembly Component plugins
- Core runtime and platform abstractions

Rendering, UI, physics, audio, networking, localization, project state, and
the editor are still in development or planned.

See [`docs/roadmap/status.md`](docs/roadmap/status.md) for the complete,
living implementation status.

## Getting started

There are no binary releases yet. Build Canary from source:

```sh
git clone https://github.com/HylightGames/canary.git
cd canary

cargo build --workspace
cargo test --workspace
cargo run -p canary-runtime