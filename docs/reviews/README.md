# Architecture Reviews

This directory holds periodic, dated senior-architecture reviews of the
project — distinct from [`docs/architecture/`](../architecture/) (which
describes current target design) and
[`docs/decisions/architecture-decision-records/`](../decisions/architecture-decision-records/)
(which records individual decisions). A review is a point-in-time
critical audit: what's weak, what's undocumented-but-load-bearing, what
will get expensive if it isn't addressed before more is built on top of it.

## Why this exists as its own category

A project that intends to survive 10+ years and thousands of contributors
needs a standing habit of stepping back and stress-testing its own
foundations — not just moving forward on the next feature. Folding that
critique into `docs/architecture/` would blur "this is the target design"
with "this is a critique of the target design," and folding it into
individual ADRs loses the connective tissue between findings. This
directory is where that critique lives, on purpose, dated, and kept even
after its findings are addressed (so the reasoning behind a later fix
remains visible).

## Index

| Date | Review | Scope |
|---|---|---|
| 2026-08 | [`2026-08-senior-architecture-review.md`](2026-08-senior-architecture-review.md) | Repository structure, Rust architecture, plugin ABI, ECS strategy, language independence, scripting, marketplace architecture, build system, versioning — conducted at the end of the `v0.0.1-pre1` foundation session, before further v0.0.1 development continued. |

See also [`risk-register.md`](risk-register.md), a living (not point-in-time)
list of tracked risks that reviews feed into and that should be updated
as risks are mitigated or new ones are found — it isn't rewritten per
review the way this index's dated entries are.

## Expectation for future reviews

A future review should be added as a new dated file here (never edited
into an old one — same append-only spirit as the ADR log), and should
explicitly note which prior findings were addressed, which were
deliberately deferred (and why), and which are still open.
