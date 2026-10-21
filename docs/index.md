# Canary Engine

Canary is a game engine written in Rust, built in public and currently
pre-1.0. This site is the design record: how the engine is put together,
what has actually landed, and what is still missing.

There is no editor and no downloadable build yet. The way to run it today
is to build the workspace from source — see
[Build system](development/build-system.md).

## Start here

- [Project goals](vision/project-goals.md) — what Canary is for, and what it
  deliberately is not.
- [Engine architecture overview](architecture/engine-overview.md) — the
  subsystem map.
- [Status](roadmap/status.md) — what is implemented right now, kept current.
- [v0.1.0 plan](roadmap/v0.1.0-plan.md) — the current milestone and its exit
  criteria.
- [Risk register](reviews/risk-register.md) — known gaps, stated plainly.

## How this documentation is organized

- **Vision** — design philosophy and long-term direction.
- **Architecture** — one document per subsystem, describing what exists.
- **Decisions** — append-only architecture decision records (ADRs).
- **Roadmap** — per-version scope, status, and the current plan.
- **Development** — build, test, and contribution mechanics.
- **Research** — evaluations that informed a decision.
- **Reviews** — architecture and release reviews, including findings that
  have not been acted on yet.
- **Release notes** — what changed in each tagged version.

Documents here are living: a behaviour change updates the document that
describes it in the same change.
