# Support

Canary is an early-stage game engine currently built from source. Support is best-effort and handled through the project's public GitHub spaces.

## Where to ask

| Need                                | Use                                                                                                                                |
| ----------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Report a reproducible bug           | [GitHub Issues](https://github.com/HylightGames/canary/issues)                                                                     |
| Ask a question or discuss an idea   | GitHub Discussions, if enabled for the repository                                                                                  |
| Propose a feature                   | Start with a GitHub Discussion for the design; open an issue once the scope is clear                                               |
| Propose an architectural change     | Open a design issue or discussion and follow the [ADR process](docs/decisions/architecture-decision-records/0001-record-format.md) |
| Report a build or toolchain problem | GitHub Issues with the environment details below                                                                                   |
| Report a security vulnerability     | Follow [`SECURITY.md`](SECURITY.md); do not use public issues or discussions                                                       |
| Report a conduct concern            | Follow the private-reporting guidance in [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)                                                |

Please search existing issues and discussions before opening a new one. Keep questions and reports focused so they can be answered efficiently.

## Reporting a bug

For engine or tooling bugs, include enough information for someone else to reproduce the problem.

Where applicable, provide:

* What you expected to happen
* What actually happened
* Steps to reproduce the issue
* The affected subsystem or crate
* The commit, branch, or release being used
* Relevant logs or error output

Small, self-contained reproductions are especially useful when practical.

## Build, test, and toolchain problems

When reporting a build, test, or toolchain problem, include:

* The commit, branch, or release being built
* Operating system and architecture
* Rust toolchain from `rust-toolchain.toml`
* Output of `rustc --version --verbose`
* The command that failed
* The complete error output in a fenced code block
* Whether `cargo build --workspace` or `cargo test --workspace` reproduces the problem
* A minimal reproduction or relevant project files, when practical

Remove secrets, private paths, credentials, and other sensitive information before posting logs.

## What not to use public spaces for

Do not disclose security vulnerabilities, private personal information, credentials, or sensitive conduct reports in public issues, discussions, or pull requests.

Use the private channels described in [`SECURITY.md`](SECURITY.md) and [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) instead.

For contribution workflow, development standards, and pull requests, see [`CONTRIBUTING.md`](CONTRIBUTING.md).

For project governance and architectural authority, see [`GOVERNANCE.md`](GOVERNANCE.md).
