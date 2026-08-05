# Git Workflow

Canary uses a small, two-branch model — deliberately lighter than
GitFlow's `main`/`develop`/`release/*`/`hotfix/*`/`feature/*` sprawl,
which is more process than an open-source engine at this stage needs.

```
main                    <- stable, always buildable, represents releases
 |
dev                     <- active development
 |
feature/<name>          <- temporary, merges back into dev
```

## `main`

The version of Canary people can trust.

- Always compiles, always passes CI.
- Only receives changes via a reviewed merge from `dev` at release time —
  never a direct commit.
- Represents releases: every commit on `main` corresponds to a tagged
  version (`v0.0.1`, `v0.0.2`, ..., `v1.0.0`), per
  [ADR 0006](../decisions/architecture-decision-records/0006-versioning-scheme.md).

```sh
git tag -a v0.0.2 -m "..."
git push origin v0.0.2
```

## `dev`

Where active development happens: new systems landing, architecture
changes, plugin work — everything that isn't yet a released version. CI
runs on `dev` exactly as it does on `main` (see
[`.github/workflows/ci.yml`](../../.github/workflows/ci.yml)); the
difference is what `main` is reserved for, not a lower quality bar on
`dev`.

## `feature/<name>` branches

For anything larger than a small fix, branch from `dev`, not `main`:

```
dev
 |
 +-- feature/render-graph
 +-- feature/canary-audio
 +-- feature/canary-ui
 +-- feature/plugin-sdk-tier-a
```

```
feature/<name>  --PR-->  dev  --release-->  main
```

Small, self-contained fixes can go straight to `dev` via a normal PR
without a named feature branch — this model exists to keep large,
in-progress work isolated, not to add ceremony to every change.

## Cutting a release

1. `dev` is in a state that satisfies the target version's roadmap
   document (e.g. [`v0.0.2-roadmap.md`](../roadmap/v0.0.2-roadmap.md))
   and its own definition of done.
2. Run through [`docs/reviews/RELEASE_CHECKLIST.md`](../reviews/RELEASE_CHECKLIST.md)
   (or the equivalent for the release in progress).
3. Merge `dev` into `main`.
4. Tag `main` at that commit (`git tag -a vX.Y.Z`), push the tag — this
   triggers [`.github/workflows/release.yml`](../../.github/workflows/release.yml).
5. Continue on `dev` for the next release.

## Why not just `main`

For a small application, a single branch is fine. For an engine, it
isn't: a single bad commit on the one branch everyone builds against can
break the renderer, the editor, plugins, the build system, and every
example simultaneously. Two branches cost almost nothing and mean
`main` never becomes, even briefly, a construction site.
