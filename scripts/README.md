# Scripts

Small, standalone developer-convenience scripts that don't belong in
`xtask` (see [ADR 0005](../docs/decisions/architecture-decision-records/0005-build-system-and-tooling.md))
because they're either one-off/exploratory, or need to run *before* Cargo
is confirmed to even be working (see `setup-check.sh`) and so can't
themselves depend on the Cargo workspace being buildable.

Most new tooling should still go into `tools/xtask` rather than here — see
[`docs/development/build-system.md`](../docs/development/build-system.md#the-xtask-pattern)
for the reasoning (cross-platform behavior, testability, no second
language/runtime to install). This directory is deliberately kept small.

## `setup-check.sh`

A dependency-free shell script (no Cargo required to run it) that checks
whether a contributor's machine has what it needs before they run their
first `cargo build`. Run it with:

```sh
./scripts/setup-check.sh
```
