# Contributing to Canary Engine

Thank you for considering contributing to Canary.

Canary is a long-term, first-principles game engine project. The codebase is expected to grow across many subsystems and over many years, so contribution guidelines are designed to preserve architectural coherence without making ordinary contributions unnecessarily difficult.

The core principle is simple:

> **Small changes should be easy to contribute. Large or irreversible changes should be deliberate.**

Please read the [Code of Conduct](CODE_OF_CONDUCT.md) before contributing.

## Before you start

Before making a change, make sure you understand the part of the project you are changing.

Start with:

1. [`docs/vision/`](docs/vision/) for Canary's goals, principles, and non-negotiable constraints.
2. [`docs/architecture/`](docs/architecture/) for the subsystem or boundary you intend to modify.
3. [`docs/decisions/architecture-decision-records/`](docs/decisions/architecture-decision-records/) for decisions that may already govern the approach.
4. [`docs/roadmap/status.md`](docs/roadmap/status.md) to understand whether the area is implemented, experimental, architected, or planned.

### Check existing decisions first

If an existing ADR defines the architecture you are working within, do not silently introduce a contradictory implementation.

A change that intentionally reverses or replaces an existing architectural decision should include a corresponding ADR update or superseding ADR.

This keeps the implementation, documentation, and decision history aligned.

### Align before investing heavily

For non-trivial work, open an issue or discussion before writing a large amount of code.

This is especially important for:

* New subsystems
* New dependencies in the trusted core
* Public API redesigns
* Plugin ABI changes
* Changes to project-state representation
* Changes to execution, threading, or scheduling models
* Changes that affect multiple architectural boundaries

A short design discussion is cheaper than reviewing a complete implementation built on the wrong assumption.

For obvious bug fixes and small, self-contained changes, you can generally proceed directly to a PR.

---

## Choosing the right contribution path

| Contribution                             | Recommended path                                                                  |
| ---------------------------------------- | --------------------------------------------------------------------------------- |
| Fix an obvious bug                       | Open a PR directly                                                                |
| Make a small, self-contained improvement | Open a PR; an issue is helpful when context is needed                             |
| Add a non-trivial feature                | Open an issue first and describe the intended approach                            |
| Change an architectural decision         | Discuss the change and add or amend an ADR                                        |
| Add a new subsystem                      | Discuss the design before implementation; prototype independently where practical |
| Add or change a plugin capability        | Review the plugin architecture and discuss ABI or sandbox implications first      |
| Make a broad API or data-model change    | Open a design issue or discussion before implementation                           |
| Report a security vulnerability          | Follow [`SECURITY.md`](SECURITY.md), not the public issue tracker                 |

When in doubt, **start with discussion rather than code** for changes that are difficult to reverse.

---

## Development workflow

Canary uses `dev` for active development and `main` for stable releases.

See [`docs/development/git-workflow.md`](docs/development/git-workflow.md) for the complete branching model.

### 1. Fork and clone

Fork the repository, then clone your fork:

```sh
git clone git@github.com:<your-user>/canary.git
cd canary
```

Add the upstream repository if you intend to keep your fork synchronized:

```sh
git remote add upstream git@github.com:HylightGames/canary.git
```

### 2. Install the pinned toolchain

Canary's required Rust toolchain is defined in [`rust-toolchain.toml`](rust-toolchain.toml).

Using `rustup` will automatically select the pinned toolchain when working in the repository.

### 3. Create a branch

Create your branch from `dev`:

```sh
git checkout dev
git pull upstream dev
git checkout -b feature/<short-name>
```

Use the branch type appropriate to the work, following [`docs/development/git-workflow.md`](docs/development/git-workflow.md).

### 4. Build the workspace

```sh
cargo build --workspace
```

### 5. Run the test suite

```sh
cargo test --workspace
```

Run the relevant tests during development as well as the complete workspace suite before opening a PR.

### 6. Format and lint

CI enforces formatting and Clippy warnings:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

A PR should not rely on CI to discover formatting or lint failures.

### 7. Run the relevant validation

Depending on the change, additional validation may be required.

For changes involving build orchestration, asset processing, development tooling, or CI, also review:

* [`docs/development/build-system.md`](docs/development/build-system.md)
* `tools/xtask`

For subsystem-specific work, run the tests and checks documented by that subsystem.

---

