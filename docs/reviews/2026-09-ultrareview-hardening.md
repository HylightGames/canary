# 2026-09 Ultrareview Hardening Pass

Point-in-time record of the whole-repository review hardening applied on
top of `v0.0.9`–`v0.0.11` (implemented on `dev`, untagged) during
September 2026. Two `/ultra` pipeline runs, one `/ultrareview` gate,
plus the fix batches they produced. Nothing here was released; it is
the "harden" half of "harden then cut".

## What was reviewed

Every engine crate, the example, `xtask`, all manifests plus the
lockfile, all CI/security/release/benchmark workflows, the
architecture/roadmap/development docs, benches, and the
`#[ignore]`-gated GPU/Xvfb suites (run on a real ICD where applicable).
Prior triages (`docs/reviews/triage/`, `risk-register.md`) were checked
first and were not re-decided.

## Findings fixed in this pass

- **UB: `panic!` inside the `extern "system"` Vulkan validation
  callback** (`canary-render-vulkan/src/device.rs`) — unwinding across
  FFI into the driver is undefined behavior. Now logs and aborts
  (still fail-loud, no unwind). Same pass corrected a false comment:
  `rapier::math::Vector` is `glam` 0.33.8's `Vec2` (via the
  parry→glamx re-export), not a distinct newer type.
- **`expect()` in `Drop for VulkanCommandEncoder`** — fired on the
  abandoned-during-unwind path (abort). Now the error is ignored and
  the buffer is still freed.
- **Release NaN poisoning** — the non-finite `Transform` guard was
  `debug_assert`-only, so release builds wrote NaN into
  `GlobalTransform` and churned change ticks every frame. Now an
  unconditional skip-and-retain (mirroring the physics sync guard),
  with a release-gated regression test.
- **Locale budget bypass** — decode budgets were checked from
  filesystem metadata but the read used unbounded `read_to_string`,
  so a path whose metadata understated its contents bypassed the
  budget. Now a capped `take(budget + 1)` reader enforces the budget
  during the read itself, with a proof test.
- **`u64→usize` truncation** in the PNG `read_rgba8` path (latent
  out-of-bounds read on 32-bit) — now a checked conversion.
- **Hierarchy removal** — raw `World::despawn` orphaned `Parent` links
  and left stale handles in survivors' `Children` lists (a deliberate
  crate boundary: `canary-ecs` must not name hierarchy types). New
  `despawn_subtree` helper in `canary-transform` (detach, post-order
  despawn, forged-cycle termination) plus docs stating `Children` is
  external-consumer metadata, not propagation input.
- **`World::entity_count` O(n)→O(1)** — incrementally maintained by
  `spawn`/`despawn`; `remove` stays `Option` (idempotent by design,
  now documented with a pinning test).
- **Index exhaustion** — body/collider slot indices moved from
  `wrapping_add` to fail-loud `checked_add` (wrapping would alias a
  live slot); generation bumps remain wrapping by policy.
- **Plugin fuel / core shutdown** — fuel re-armed before every guest
  entry point (silent no-op would mean an unenforced budget);
  `App` shutdown now runs what started even when init fails, and is
  panic-safe (shutdown-then-resume, first panic propagates).
- **CI/release** — release `NOTES_FILE` pointed at a nonexistent
  repo-root path (every release silently fell back to auto-generated
  notes); release checkout lacked `lfs: true` while testing
  LFS-tracked fixtures; `--locked` and `timeout-minutes: 30`
  everywhere; `cargo audit --deny warnings` with the single accepted
  `RUSTSEC-2026-0192` ignore; hash-pinned `moz.l10n` requirements;
  Vulkan CI job runs all three `--test` targets.

## Verification

- `cargo build --workspace`, `cargo test --workspace` (370 passed,
  0 failed), `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`, `cargo doc --workspace --no-deps`
  (zero warnings), `wasm32-wasip2` check — all clean, both
  `winit-backend` states where applicable.
- **Miri on `canary-ecs`** (nightly, `PROPTEST_CASES=4`,
  `-Zmiri-disable-isolation` for proptest's file persistence):
  full lib suite 42/42 passed — the `column_pair_mut` header-alias
  pattern behind `query2_mut` shows no undefined behavior. (The one
  Miri failure seen without `-Zmiri-disable-isolation` was proptest
  calling `getcwd`, an isolation limit, not project UB.)
- GPU-gated suites green on a real ICD at review time.

## Deliberately deferred (owner decisions)

- Rapier dead `BodySlot`s are never removed (unbounded growth under
  create/destroy churn); removal is behavior-preserving but coupled
  to documented recovery invariants.
- `VulkanInitError` variant shapes changed (pre-1.0 breaking change —
  must be called out when committed).
- Release-only `debug_assert` content guards; `PhysicsClock.fixed_dt`
  decoy field; `f32 SimulationTime` drift; deprecated
  `AssetHandle::with_type` aliasing hazard; `VulkanDevice::Drop`
  unconditional panic (documented tradeoff).
- Known measured perf taxes: fresh OS threads per scheduler stage,
  per-tick allocations in query/propagation/physics paths, no
  `ExtractScratch` reuse on mesh/textured bakes, per-tick velocity
  re-application vs solver sleep.
