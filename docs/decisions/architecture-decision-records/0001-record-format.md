# 0001. Use Architecture Decision Records, in this format

**Status:** Accepted

## Context

Canary is designed to be a multi-year project with a changing set of
contributors. The most common way a long-lived codebase loses coherence
isn't a single bad decision — it's good decisions whose reasoning was never
written down, so a later contributor can't tell whether a constraint is load-
bearing or accidental, and either breaks it by mistake or is too cautious to
touch it at all.

## Decision

Significant, hard-to-reverse decisions get a numbered Architecture Decision
Record (ADR) in this directory, using this format (adapted from the format
popularized by Michael Nygard):

```
# NNNN. Short, descriptive title (a decision, not a topic)

**Status:** Proposed | Accepted | Superseded by NNNN

## Context
What problem forced this decision? What constraints applied?

## Decision
What was decided, stated plainly.

## Alternatives considered
What else was on the table, and why it was rejected. This section is not
optional — a decision record without rejected alternatives is just an
announcement, and doesn't help a future reader understand whether their new
information should change the outcome.

## Consequences
What this makes easier, what it makes harder, and what it forecloses.
```

**What warrants an ADR:** a new dependency in the trusted core, a change to
a public plugin/ABI interface, a choice between architecturally distinct
approaches (not "which crate implements approach X" once approach X is
settled), anything that would be expensive to reverse later. **What doesn't:**
routine implementation details, naming, or anything easily changed in a
follow-up PR. When in doubt, err toward writing the ADR — see
[`CONTRIBUTING.md`](../../../CONTRIBUTING.md#architecture-decision-records-adrs).

ADRs are numbered sequentially and are not renumbered or deleted when
superseded — a superseded ADR is marked `Superseded by NNNN` and left in
place, because the historical reasoning (including *why* it seemed right at
the time) is exactly the information a future re-litigation of the same
question needs.

## Alternatives considered

- **No formal record, rely on commit messages / PR descriptions.** Rejected:
  these are hard to discover later (which commit explained *why* we picked
  Rust?) and aren't a canonical, browsable "here's every big decision and its
  status" list.
- **A single running `DECISIONS.md`.** Rejected: doesn't scale past a
  handful of decisions, and makes diffs/history noisy (every new decision is
  a diff against a huge shared file).
- **RFC-style proposals requiring community vote before any decision is
  "Accepted."** Rejected for this project's current stage: Canary's founding
  brief explicitly calls for an owned technical direction rather than
  design-by-committee at this early stage; ADRs here record decisions made
  by whoever currently holds architectural responsibility, with dissent and
  alternatives welcome via the normal PR/issue process. This can change later
  if project governance evolves — that would itself be a good candidate for
  a future ADR.

## Consequences

- Every non-trivial architectural claim elsewhere in `docs/` should be
  traceable to an ADR if it's meant to be binding, and treated as
  aspirational/discussion otherwise. Documents that describe target
  architecture (most of `docs/architecture/`) link to the relevant ADR;
  where no ADR exists yet, treat the description as provisional.
- This adds a small amount of process overhead to genuinely big decisions.
  That's the point — it's supposed to feel like slightly more friction than
  making the decision silently in code.
