# Coding Standards

These apply to the trusted Rust core (`engine/*`, `tools/*`). Plugins and
mods authored in other languages follow the ABI/interface contract in
[`docs/architecture/plugin-system.md`](../architecture/plugin-system.md),
not these standards.

## Non-negotiable, CI-enforced

- `cargo fmt --all -- --check` must pass. Formatting is not a matter of
  personal preference on a project with many contributors — see
  [`rustfmt.toml`](../../rustfmt.toml) for the (deliberately close-to-default)
  configuration.
- `cargo clippy --workspace --all-targets -- -D warnings` must pass. See
  [`clippy.toml`](../../clippy.toml) for thresholds.
- `cargo test --workspace` must pass.
- New public items (functions, types, traits) need doc comments (`///`).
  `cargo doc` should produce useful documentation without hand-holding —
  this matters doubly for a project explicitly aiming to be legible to
  AI-assisted contribution (see
  [`docs/vision/design-philosophy.md`](../vision/design-philosophy.md#ai-ready-architecture)).

## Naming

- Crates are named `canary-<subsystem>` (`canary-ecs`, `canary-platform`,
  ...) — consistent, greppable, and namespaced against collisions with
  unrelated crates on crates.io once this is published.
- Public types and traits use standard Rust conventions (`UpperCamelCase`
  types, `snake_case` functions/modules) — no project-specific deviations
  from ecosystem norms; a contributor's existing Rust muscle memory should
  work here unmodified.

## `unsafe` code

`unsafe` blocks are expected primarily at two boundaries: the platform
abstraction layer (`canary-platform`, wrapping OS APIs) and the Tier B
native plugin loader (`canary-plugin-api`, wrapping `libloading` and the C
ABI boundary). Everywhere else, `unsafe` should be rare enough that its
presence is itself a signal something needs a closer look in review.

Every `unsafe` block requires a `// SAFETY:` comment immediately above it,
stating the invariant that makes the block sound — not just restating what
the code does. Example:

```rust
// SAFETY: `handle` was returned by a successful `Library::new` call above
// and has not been dropped; the symbol name matches the `#[no_mangle]`
// export contract documented in docs/architecture/plugin-system.md.
let entry: Symbol<PluginEntryFn> = unsafe { library.get(b"canary_plugin_entry") }?;
```

A PR introducing `unsafe` without a `// SAFETY:` comment should be treated
as incomplete, not just under-documented.

## Error handling

See [`docs/architecture/core-runtime.md`](../architecture/core-runtime.md#error-handling-conventions)
for the full rationale. In short:

- Library crates: typed errors via `thiserror`-derived enums.
- Binary/application crates: a boxed/dynamic error type with context added
  at the boundary is acceptable.
- Panics are for programmer errors/violated invariants only, never for
  expected failure modes (missing file, failed plugin load, malformed
  input) — those are `Result`s.

## Module and crate structure

- Each crate's `src/lib.rs` (or `main.rs` for binaries) should have a
  module-level doc comment (`//!`) explaining what the crate is for and,
  where relevant, explicitly stating what's a placeholder versus what's the
  target design — see `engine/canary-ecs/src/lib.rs` for the pattern this
  is modeled on.
- Prefer several small, focused modules over one large file. There is no
  hard line-count rule; if you're scrolling to find something, it's a
  signal, not yet a violation.

## Testing expectations

- New logic gets tests in the same PR, not "added later." A bug fix should
  generally include a regression test.
- `canary-ecs`'s core invariants (entity IDs never alias after despawn +
  respawn, components round-trip through insert/query) are tested with
  `proptest` rather than a fixed list of example cases, since these are
  exactly the kind of invariant that a specific hand-picked example can miss.
- Integration tests that exercise more than one crate together live under
  the workspace's `tests/` directory (see
  [`repository-structure.md`](repository-structure.md)) rather than inside
  any single crate.

## Documentation-code consistency

A PR that changes behavior described in `docs/architecture/*.md` updates
that document in the same PR — see
[`CONTRIBUTING.md`](../../CONTRIBUTING.md#pull-requests). Treat a stale
architecture doc as a bug, not as acceptable drift.

## Commit and PR conventions

Covered in [`CONTRIBUTING.md`](../../CONTRIBUTING.md#commit-messages) rather
than duplicated here.
