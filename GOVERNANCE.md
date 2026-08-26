# Governance

Canary is intended to be a long-lived open-source project. Its governance therefore needs to answer questions that become increasingly important as the project grows:

* Who can make which decisions?
* How are maintainers added?
* How are architectural disagreements resolved?
* What happens if a maintainer becomes unavailable?
* How does the project's governance evolve without requiring a crisis to force the answer?

Canary's governance is intentionally lightweight while the project is small. It is also intentionally explicit: the process should grow with the project rather than being invented during a dispute.

This document describes the **current model and the direction in which it is expected to evolve**. It does not pretend the project has an organization, foundation, or maintainer structure that does not yet exist.

## Current state

**Canary currently has one active maintainer/architect.**

No formal maintainer team exists yet.

This is the project's current reality and its largest governance risk. The project is therefore deliberately documenting succession, authority, and future delegation before those questions become urgent.

The governance model below should be understood in that context: it is a working foundation for a one-maintainer project, designed to evolve as trusted contributors join.

---

## Roles

### Contributor

A **Contributor** is anyone who participates in the project through issues, discussions, documentation, code, testing, design, or other project work.

Contributors do not require special status to propose changes or participate in architectural discussions.

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the contribution process.

### Maintainer

A **Maintainer** is a trusted contributor with repository responsibilities.

Depending on their scope, a maintainer may:

* Review and approve pull requests
* Merge changes
* Maintain one or more subsystems
* Manage project infrastructure
* Participate in architectural decisions
* Hold ownership over specific repository paths through [`CODEOWNERS`](.github/CODEOWNERS)

Maintainer authority is delegated trust, not permanent ownership. It exists to serve the project and may be expanded or reduced as the project's needs change.

### Architect

The **Architect** is responsible for maintaining the coherence of Canary's overall technical direction and for making or recording binding architectural decisions under the current governance model.

The architect:

* Makes significant architectural decisions
* Ensures those decisions are recorded as ADRs
* Maintains consistency between architecture and implementation
* Resolves architectural deadlocks under the current single-architect model
* May delegate subsystem-level authority as the maintainer structure grows

At present, the architect and maintainer roles are held by the same person.

This is a temporary organizational state, not a design goal.

---

## Decision making

Not every decision requires the same level of process.

### Ordinary implementation decisions

Bug fixes, documentation changes, small features, refactors, and other non-architectural work follow the normal pull request process described in [`CONTRIBUTING.md`](CONTRIBUTING.md).

These decisions should be made as close to the relevant code and maintainers as practical.

### Architectural decisions

Architectural decisions are decisions that significantly constrain future implementation, introduce difficult-to-reverse dependencies, alter public contracts, or affect multiple subsystem boundaries.

Examples include:

* Changes to the plugin ABI
* Major ECS or scheduling changes
* New dependencies in the trusted core
* Changes to the project-state model
* New or replaced core subsystem boundaries
* Changes to execution or ownership models spanning multiple systems

Significant architectural decisions are recorded through the [ADR process](docs/decisions/architecture-decision-records/0001-record-format.md).

### Current decision model

While Canary has one architect, the architect has final authority over architectural decisions.

This is intentionally **not** a formal voting system.

With one architect, a voting procedure would create the appearance of distributed authority without actually providing it. The current model instead makes authority explicit while requiring important decisions to be documented, reviewable, and open to future revision.

Architectural authority does not make an ADR immutable. A decision may be revisited when new evidence, implementation experience, or project requirements justify doing so.

---

## How governance evolves

The governance model should become more distributed as the project develops.

### Adding maintainers

New maintainers are selected based on sustained contribution, technical judgment, reliability, communication, and demonstrated alignment with Canary's documented engineering principles.

There is intentionally no fixed numerical threshold such as a required number of commits, pull requests, or months of participation.

Those measures can provide evidence, but they are not substitutes for judgment.

Maintainer appointments are made by the existing architect or maintainer group.

### Delegating subsystem ownership

As additional maintainers join, authority should be delegated where practical.

[`CODEOWNERS`](.github/CODEOWNERS) is the repository-level mechanism for expressing this ownership.

A maintainer responsible for rendering, for example, may have approval authority over rendering changes without automatically having authority over unrelated areas such as networking or project-state architecture.

Delegation should follow demonstrated expertise and responsibility rather than repository-wide privilege by default.

### Moving beyond one architect

Once multiple maintainers have demonstrated sustained responsibility for the project, architectural decisions should move toward a collaborative model.

The intended direction is:

1. Relevant maintainers discuss the proposal.
2. The team seeks consensus.
3. The decision and reasoning are recorded in an ADR.
4. If consensus cannot be reached, the designated architectural authority resolves the deadlock.
5. The governance document is amended when the new decision process becomes established practice.

The exact mechanism should be formalized when multiple maintainers actually exist rather than being invented prematurely for a team that does not yet exist.

