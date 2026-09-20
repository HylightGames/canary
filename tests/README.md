# Integration Tests

This directory is for **cross-crate** integration tests — tests that
exercise more than one `engine/*` crate together (e.g., "does a plugin
loaded via `canary-plugin-api` correctly register a system that runs
against a `canary-ecs` `World`"). Single-crate unit tests live inside that
crate's own `src/` (standard Rust `#[cfg(test)]` modules) or its own
`tests/` directory, per normal Cargo convention — see
[`docs/development/coding-standards.md`](../docs/development/coding-standards.md#testing-expectations).

Cross-crate seams that already exist (scheduler ordering across
systems, physics → transform → render pixel chains) are currently
proven by `#[ignore]`-gated integration tests living in the owning
crates (`canary-render-ecs/tests/`, `canary-render-vulkan/tests/`),
run in dedicated CI jobs. Add tests *here* when a seam needs proving
independently of any single crate's harness — e.g. a plugin loaded
from a real `.wasm`/native library driving a scheduled `World`, or a
full-pipeline startup/shutdown sequence — rather than re-testing what
those in-crate suites already cover.