## Making a change

A useful Canary change generally follows this sequence:

```text
Understand
   ↓
Check existing architecture and ADRs
   ↓
Discuss when the change is non-trivial
   ↓
Implement
   ↓
Test and validate
   ↓
Update documentation
   ↓
Open a focused PR
   ↓
Review
   ↓
Merge
```

The exact process can be lighter for a small bug fix and more deliberate for an architectural change.

The important part is that **code, documentation, tests, and architectural decisions should move together**.

---

## Pull requests

Keep pull requests focused and reviewable.

A good PR should make it easy for a reviewer to answer:

1. What changed?
2. Why was it changed?
3. What architectural assumptions does it rely on?
4. How was it tested?
5. Does any documentation or decision record need to change?

### Keep scope tight

Avoid unrelated cleanup in the same PR.

For example, a change to the ECS should not also introduce an unrelated logging redesign simply because both happen to be nearby in the codebase.

Smaller PRs are easier to reason about, review, revert, and preserve in the project's history.

### Explain why

The code already shows **what** changed.

The PR description should explain:

* Why the change is necessary
* Why this approach was chosen
* Relevant alternatives considered
* Any important trade-offs
* Known limitations
* How the change was validated

### Keep documentation synchronized

Documentation is part of the implementation.

When a code change changes an architectural contract, public API, data model, workflow, or subsystem behavior, update the relevant documentation in the same PR.

A PR that leaves known documentation contradictions behind should be treated as incomplete rather than creating future documentation debt.

### Include tests

New logic should normally include tests.

Bug fixes should include a regression test where practical, particularly when the failure could otherwise return unnoticed.

Tests are part of the change, not an optional follow-up.

---

## When an ADR is required

Use an [Architecture Decision Record](docs/decisions/architecture-decision-records/) when a decision is:

* Significant
* Cross-cutting
* Expensive to reverse
* Likely to constrain future architecture
* Relevant to public or plugin-facing contracts

Examples include:

* Adding a dependency to the trusted core
* Changing the plugin ABI
* Changing a major serialization or project-state model
* Introducing or replacing a core subsystem boundary
* Changing the default backend for a core system
* Changing execution, scheduling, or ownership models in a way that affects multiple subsystems

An ADR should record the **decision and its reasoning**, not merely describe the implementation.

At minimum, it should make clear:

* The problem being solved
* The chosen approach
* Alternatives considered
* Why those alternatives were rejected
* Important consequences and trade-offs

See [`0001-record-format.md`](docs/decisions/architecture-decision-records/0001-record-format.md) for the ADR format.

### ADRs evolve too

An existing ADR is not a permanent prohibition against change.

If new evidence shows that an architectural decision should be replaced, document the new reasoning and supersede or amend the previous decision rather than quietly drifting away from it.

The goal is a trustworthy decision history, not architectural fossilization.

---

## Commit messages

