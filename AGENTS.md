# AGENTS.md — AI contributor guide for Canary Engine

This is the canonical instruction file for AI coding assistants working on
Canary. The human contributor guide is `CONTRIBUTING.md`; this file is the
machine-oriented complement, not a replacement. Tool-specific files
(`CLAUDE.md`, etc.) point here — do not duplicate this content into them.

## Verify, don't trust

- `git fetch` and check `git log` / repo state yourself before trusting any
  claimed commit hash or status. Remembered state has lost real work before.
- Read the relevant docs fresh before writing code: `docs/roadmap/status.md`
  (what's actually done), `docs/roadmap/v0.1.0-plan.md` (what's next, in
  dependency order), `docs/architecture/` for the subsystem you're touching,
  and the governing ADR(s). Short docs, read them whole.

## Hard gates (all four, every time, before anything is "done")

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- Toolchain: `rust-toolchain.toml` (`stable` via rustup). If only a
  version-suffixed apt toolchain exists (e.g. `cargo-1.91`), invoke it
  explicitly — bare `cargo` may be too old to parse `Cargo.lock`.
  Details: `docs/development/build-system.md` (including the
  transitive-dependency pin policy — follow it, don't improvise).

## Architecture rules (`docs/vision/design-philosophy.md`, ADRs)

- One crate per subsystem: `engine/canary-<name>`, added to the workspace
  `members` list. Never grow an existing crate to cover a new subsystem.
- Dependency direction: leaves (`canary-ecs`, `canary-platform`) know
  nothing about what's composed above them. Composition happens upward
  (`canary-runtime` ties subsystems together).
- Backend-facing traits never leak third-party types in public signatures.
- Rendering is `ash`-direct Vulkan (`canary-render` + `canary-render-vulkan`).
  There is no wgpu anywhere — do not reintroduce it (ADR 0016).
- Math types: `glam` for graphics-facing types. Physics backends own their
  own math internally and convert at the sync boundary.
- Branches: `dev` for work, `stable` for tagged releases ("harden then
  cut" — fully implement + test on `dev`; `stable` only gets releases).

## Code and test conventions (`docs/development/coding-standards.md`)

- `///` docs on every new public item; crate-level `//!` docs stating
  what's stub vs target design.
- `unsafe` needs a `// SAFETY:` invariant comment; outside `canary-platform`
  and the native plugin loader it is a review flag.
- Tests live with the code (`#[cfg(test)] mod tests`, descriptive
  snake_case names); core ECS invariants get `proptest`, not hand-picked
  cases. Integration tests spanning crates go under workspace `tests/`.
- Never silence the compiler to get past a gate (`as` casts to dodge type
  errors, `#[allow]` on lints, `unwrap()` on plausible runtime failures) —
  return typed errors (`thiserror`) instead.

## Docs move with code

- A behavior change updates `docs/architecture/*.md` in the same change; a
  stale architecture doc is a bug, not drift (`CONTRIBUTING.md`).
- `docs/roadmap/status.md` is a living document — update it when status
  changes. Check `docs/decisions/2026-09-review-triage.md`,
  `docs/decisions/2026-09-review-3-triage.md`,
  `docs/decisions/2026-09-review-4-triage.md`, and
  `docs/reviews/risk-register.md` before re-deciding something they cover.
- New architectural decisions get an ADR (next number, append-only, never
  rewrite history). New trusted-core dependencies need discussion + usually
  an ADR first.

## Commits

- Before every commit — not just big ones, in general: re-run all four
  gates on the final tree, confirm zero bottlenecks/regressions across
  repeated checks, and put anything beyond a trivial docs tweak through
  a strict review pass (goal match, code quality, security, hands-on QA
  by actually running it, missed-context mining). Fix findings first;
  a passing review is part of "done".
- Conventional Commits (`feat(ecs): ...`), focused scope, no unrelated
  cleanup in the same change.
- Every commit needs a `Signed-off-by` trailer (DCO): `git commit -s`.
- Never commit or push unless explicitly asked.
