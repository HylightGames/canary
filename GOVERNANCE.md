# Governance

This document exists because of a gap identified in
[`docs/reviews/2026-08-senior-architecture-review.md`](docs/reviews/2026-08-senior-architecture-review.md)
(Finding 1.1): a project explicitly aiming to survive 10+ years and
thousands of contributors had, until now, no answer to "what happens when
the person currently making architectural decisions is unavailable,"
"how does a second maintainer get added," or "who decides contested
questions." Technical debt is recoverable by definition — it's debt,
something can be paid down. A governance vacuum tends not to be: the
projects that fail this way usually fail by fragmenting, stalling, or
forking acrimoniously, not by a bug report. This document is Canary's
first attempt at closing that gap, written honestly for a project that
currently has exactly one maintainer, not padded out to sound like it
has more structure than it does.

## Current state (as of this writing)

**One active maintainer/architect.** No formal maintainer team exists
yet. This is itself the single largest governance risk this project
carries — see the succession section below, which exists specifically
because this state is a risk, not a stable end point.

## Roles

- **Architect** — makes and records binding architectural decisions (see
  [ADR process](docs/decisions/architecture-decision-records/0001-record-format.md)).
  Currently one person. Intended to become a small group as trusted
  maintainers are added, not to remain a single-person role
  indefinitely — a project this ambitious that never grows past one
  architect has, in practice, already answered "does this survive its
  founder" in the negative.
- **Maintainer** — has merge rights to some or all of the repository;
  reviews and approves PRs in their area; may hold architect
  responsibilities for specific subsystems (see "path-based ownership"
  below and [`.github/CODEOWNERS`](.github/CODEOWNERS)).
- **Contributor** — anyone who opens an issue, PR, or discussion. No
  special status required; see [`CONTRIBUTING.md`](CONTRIBUTING.md).

## How decisions get made today

For genuinely architectural, hard-to-reverse decisions: the architect
decides, records the decision and its rejected alternatives as an ADR
(see [`0001-record-format.md`](docs/decisions/architecture-decision-records/0001-record-format.md)),
and remains open to a new ADR revisiting it later if new information
emerges. This is deliberately not a design-by-committee or formal-vote
process at this stage — the founding brief for this project explicitly
called for owned technical direction rather than a menu of options, and
with one maintainer, a voting process would be theater, not a real
mechanism. **This is expected to change** as more maintainers join; see
below.

For everything else (bug fixes, non-architectural features, docs): normal
PR review, per [`CONTRIBUTING.md`](CONTRIBUTING.md).

## How this changes as the project grows

This section states intent, not a fully worked-out constitution — the
project isn't at a size where the latter would be anything but
speculative. The intent:

1. **Adding a maintainer** is a decision the current architect(s) make,
   based on sustained, trusted contribution — there's no fixed
   quantitative bar (a number of PRs, a tenure) because that kind of bar
   is easy to game and a poor proxy for the actual thing that matters
   (judgment, alignment with the project's stated design philosophy in
   [`docs/vision/design-philosophy.md`](docs/vision/design-philosophy.md)).
2. **As more than one maintainer exists**, architectural decisions move
   from "the architect decides" toward "maintainers reach consensus;
   the architect (or a designated lead, if the role has been formally
   split) breaks ties." The exact mechanism should be written down as an
   amendment to this document *when* it's needed, not spec'd
   hypothetically now.
3. **Path-based ownership** ([`.github/CODEOWNERS`](.github/CODEOWNERS))
   is how subsystem-level authority is expected to be delegated as the
   project grows — a maintainer trusted with the rendering subsystem
   doesn't need architect-level authority over networking, and vice
   versa. The current `CODEOWNERS` file names one person for everything,
   honestly reflecting that no delegation has happened yet; update it as
   real delegation happens, rather than leaving it stale.
4. If the project reaches a scale where a lightweight, informal process
   like the above stops working — real disputes that don't resolve by
   consensus, meaningful outside funding, contributors wanting formal
   standing — the appropriate move is very likely a nonprofit foundation
   model, the way Godot (via the Godot Foundation) and several other
   long-lived open-source game engines have done. This is **not a
   commitment being made now** — standing up a foundation prematurely is
   its own kind of overhead — but it's the anticipated direction if/when
   informal governance stops scaling, named here so it doesn't need to be
   invented from scratch under pressure later.

## Succession and bus factor

This is the part of this document that matters most while the project
has one maintainer, and the part most projects skip until it's too late
to matter.

- **If the current architect becomes unavailable** (temporarily or
  permanently) with no successor designated: this is, right now, an
  unresolved single point of failure. The honest, current answer is "the
  project would need someone in the existing contributor base to step
  up and be recognized by the community as a new maintainer," which is a
  real risk, not a plan. Closing this gap — naming at least one
  contributor who could take over repository, crates.io, and any future
  organizational-account access if needed — is the most important open
  governance action item this project has, more important than any
  purely technical finding in the architecture review this document
  grew out of.
- **Account/access continuity**: the GitHub repository, crates.io crate
  ownership, and any future domain/organization accounts should not be
  tied to a single personal account with no documented recovery/
  succession path once a second trusted maintainer exists. This is an
  administrative task, not a design decision, and should not wait for a
  crisis to be addressed.
- **This document itself** should be one of the first things a new
  maintainer reads and is asked to help keep current — a governance
  document that isn't updated as the project's actual structure changes
  is worse than none, because it actively misleads.

## Trademark and brand

The MIT license covers code; it grants no trademark rights in the
"Canary Engine" name or any future logo. No trademark policy exists yet
(can a fork call itself "Canary Engine"? can a commercial product say
"Powered by Canary" without asking?). This is an open question, not a
settled one, and should be resolved — likely with a short, permissive
trademark policy modeled on how other open-source projects with a name
worth protecting handle it — before the project has enough adoption that
resolving it becomes contentious rather than administrative. See also
[`docs/reviews/risk-register.md`](docs/reviews/risk-register.md) R-07 and
R-10 for the related crates.io-naming and brand-connotation findings.

## Monetization and the future marketplace

No position exists yet on whether marketplace plugins may be sold, and
if so under what terms (see
[`docs/reviews/2026-08-senior-architecture-review.md`](docs/reviews/2026-08-senior-architecture-review.md),
Finding 7.2). This is explicitly a governance question, to be decided by
whatever the maintainer structure looks like when the marketplace (Era 6,
[`docs/vision/long-term-roadmap.md`](docs/vision/long-term-roadmap.md))
actually approaches implementation — named here as a known open question
so the first person who asks isn't the one who accidentally decides it
by precedent.

## Relationship to the ADR process

ADRs ([`docs/decisions/architecture-decision-records/`](docs/decisions/architecture-decision-records/))
record *what* was decided technically and *why*. This document records
*who* gets to decide, and how that authority is expected to change as the
project grows. They're deliberately separate: a technical decision can be
excellent regardless of how it was arrived at, but a project this
ambitious needs both to be legible, not just the technical half.
