---
name: Feature request
about: Propose a new capability, subsystem, or change in direction
title: "[feature] "
labels: enhancement
---

**What problem does this solve?**
Describe the problem, not just the solution. What can't you do today?

**Proposed approach**
Your idea for how to solve it. Doesn't need to be fully worked out.

**Does this touch an existing architectural decision?**
Check `docs/decisions/architecture-decision-records/`. If this would
contradict or supersede an existing ADR, say which one — the discussion will
likely need a new ADR before code lands (see CONTRIBUTING.md).

**Could this be a plugin instead of a core engine change?**
Canary is modding/plugin-first by design (see
`docs/architecture/plugin-system.md`). If this can live as an out-of-tree
plugin rather than a core change, that's usually the faster path to landing
it *and* a good stress-test of the plugin API itself.

**Alternatives considered**
Anything else you thought about and why you didn't propose it instead.
