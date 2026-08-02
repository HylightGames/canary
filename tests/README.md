# Integration Tests

This directory is for **cross-crate** integration tests — tests that
exercise more than one `engine/*` crate together (e.g., "does a plugin
loaded via `canary-plugin-api` correctly register a system that runs
against a `canary-ecs` `World`"). Single-crate unit tests live inside that
crate's own `src/` (standard Rust `#[cfg(test)]` modules) or its own
`tests/` directory, per normal Cargo convention — see
[`docs/development/coding-standards.md`](../docs/development/coding-standards.md#testing-expectations).

Currently empty because there isn't yet a second crate-boundary interaction
worth testing beyond what each crate's own unit tests already cover — the
`canary-runtime` binary crate exercises `canary-core` + `canary-ecs` +
`canary-plugin-api` together today, which currently serves as the
"integration test" in spirit, even though it isn't wired up as a
`#[test]`. As more subsystems land (a real plugin loaded from a `.wasm` or
native library, a physics backend synchronizing into the ECS), add
integration tests here that specifically exercise the seams between
crates, rather than re-testing what a single crate's own unit tests
already cover.
