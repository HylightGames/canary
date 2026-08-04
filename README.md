# Canary Engine

**A next-generation, open-source game engine, built from first principles
rather than as a clone of Unreal, Unity, or Godot.**

> **Status: `v0.0.1` — Foundation, released.** This is the release that
> establishes the project's engineering standards and architectural core:
> repository, documentation, governance, a hardened plugin ABI, and a
> compiling, tested engine skeleton — deliberately not the release that
> adds rendering, physics, networking, real windowing, or an editor. See
> [what `v0.0.1` contains, and why the bar for it was set here](docs/roadmap/v0.0.1-roadmap.md),
> and [the release notes](RELEASE_NOTES_v0.0.1.md) for the full account.

Everything before `v1.0.0` is explicitly experimental — see the
[versioning scheme](docs/decisions/architecture-decision-records/0006-versioning-scheme.md).

## What is Canary?

Canary is a long-term (multi-year), MIT-licensed game engine project
organized around a few non-negotiable bets:

- **Language-agnostic, sandboxed plugins and mods**, via a two-tier
  architecture: WebAssembly Components (any language with a Component Model
  toolchain — Rust, C, C++, Zig, and more) for community/marketplace
  content, and a trusted native C ABI for performance-critical subsystem
  replacement. See [`docs/architecture/plugin-system.md`](docs/architecture/plugin-system.md).
- **Everything replaceable is a trait, not a fork.** Rendering, physics, and
  asset importers are designed as swappable subsystems from day one.
- **Rust** as the engine core's implementation language — see
  [ADR 0002](docs/decisions/architecture-decision-records/0002-primary-language-selection.md)
  for the full reasoning (and why Zig and C++ were seriously considered and
  not chosen, for now).
- **Decisions are written down.** Every hard-to-reverse architectural choice
  gets an [Architecture Decision Record](docs/decisions/architecture-decision-records/)
  explaining the alternatives it rejected, not just the one it kept.
- **Multiplayer and modding are foundational, not bolted on.** The ECS and
  networking model are designed together from the start; the plugin
  sandbox exists specifically to make a community marketplace safe by
  construction, not by review policy.
- **One native UI system, shared by the editor and every game built with
  Canary.** `CanaryUI` is a trait-based abstraction from day one,
  bootstrapped on `egui` — see
  [ADR 0011](docs/decisions/architecture-decision-records/0011-canaryui-abstraction-bootstrapped-on-egui.md).
- **The project itself is versionable data, not a folder of opaque
  files.** Explicit, identifiable, serializable state is a founding
  principle specifically so Git-friendliness, marketplace packages, and
  eventual collaborative editing become consequences of the architecture
  rather than separate features — see
  [ADR 0012](docs/decisions/architecture-decision-records/0012-project-state-as-a-versionable-graph.md).

For the full pitch, read [`docs/vision/project-goals.md`](docs/vision/project-goals.md)
and [`docs/vision/design-philosophy.md`](docs/vision/design-philosophy.md).

## Repository map

| Path | What's there |
|---|---|
| [`docs/vision/`](docs/vision/) | What Canary is, and what it refuses to compromise on |
| [`docs/architecture/`](docs/architecture/) | Subsystem-by-subsystem target design |
| [`docs/decisions/architecture-decision-records/`](docs/decisions/architecture-decision-records/) | The numbered decision log |
| [`docs/roadmap/`](docs/roadmap/) | What's scoped for which milestone, and what's done today |
| [`docs/development/`](docs/development/) | Build system, coding standards, repo layout |
| [`docs/ui/`](docs/ui/) | Target design for the (not-yet-built) editor |
| [`docs/research/`](docs/research/) | Sourced comparisons/evaluations behind the decisions above |
| [`docs/reviews/`](docs/reviews/) | Periodic critical self-audits, plus a living risk register |
| [`GOVERNANCE.md`](GOVERNANCE.md) | How decisions get made, succession/bus-factor plan |
| [`engine/`](engine/) | The engine crates themselves |
| [`tools/xtask/`](tools/xtask/) | Build orchestration (see [ADR 0005](docs/decisions/architecture-decision-records/0005-build-system-and-tooling.md)) |
| [`examples/`](examples/), [`tests/`](tests/), [`scripts/`](scripts/) | Currently near-empty; see each directory's README for why |

See [`docs/development/repository-structure.md`](docs/development/repository-structure.md)
for the fully annotated layout.

## Building

```sh
# Requires a stable Rust toolchain (rustup will pick up rust-toolchain.toml
# automatically, including the wasm32-wasip2 target).
cargo build --workspace
cargo test --workspace
cargo run -p canary-runtime
```

See [`docs/development/build-system.md`](docs/development/build-system.md)
for the full set of commands, including linting and the `xtask` build
orchestrator.

## What's actually implemented right now

Being direct about this, because architecture docs describe target designs
that intentionally run ahead of the code:

- `canary-core` — an `App`/subsystem bootstrap and structured logging.
- `canary-platform` — trait definitions for windowing/input, plus a
  headless implementation. No real windowing backend yet.
- `canary-ecs` — a minimal, generational-index entity/component store
  (`Send + Sync`-bounded, 64-bit generation counters), with synchronous
  system execution. **Not** the archetype-based, parallel design described
  in [`docs/architecture/core-runtime.md`](docs/architecture/core-runtime.md) —
  that's `v0.0.2`+ work, tracked explicitly in the roadmap.
- `canary-plugin-api` — the `Plugin` trait and a working, **versioned**
  native (Tier B, C-ABI) loader (explicit ABI version field, forward-
  extension hook — see [ADR 0009](docs/decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md)).
  The sandboxed WASM (Tier A) loader is designed but not implemented yet.
- `canary-runtime` — a headless boot harness proving the above compile,
  link, and run together.

Full detail: [`docs/roadmap/v0.0.1-roadmap.md`](docs/roadmap/v0.0.1-roadmap.md)
and [`docs/roadmap/milestones.md`](docs/roadmap/milestones.md).

## Contributing

Start with [`CONTRIBUTING.md`](CONTRIBUTING.md) and the
[Code of Conduct](CODE_OF_CONDUCT.md). See [`GOVERNANCE.md`](GOVERNANCE.md)
for how decisions get made and how that's expected to change as more
maintainers join. Security issues: see [`SECURITY.md`](SECURITY.md), not
a public issue.

See [`docs/reviews/`](docs/reviews/) for periodic critical self-audits of
this project's own architecture — including
[the one conducted right after this foundation was built](docs/reviews/2026-08-senior-architecture-review.md),
which surfaced several of the ADRs and this very `GOVERNANCE.md` file.

## License

[MIT](LICENSE).