Canary uses [Conventional Commits](https://www.conventionalcommits.org/).

Use:

```text
<type>(<optional scope>): <short summary>

<optional body explaining why>
```

Common types include:

```text
feat
fix
docs
refactor
perf
test
build
ci
chore
deps
```

Examples:

```text
feat(ecs): add cached component queries
```

```text
fix(ecs): invalidate stale archetype query state

Previously cached query results could survive a structural
change and return stale matches. This invalidates the affected
query state when the archetype layout changes.
```

Prefer commit messages that communicate intent rather than narrating mechanical details.

The repository's history is part of its long-term documentation, so commit messages should remain useful after the original context is gone.

---

## Coding standards

See [`docs/development/coding-standards.md`](docs/development/coding-standards.md) for the full Rust coding guide.

At a minimum:

* `rustfmt` is required.
* Clippy warnings are treated as errors in CI.
* Public APIs should have appropriate Rust documentation.
* `unsafe` code requires a `// SAFETY:` comment explaining the invariant that makes the operation sound.
* Avoid unnecessary unsafe code, hidden global state, and implicit cross-subsystem coupling.
* Prefer clear ownership and explicit boundaries over clever abstractions that obscure them.

When modifying an existing subsystem, follow its established conventions unless the purpose of the change is to deliberately revise those conventions.

---

## Multi-language contributions

Canary's engine core is written in Rust, as recorded in [ADR 0002](docs/decisions/architecture-decision-records/0002-primary-language-selection.md).

The extension layer is intentionally broader:

* WebAssembly Components are intended for sandboxed community and marketplace content.
* Trusted native plugins can use the native C ABI where the architecture permits it.
* Other supported languages can participate through those defined interfaces rather than becoming direct dependencies of the Rust core.

Language choice does not override interface contracts, sandbox requirements, or compatibility expectations.

For plugin work, start with:

* [`docs/architecture/plugin-system.md`](docs/architecture/plugin-system.md)
* [`docs/architecture/scripting-system.md`](docs/architecture/scripting-system.md)

---

## Documentation contributions

Documentation is a first-class part of Canary's development process.

You can contribute documentation independently, including:

* Architecture documentation
* ADRs
* Tutorials and examples
* Build and development guides
* Roadmap and status information
* API documentation
* Contributor documentation

When documentation describes implementation behavior, keep it consistent with the current code.

When documentation describes future architecture, make that status explicit rather than presenting planned systems as implemented functionality.

---

## Issue reports

A useful issue should contain enough context for someone else to reproduce or reason about the problem.

For bug reports, include where practical:

* What you expected to happen
* What actually happened
* Reproduction steps
* Relevant logs or error output
* The affected platform or environment
* The relevant Canary version or commit

Before opening a new issue, search existing issues for duplicates.

See `.github/ISSUE_TEMPLATE/` for the available templates.

---

## Security issues

Do not disclose security vulnerabilities through public issues or pull requests.

Follow [`SECURITY.md`](SECURITY.md) for the appropriate reporting process.

This includes vulnerabilities involving, where applicable:

* Plugin or ABI boundaries
* Sandbox escapes
* Malicious project data
* Remote or networked functionality
* Dependency vulnerabilities
* Memory-safety or unsafe-code issues

---

## Licensing and provenance

Canary is distributed under the [MIT License](LICENSE).

By submitting a contribution, you agree that your contribution may be distributed under the project's license. If you are contributing on behalf of an employer or another organization, make sure you are authorized to do so.

### Developer Certificate of Origin

Canary requires the [Developer Certificate of Origin (DCO)](https://developercertificate.org/) for commits.

Each commit must include a `Signed-off-by` trailer indicating that you have the right to submit the contribution under the project's license.

The easiest way to add it is:

```sh
git commit -s
```

You can also add the trailer manually:

```text
Signed-off-by: Your Name <your.email@example.com>
```

Do not sign a commit you are not authorized to submit.

The DCO requirement exists to establish clear contribution provenance without requiring a separate Contributor License Agreement.

---

## Issue and pull request templates

Available templates live in:

```text
.github/ISSUE_TEMPLATE/
.github/PULL_REQUEST_TEMPLATE.md
```

Please use them when applicable.

They are designed to capture the information maintainers need for efficient review and reduce unnecessary back-and-forth.

---

## Maintainer review

Maintainers may request changes when a contribution:

* Conflicts with an existing architectural decision
* Introduces unnecessary coupling
* Expands scope beyond the stated purpose
* Lacks appropriate validation
* Leaves public behavior or documentation inconsistent
* Adds complexity without a clear benefit
* Introduces a significant architectural change without documenting the decision

A request for changes is not necessarily a rejection of the underlying idea. Large ideas may simply need a different design, a smaller first step, or an explicit architectural decision before implementation.

---

## A note on architectural consistency

Canary is intentionally being built over a long time horizon.

That means not every technically correct change is necessarily a good change for the project.

When contributing, prefer solutions that are:

* Explicit
* Testable
* Reversible where practical
* Clearly bounded
* Consistent with existing interfaces
* Understandable to future contributors

Avoid solving a local problem by creating a global architectural dependency unless the broader trade-off has been considered.

The goal is not to prevent experimentation. It is to make sure experimentation does not silently become permanent architecture.

---

## Getting help

For questions about how something works, start by checking the relevant documentation and existing ADRs.

For design questions or non-trivial proposed changes, use the project's issue or discussion mechanisms before beginning a large implementation.

For security concerns, use [`SECURITY.md`](SECURITY.md).

---

## The standard for a good contribution

A good Canary contribution is not necessarily large or sophisticated.

It is a change that:

> **Solves a real problem, fits the architecture, explains its reasoning, includes appropriate validation, and leaves the project easier to understand than it was before.**
