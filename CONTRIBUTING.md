# Contributing to Canary Engine

Thank you for considering contributing. Canary is a multi-year, first-
principles game engine project — the guidance below exists to keep a project
of that scope coherent as more people join it. Please also read the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Before you start

- Read [`docs/vision/`](docs/vision/) to understand what Canary is trying to
  be and, just as importantly, what it refuses to compromise on.
- Read [`docs/architecture/`](docs/architecture/) for the subsystem you want
  to touch.
- Check [`docs/decisions/architecture-decision-records/`](docs/decisions/architecture-decision-records/)
  — if a decision already has an ADR, a PR that quietly contradicts it needs
  a new ADR proposing the change, not just code that goes a different way.
- For anything nontrivial, open an issue or discussion before writing code.
  This project would rather spend ten minutes aligning on approach than
  review (and likely reject) a fully-formed PR built on a mistaken premise.

## What kind of contribution is this?

| You want to... | Do this |
|---|---|
| Fix an obvious bug | Open a PR directly, reference the issue if one exists |
| Add a small, self-contained feature | Open an issue first describing the approach |
| Change an architectural decision | Open a PR that adds/amends an ADR under `docs/decisions/` *before* or *alongside* the code |
| Propose a new subsystem or plugin | Start a discussion; large additions should be prototyped as an out-of-tree plugin first where possible (see `docs/architecture/plugin-system.md`) |
| Report a security issue | See [`SECURITY.md`](SECURITY.md) — not a public issue |

## Development workflow

1. Fork and clone the repository.
2. Install the pinned toolchain (`rust-toolchain.toml` will pick this up
   automatically if you use `rustup`).
3. Build the workspace:
   ```sh
   cargo build --workspace
   ```
4. Run tests before and after your change:
   ```sh
   cargo test --workspace
   ```
5. Format and lint (CI enforces both, with warnings denied on lint):
   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   ```
6. For anything touching build orchestration, asset cooking, or CI, see
   [`docs/development/build-system.md`](docs/development/build-system.md) and
   the `xtask` crate under `tools/xtask`.

## Commit messages

Canary uses [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<optional scope>): <short summary>

<optional longer body explaining WHY, not just what>
```

Common types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`,
`ci`, `chore`. Example:

```
feat(ecs): add generational entity IDs to canary-ecs

Generational indices let us detect use-after-despawn without a full
archetype migration, which we're deferring to a later pre-release
(see docs/roadmap/v0.0.1-roadmap.md).
```

This isn't bureaucracy for its own sake: a multi-year project's git log is
itself documentation, and future contributors (including future-you) will
grep it.

## Pull requests

- Keep PRs focused. A PR that touches the ECS *and* rewrites the logging
  format is two PRs.
- Explain **why**, not just what — the diff already shows what.
- Update relevant docs in the same PR as the code change. A code change that
  invalidates `docs/architecture/*.md` without updating it is treated as
  incomplete, not just "docs debt to clean up later."
- New public APIs should have doc comments (`///`) — see
  [`docs/development/coding-standards.md`](docs/development/coding-standards.md).
- Tests are expected for new logic, not just for bug fixes.

## Architecture Decision Records (ADRs)

Significant, hard-to-reverse decisions (a new dependency in the trusted core,
a change to the plugin ABI, a new default backend) get an ADR in
`docs/decisions/architecture-decision-records/`, numbered sequentially. See
[`0001-record-format.md`](docs/decisions/architecture-decision-records/0001-record-format.md)
for the template and the bar for "does this need an ADR."

## Licensing

By contributing, you agree that your contributions are licensed under the
project's [MIT License](LICENSE). If you're contributing on behalf of an
employer, please make sure you're authorized to do so.

## Multi-language contributions

Canary's engine core is Rust (see ADR
[0002](docs/decisions/architecture-decision-records/0002-primary-language-selection.md)),
but the plugin/mod layer is explicitly designed to accept other languages
through WASM components and, for trusted native plugins, a C ABI. If you want
to contribute a plugin in another language rather than to the Rust core
itself, see `docs/architecture/plugin-system.md` and `docs/architecture/scripting-system.md` —
those contributions don't need to match the Rust coding standards below,
just the ABI/interface contracts.

## Coding standards (Rust core)

See [`docs/development/coding-standards.md`](docs/development/coding-standards.md)
for the full guide. In short: `rustfmt` and `clippy` are non-negotiable and
enforced in CI; `unsafe` blocks require a `// SAFETY:` comment explaining the
invariant that makes them sound; public items need doc comments.

## Issue and PR templates

See `.github/ISSUE_TEMPLATE/` and `.github/PULL_REQUEST_TEMPLATE.md`. Using
them isn't mandatory, but skipping them usually just means we ask the same
questions back in a review comment.
