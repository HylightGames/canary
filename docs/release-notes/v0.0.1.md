# Canary Engine `v0.0.1` — Release Notes

**Released:** 2026-08-03
**Tag:** `v0.0.1`
**Nature of this release:** foundational, not feature-bearing. There is no
rendering, no physics, no networking, no real windowing, and no editor in
this release, and that's on purpose — see "What this release deliberately
is not," below.

## What `v0.0.1` is

`v0.0.1` is the release where Canary Engine's engineering standards and
architectural core become real, working, and tested — not just described
in a document. Concretely, this release ships:

- **A governed, documented project**, not just a code dump: an MIT
  license, contribution guidelines with a DCO sign-off requirement, a
  code of conduct, a security policy, and — critically for a project that
  wants to outlast any one person — a governance document that names the
  single biggest risk a young open-source project like this one carries
  (no succession plan) and starts closing it.
- **Twelve Architecture Decision Records**, each stating not just what was
  decided but what was rejected and why: Rust as the primary language,
  the two-tier (sandboxed WASM / trusted native) plugin architecture, the
  rendering strategy, build tooling, versioning, networking model, crate
  versioning, plugin ABI versioning, the open question of component
  identity across languages, a native UI abstraction (`CanaryUI`) shared
  between editor and game, and project state as an explicit, versionable
  graph.
- **Two more architectural commitments recorded, not implemented**,
  reflecting the same "document the hard cross-cutting decisions before
  code accumulates around an unexamined assumption" discipline as
  rendering, physics, and networking before them:
  - `CanaryUI` — a trait-based UI abstraction, bootstrapped on `egui`,
    shared by the editor and by games built with Canary, so a HUD and an
    inspector panel are the same kind of thing to build rather than two
    parallel systems.
  - **Explicit, versionable project state** — a founding principle that
    scenes, assets, and other authored data should be identifiable,
    diffable, and mergeable from the start, so Git-friendly
    collaboration, marketplace packages, and eventual live collaborative
    editing become consequences of the architecture instead of features
    to build separately later.
- **A real, if intentionally minimal, engine core**: an `App`/subsystem
  bootstrap, structured logging, a generational-index ECS, and a native
  plugin loader — all compiling, all tested, all wired together in a
  headless boot harness you can actually run.
- **A plugin ABI that's actually safe to build on.** This is the part of
  this release we'd point to first if asked "why does this matter more
  than it sounds like it should": the native plugin interface now
  declares an explicit version and has a forward-extension mechanism, so
  a future addition to it fails with a clear error instead of silently
  corrupting memory in a third-party plugin's process. This was fixed
  *before* a single plugin exists in the wild — exactly when it was
  cheapest to fix, and the reason it's cheap here and expensive almost
  everywhere else this kind of thing gets caught.
- **A codebase that was actually audited against its own documentation**,
  not just written and left to drift. One real doc/code mismatch (a
  lifecycle hook that was documented but never implemented) was caught
  and fixed in the course of preparing this release. Every ADR was
  checked against current reality. Every "known limitation" note left by
  the architecture review was either resolved or explicitly, permanently
  deferred with a reason.

## What this release deliberately is not

Nothing here is an oversight — each of these was evaluated and pushed to
`v0.0.2` or later on purpose, with the reasoning recorded in
[`docs/roadmap/v0.0.1-roadmap.md`](docs/roadmap/v0.0.1-roadmap.md):

- **No rendering, physics, networking, or editor.** These were never in
  scope for `v0.0.1` under any version of the plan.
- **No archetype-based ECS storage or parallel job scheduler.** The
  current ECS is a correct, tested, but deliberately minimal placeholder.
- **No sandboxed WASM (Tier A) plugin loading.** Only the trusted native
  (Tier B) tier is implemented; the sandboxed, language-agnostic tier
  that's central to Canary's long-term modding vision is real, working
  Wasmtime integration away.
- **No real windowing backend.** `canary-platform` ships trait
  definitions and a headless implementation only.

If you were expecting to run a game against this release: not yet, and
that's the point. `v0.0.1` is the foundation everything else gets built
on, held to a standard that assumes people will still be relying on it in
ten years — not a tech demo.

## A note on how this release's scope came to be

This project's own roadmap originally set a higher bar for "unqualified
`v0.0.1`" than what shipped — it would have required the archetype ECS,
the Tier A loader, and real windowing to all be *implemented*, not just
designed, before the version number could drop its `-pre` suffix. That
bar was revised during this release, deliberately and on the record (see
[`v0.0.1-roadmap.md`](docs/roadmap/v0.0.1-roadmap.md#definition-of-done-for-the-unqualified-v001--revised)):
holding a foundation-laying release to a bar that keeps growing every
time a harder subsystem gets designed is how ambitious projects never
ship anything. What actually matters for a release meant to establish
standards is that everything *in* it is genuinely solid — not that it
includes everything anyone could imagine wanting.

## Try it

```sh
git clone <this repository>
cd canary
cargo build --workspace
cargo test --workspace
cargo run -p canary-runtime
```

See [`README.md`](README.md) for the full picture and
[`CONTRIBUTING.md`](CONTRIBUTING.md) to get involved.

## What's next: goals for `v0.0.2`

`v0.0.2` is where Canary's two hardest, most load-bearing pieces of
unfinished architecture get built — deliberately not in parallel with
new feature surface area elsewhere, so each gets real attention:

1. **The archetype-based ECS migration.** Replace the current
   `HashMap`-per-component-type placeholder with the cache-friendly,
   archetype-table design described in
   [`docs/architecture/core-runtime.md`](docs/architecture/core-runtime.md).
   Change-detection query filters — needed by both networking replication
   and editor hot-reload later — should be designed *as part of* this
   migration, not bolted on after.
2. **The Tier A (sandboxed WASM) plugin loader.** Wire in Wasmtime and
   WASI 0.2 component support per
   [`docs/architecture/plugin-system.md`](docs/architecture/plugin-system.md),
   delivering on the "language-agnostic, marketplace-safe" half of
   Canary's plugin vision that `v0.0.1` only designed.
3. **Resolve [ADR 0010](docs/decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)
   for real**, prototyped alongside the two efforts above rather than
   decided from a desk review: component identity needs a form that
   survives the Rust/WASM boundary before Tier A plugins can meaningfully
   declare which components they touch.

Real windowing (`winit`) is a candidate for `v0.0.2` as well if it's
needed to validate the above in a non-headless environment, but is not
this release's headline goal the way the three items above are. Nothing
beyond this list is committed to `v0.0.2` yet — see
[`docs/roadmap/future-roadmap.md`](docs/roadmap/future-roadmap.md) for
everything else still waiting on these three to land first.

## Thanks

This release was built and reviewed as a single continuous effort by its
founding architect. If you're reading this as an early contributor: the
governance document exists because we'd rather that stop being true
before it has to.
