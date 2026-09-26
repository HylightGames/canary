# Benchmarking

Canary's performance is measured the same way its correctness is: in the
repository, on every change, rather than reconstructed from memory when
something feels slow. This document covers how the benchmarks are
organized, how to run them, and what the CI integration does with them.

Benchmarks are **not** one of the four hard gates
([`AGENTS.md`](../../AGENTS.md), [`CONTRIBUTING.md`](../../CONTRIBUTING.md)):
they report, they do not block. A measured regression is a question to
answer in review ("is this cost bought by something?"), not an automatic
rejection.

## Where benchmarks live

One `benches/` directory per benchmarked crate, next to that crate's own
code — the same locality rule as tests
([`coding-standards.md`](coding-standards.md)):

| Crate                | Suite                            | What it covers                                                                         |
| -------------------- | -------------------------------- | -------------------------------------------------------------------------------------- |
| `canary-ecs`         | `benches/ecs.rs`                 | spawn/despawn, component insert and remove (archetype moves), every query shape, change detection, resource access |
| `canary-scheduler`   | `benches/scheduler.rs`           | stage computation and dispatch for read-only, write-only, and mixed schedules            |
| `canary-transform`   | `benches/transform.rs`           | local-to-matrix composition, hierarchy edits, propagation across flat/wide/deep scenes    |
| `canary-assets`      | `benches/assets.rs`              | content-hash identity, the generational `AssetStore`, PNG decode through the real loader |
| `canary-loc`         | `benches/localization.rs`        | bundle construction, `.ftl` parsing, key resolution (hit, interpolated, plural, fallback, missing) |
| `canary-physics`     | `benches/physics.rs`             | Rapier body creation, fixed steps, pose reads, and the scheduled `physics_step_system`     |
| `canary-render-ecs`  | `benches/render_bridge.rs`       | scene extraction (fresh vs reused buffers), CPU bake, and the full propagate → extract → bake frame |

Rendering *draw* work is deliberately absent: it needs a real Vulkan
device, which is what this crate's `#[ignore]`-gated integration tests
are for (see [`ci.yml`](../../.github/workflows/ci.yml)). Everything
above is CPU work that runs identically with or without a GPU.

## Harness

Suites use [`divan`](https://docs.rs/divan) through
`codspeed-divan-compat`, a drop-in replacement declared as
`divan = { package = "codspeed-divan-compat", ... }` in each crate's
`[dev-dependencies]`. Run locally it behaves exactly like upstream
`divan` and prints a normal timing table; run under CodSpeed's runner it
reports instrumented measurements instead. No `#[cfg]` split, and no
second harness to keep in sync.

Fixtures are synthesized in-process. Notably, `canary-assets` benchmarks
encode a PNG into a temporary directory rather than reading
`engine/canary-assets/tests/fixtures/`: those are Git-LFS objects, and a
benchmark that silently measured 130 bytes of LFS pointer text would be
worse than no benchmark at all.

## Running them

```sh
# One crate, plain divan output
cargo bench -p canary-ecs

# One suite, filtered to matching benchmark names
cargo bench -p canary-transform --bench transform -- propagate

# A quick smoke run (one sample), useful when editing a benchmark
cargo bench -p canary-physics -- --sample-count 1 --sample-size 1
```

To reproduce what CI measures, use the CodSpeed CLI with the CPU
simulation instrument (requires `cargo-codspeed` and the CodSpeed
runner):

```sh
cargo codspeed build -m simulation -p canary-ecs
codspeed run --mode simulation -- cargo codspeed run
```

## In CI

[`.github/workflows/codspeed.yml`](../../.github/workflows/codspeed.yml)
builds the benchmarked crates and runs every suite under CodSpeed's CPU
simulation instrument on each push to `dev`/`stable` and each pull
request against them. Simulation measures simulated CPU work rather than
wall-clock time, so numbers from a shared, noisy CI runner are still
comparable run to run; results are posted back to the pull request.

The workflow lists the benchmarked crates explicitly (`cargo codspeed
build -p ...`) instead of building the workspace: an unscoped build would
compile every crate's dev-dependencies — `wasmtime`, `winit`, `naga` —
for suites that don't exist. **Adding a `benches/` directory to a new
crate means adding that crate to the workflow's list**, otherwise its
suite is built by nobody and quietly never measured.

## CI gating policy: regressions inform, failures block

CodSpeed is a performance observability and review signal, not a
correctness gate — deliberately separate from the build, test, clippy,
fmt, and security gates:

```text
Performance regression:
    informational → review it


Benchmark infrastructure failure:
    CI failure → fix it
```

Concretely:

- **A measured regression never fails the workflow.** When benchmarks
  run and upload successfully, the `benchmarks` job passes regardless
  of what the numbers say; CodSpeed posts the comparison (including
  regressions) to the pull request for review. A slower number is a
  question to answer in review ("is this cost bought by something?"),
  not proof the PR is invalid — small fluctuations on shared CI
  hardware are expected, so investigate meaningful regressions rather
  than optimizing every wiggle.
- **Anything that stops CodSpeed from executing still fails.** A
  benchmark that does not compile, a `cargo codspeed build`/`run`
  error, an invalid benchmark configuration, or a failed results
  upload fails the job exactly like any other CI step — there is
  deliberately no `continue-on-error` anywhere in `codspeed.yml`.
  This distinction is structural, not conventional: the workflow has
  no input that could turn a regression into a pass or an
  infrastructure failure into a pass, because the regression verdict
  is reported by CodSpeed itself while step failures are GitHub's.
- **CodSpeed must not become a required check.** The repository's
  branch rulesets (managed in GitHub's settings/API, not in this
  repository — there is intentionally no in-repo representation to
  edit) list only the correctness gates as required. Do not add the
  CodSpeed status to any ruleset's required checks: that single
  setting is what would turn an informational regression into a
  merge block, and flipping it would contradict this policy without
  changing a line of YAML.

## Writing a new benchmark

- Measure something a frame or a load actually does. The suites above are
  organized around real per-tick work (propagate, extract, bake, step),
  not around whichever function was easiest to call.
- Keep setup out of the measured region: `Bencher::with_inputs` builds
  the world, `bench_local_values` measures only what you do to it.
- `divan::black_box` the inputs and the results, or the optimizer will
  happily delete the work being measured.
- Size the workload so one iteration stays in the microsecond-to-few-
  milliseconds range. Instrumented runs are far slower than native ones,
  and a benchmark nobody waits for is a benchmark nobody runs.
- Prefer a couple of contrasting sizes (`args = [1_000, 10_000]`) over
  one: scaling behavior is usually the interesting part, and a
  super-linear path shows up immediately.
- Benchmarks are compiled by `cargo clippy --workspace --all-targets`,
  so they are held to the same lint bar as the rest of the workspace.
