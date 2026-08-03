# Repository Structure

```
canary/
├── README.md, LICENSE, CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md, CHANGELOG.md
├── .github/                    CI workflow, issue/PR templates
├── .gitignore, .editorconfig, rust-toolchain.toml, rustfmt.toml, clippy.toml
├── Cargo.toml                  workspace root
│
├── docs/
│   ├── vision/                 what Canary is and why (rarely changes)
│   ├── architecture/           target + current subsystem design (changes with the engine)
│   ├── decisions/
│   │   └── architecture-decision-records/   numbered, append-only decision log
│   ├── roadmap/                 what's scoped for which milestone, and when things are "done"
│   ├── development/             this document and its neighbors: build, standards, layout
│   ├── ui/                      editor design (target — no editor exists yet)
│   └── research/                 sourced comparisons/evaluations that inform the above
│
├── engine/                      the engine itself, one crate per subsystem
│   ├── canary-core/              App bootstrap, logging, error conventions
│   ├── canary-platform/          windowing/input/filesystem/time traits (+ headless impl)
│   ├── canary-ecs/                entities, components, the (currently minimal) World
│   ├── canary-plugin-api/         Plugin trait, capability types, native (Tier B) loader
│   └── canary-runtime/            headless boot-harness binary tying the above together
│
├── tools/
│   └── xtask/                    build orchestration (see docs/development/build-system.md)
│
├── examples/                     will hold runnable example games/scenes (empty for now — see examples/README.md)
├── tests/                        cross-crate integration tests (empty for now — see tests/README.md)
└── scripts/                      small standalone dev scripts (see scripts/README.md)
```

## Why `engine/` is one crate per subsystem, not one big crate

This directly implements the "extremely modular" and "replaceable engine
subsystems" goals at the level of the build graph, not just in prose:

- A consumer who only wants the ECS and platform abstraction (say, a
  headless simulation tool with no rendering) depends on exactly
  `canary-ecs` and `canary-platform`, and pays zero compile time or binary
  size for the rendering/physics/networking crates that don't exist yet and
  won't be pulled in later just because they're "part of the engine."
- Swapping a subsystem's implementation (a different RHI, a different
  physics backend) is adding a new crate that satisfies an existing trait,
  not editing a monolith — see
  [`docs/architecture/engine-overview.md`](../architecture/engine-overview.md#the-two-structural-bets-this-engine-makes).
- Compile times scale with what you touch: changing `canary-ecs` doesn't
  force a rebuild of unrelated crates that merely depend on it through a
  stable trait/type surface (as long as that surface didn't change).

## Why `docs/` is this granular

Each subdirectory has a distinct *rate of change* and *audience*, which is
exactly why they're separated rather than combined:

- `vision/` should rarely change — it's the "why does this project exist"
  layer.
- `architecture/` changes when the engine's design changes, and should
  always reflect current target design (with explicit placeholder notices
  where implementation lags the target).
- `decisions/architecture-decision-records/` is append-only — old ADRs are
  marked superseded, never deleted or rewritten, because the historical
  reasoning is itself valuable (see
  [ADR 0001](../decisions/architecture-decision-records/0001-record-format.md)).
- `roadmap/` changes constantly (that's its job) and is explicitly allowed
  to be wrong/updated, unlike the other directories.
- `development/` (this file and its neighbors) is about *how to work on
  Canary*, not what Canary *is* — a distinct audience (contributors)
  from `vision/`/`architecture/` (which serve anyone trying to understand
  the project, including its own contributors).
- `ui/` is separated from `architecture/` because the editor is a
  consumer of the engine, built on top of it via the plugin system, not a
  subsystem the engine core depends on — it deserves its own section for
  the same reason a game built with Canary would, conceptually, even though
  in this case it's a first-party one.
- `research/` exists so that "why did we choose X over Y" claims made in
  `architecture/` and `decisions/` are traceable to actual sourcing, kept
  separate so the decision documents themselves stay focused on the
  decision rather than becoming literature reviews.

## Why `examples/`, `tests/`, and `scripts/` are currently near-empty

They're part of the suggested top-level structure and are created now so
their *purpose* is documented and their location is settled — but populating
them meaningfully depends on there being a renderer to build an example
around, cross-subsystem integration to test, or a recurring dev task worth
scripting, none of which exist yet in this foundation. Each directory has
its own `README.md` explaining this rather than being silently empty with
no explanation.

## Known limitations (added by the August 2026 architecture review)

- **The flat `engine/*` layout has no stated scaling plan.** Fine at six
  crates; a mature engine plausibly has dozens (multiple render/physics
  backends, one crate per asset-format importer, editor-panel crates).
  Cheap to reorganize now, expensive later — worth a named decision
  point in the roadmap rather than something 30 crates decide by
  accident. See
  [`docs/reviews/2026-08-senior-architecture-review.md`](../reviews/2026-08-senior-architecture-review.md),
  Finding 1.6, and risk register R-22.
- **`canary` and `canary-rs` are already taken on crates.io** by
  unrelated projects, verified during that review — meaning a future
  "one dependency, get the whole engine" meta-crate can't use the bare
  `canary` name. `canary-engine`'s availability wasn't confirmed either
  way. See the review, Finding 1.3, and risk register R-07 — this is
  time-sensitive in a way most other findings in this document aren't,
  since crates.io names are first-come-first-served with no reservation
  system.