---

## Architectural authority and disagreement

Technical disagreement is expected.

Disagreement should be resolved through evidence, explicit trade-offs, experimentation where practical, and documented decisions rather than through personal authority alone.

When a proposal conflicts with an existing ADR, the preferred process is to challenge or supersede the ADR explicitly rather than silently implementing a different architecture.

The project's goal is not to eliminate disagreement. It is to make disagreement **legible and reversible where possible**.

A contributor should be able to understand:

> what was decided, why it was decided, what alternatives were considered, and what evidence could justify changing it later.

---

## Succession and continuity

A one-maintainer project has an unavoidable bus-factor risk.

Canary therefore treats continuity as a governance concern rather than assuming that the current maintainer will always be available.

### If the current maintainer becomes unavailable

At the present stage, there is no fully independent successor authority.

That means the honest current answer is that the project would depend on trusted contributors stepping forward and establishing a new maintainer structure.

This is a known risk, not an acceptable permanent state.

As the project develops, the maintainer team should establish enough continuity that the loss of a single person does not halt the project.

### Repository and service access

When multiple trusted maintainers exist, critical project infrastructure should not depend exclusively on one personal account.

This includes, where applicable:

* GitHub organization and repository administration
* Package registries such as crates.io
* Domains and project websites
* CI infrastructure
* Release credentials
* Future marketplace or service accounts

Access should be structured so that legitimate succession does not require recovering one person's personal account.

### Governance continuity

This document should be reviewed whenever the project's actual governance changes.

A governance document that describes an organization that no longer exists is worse than an incomplete one because it creates false expectations.

---

## ADRs and governance are different

The [ADR system](docs/decisions/architecture-decision-records/) and this document serve different purposes.

**ADRs answer:**

> What technical decision did we make, and why?

**Governance answers:**

> Who has the authority to make that decision, and how does that authority change?

They should remain separate.

A technically sound decision still needs legitimate authority behind it, and a well-governed project still needs its technical reasoning recorded.

Together, they make Canary's architecture understandable both **technically and institutionally**.

---

## Project principles

Certain project principles may be treated as stronger constraints than ordinary implementation decisions.

These currently include the project's stated commitment to:

* Open-source distribution under the MIT License
* Explicit architectural boundaries
* Documented significant architectural decisions
* Long-term project maintainability
* Transparent contribution and governance practices

Changes to principles that materially redefine Canary should be treated as project-level decisions rather than ordinary implementation choices.

Where such principles are documented elsewhere, the relevant vision or policy document remains the authoritative source.

---

## Trademark and project identity

The MIT License governs the project's code. It does not by itself establish a trademark policy for the **Canary Engine** name, logo, or other project identity.

The project does not currently maintain a comprehensive trademark policy.

This is a known governance item rather than an unresolved implementation detail. A policy should be established before project adoption becomes large enough for naming disputes to become difficult or disruptive.

Until then, contributors should not assume that the right to use the code automatically grants unrestricted rights to use Canary's branding.

Related risks are tracked in [`docs/reviews/risk-register.md`](docs/reviews/risk-register.md).

---

## Marketplace and monetization policy

Canary does not currently have a finalized policy governing commercial marketplace content.

This includes questions such as:

* Whether marketplace plugins may be sold
* What restrictions may apply to marketplace content
* Whether Canary itself takes a fee
* What branding or compatibility requirements may exist

These questions are intentionally left open rather than being decided by accident through an early implementation.

A formal policy should be established before the marketplace reaches implementation. See [`docs/vision/long-term-roadmap.md`](docs/vision/long-term-roadmap.md) for the current roadmap context.

---

## Future organizational structure

Canary does not currently operate as a foundation or other formal nonprofit organization.

If the project grows to the point where informal maintainer governance is no longer sufficient, a formal legal or organizational structure may become appropriate.

Possible triggers include:

* A large maintainer or contributor community
* Significant external funding
* The need for formal institutional continuity
* Complex ownership or trademark requirements
* Governance disputes that cannot be handled by the existing maintainer model

A foundation or similar structure is therefore a **possible future evolution**, not a present commitment.

The objective is to introduce that complexity only when the project's scale makes it useful.

---

## Governance changes

This document should evolve with the project.

Changes to governance that materially alter:

* Who can make decisions
* How maintainers are appointed
* How architectural disputes are resolved
* How repository authority is delegated
* How succession is handled

should be proposed and reviewed explicitly rather than being introduced incidentally through unrelated repository changes.

Minor editorial or administrative corrections may be made through the normal documentation workflow.

---

## The governing principle

Canary's governance is built around one idea:

> **Authority should be explicit, architectural decisions should be traceable, and no project's long-term future should depend on a single undocumented person's judgment or continued availability.**

The project is intentionally small today.

The purpose of this document is to make sure its governance can become larger without becoming accidental.
