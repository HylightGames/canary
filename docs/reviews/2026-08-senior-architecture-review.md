# Senior Architecture Review — August 2026

**Scope:** repository structure, Rust architecture, plugin ABI, ECS
strategy, language independence strategy, scripting strategy, future
marketplace architecture, build system, versioning.

**Conducted:** at the close of the `v0.0.1-pre1` foundation session,
before further v0.0.1 development continues, at the founding architect's
own request for a critical self-audit.

**Framing:** this review assumes the project's stated ambition literally —
10+ years, thousands of contributors — and asks, for each area, "what
here is cheap to fix today and would be expensive to fix after that
ambition starts coming true?" A decision that's merely imperfect but
easy to change later is not flagged as a finding; a decision that's
merely inconvenient to reverse doesn't automatically outrank one that's
comfortable today but corrupts data or breaks compatibility silently
after years of production use. Severity ratings reflect that framing,
not raw impact.

**How to read this document:** each area has a short assessment followed
by numbered findings. Each finding states the weakness, *why it compounds
with scale/time* (not just "why it's bad"), and a recommendation. Findings
that produced a new or amended ADR link to it; all findings are also
tracked in [`risk-register.md`](risk-register.md) for ongoing status.

---

## Cross-cutting theme: the ADR process wasn't fully followed building the foundation it created

Before the itemized findings: the single most important pattern this
review surfaced isn't in any one subsystem. [ADR 0001](../decisions/architecture-decision-records/0001-record-format.md)
states that any hard-to-reverse, load-bearing decision gets a written
record. Several decisions below (crate versioning strategy, the absence
of `Send + Sync` bounds on ECS component storage, the absence of an ABI
version field on the plugin vtable) were made silently — in `Cargo.toml`,
in a trait bound left off, in a struct definition — during the same
session that wrote the ADR policy requiring they be written down. This
isn't a moral failing so much as a predictable one: producing a large
amount of code and documentation in one sitting makes it easy for a
process the docs themselves mandate to quietly not apply to the docs'
own author. The fix isn't more process for its own sake; it's specifically
naming this failure mode so it's checked for in future reviews, not just
this one. See Finding B-1 in the versioning section and Findings C-1 and
D-1 below — all three are, at root, the same failure appearing in
different subsystems.

---

## 1. Repository structure

**Assessment:** solid for a single-architect, pre-code foundation. Several
choices that are invisible at this scale (one contributor, six crates)
become load-bearing the moment either number grows by 10x.

### Finding 1.1 — No governance or succession plan exists [Critical]

The entire decision-making model documented so far is "ADRs record
decisions made by whoever currently holds architectural responsibility"
([`CONTRIBUTING.md`](../../CONTRIBUTING.md#architecture-decision-records-adrs)).
There is exactly one person who currently holds that responsibility, and
nothing describes what happens if they disappear, what happens when a
second maintainer is added, or how disputes between maintainers resolve.
For a 10-year project, this is the single highest-value gap this review
found — bus-factor and governance failures, not technical debt, are what
actually kill or fork long-lived open-source projects. **Addressed in
this review by adding [`GOVERNANCE.md`](../../GOVERNANCE.md)** — see that
document for the substance; flagged here because it belongs in the
findings list, not just as a new file that appeared.

### Finding 1.2 — No contribution-provenance policy (DCO/CLA) [High]

[`CONTRIBUTING.md`](../../CONTRIBUTING.md#licensing) currently only says
contributions are MIT-licensed by virtue of being submitted. For a
project expecting thousands of contributors over a decade, having no
lightweight mechanism (a Developer Certificate of Origin sign-off is the
standard, low-friction choice — no CLA-holding entity required) to
establish contribution provenance is a gap that's nearly free to close
now and effectively impossible to close retroactively across thousands
of historical commits later, if an IP dispute or a corporate contributor's
legal team ever asks "can we prove every line was properly licensed to
contribute." **Recommendation:** adopt a DCO requirement now (see
"Recommended changes" below).

### Finding 1.3 — Crate names are already colliding on crates.io [High]

Verified directly: the bare crate name `canary` is already registered on
crates.io (an unrelated distributed-systems/networking library, last
published 2022), and `canary-rs` is also taken (an unrelated speech
recognition binding). This means a natural future meta-crate — the "add
one dependency to get the whole engine" crate many users would expect,
the way `bevy = "0.19"` works for Bevy — **cannot be published as plain
`canary`**. This wasn't checked before crate names were chosen, and
crates.io names are first-come-first-served with no reservation system:
every day this isn't checked and reserved is a day someone else could
take a name this project will want. This review could not confirm
whether `canary-engine` specifically is free (search evidence was
suggestive but not conclusive either way) — that needs a direct,
authoritative check, not an inference from search results.

### Finding 1.4 — "Canary" carries strong existing connotations as a build-channel name, not a proper noun [Medium]

Beyond crates.io specifically: "Canary" is heavily used across the software
industry to mean "the nightly/experimental release channel" (Chrome
Canary, Xenia Canary being the most prominent examples this review's
search surfaced). This creates an ongoing discoverability and brand-
confusion cost distinct from trademark conflict — search results for
"canary engine" will structurally compete with unrelated "canary build"
content indefinitely, and newcomers may reflexively parse "Canary Engine"
as "the canary channel of some other engine" rather than as a proper
noun. This doesn't necessarily justify a rename (renaming after any real
adoption is its own expensive mistake), but it should be a *known,
named* tradeoff rather than a surprise later, and argues for the project
being unusually consistent about capitalizing/branding "Canary Engine"
as a compound proper noun in all official material.

### Finding 1.5 — No path-based ownership or review-routing exists yet [Medium]

There's no `CODEOWNERS` file. With one contributor this is invisible;
with hundreds of open PRs across dozens of subsystems, "who is supposed
to review a change to the plugin ABI" needs an answer that doesn't
depend on someone remembering. **Addressed in this review** with a
minimal, honest `CODEOWNERS` (see "Recommended changes") — deliberately
not fabricating a team structure that doesn't exist yet.

### Finding 1.6 — Flat `engine/*` crate layout has no stated scaling plan [Low]

Six crates directly under `engine/` is fine. A mature engine plausibly
has dozens (multiple render backends, multiple physics backends, one
crate per asset-format importer, editor-panel crates, ...). Nothing
currently states whether `engine/` stays flat, grows subdirectories
(`engine/render/`, `engine/physics/`), or splits into a workspace-of-
workspaces. This is genuinely low-severity today — it's cheap to
reorganize a handful of crates, and expensive but not urgent to
reorganize hundreds — but it should be a named, anticipated decision
point in the roadmap rather than something that gets decided implicitly
by whichever crate happens to get added 30th.

---

## 2. Rust architecture

### Finding 2.1 — CI's global `RUSTFLAGS: "-D warnings"` is a latent, self-inflicted CI-fragility risk [Critical]

[`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) sets
`RUSTFLAGS: "-D warnings"` as a job-wide environment variable. This is a
well-known Rust ecosystem footgun: `RUSTFLAGS` applies to *every* `rustc`
invocation Cargo makes, including for third-party dependencies — not just
this project's own crates. The workspace `[lints]` table already declares
`missing_docs` and `clippy::all` at `warn` *scoped to workspace members*,
which is the correct mechanism. Layering a blanket `RUSTFLAGS` denial on
top means CI can fail for a reason entirely outside this project's
control: a new compiler release adds a lint that fires inside a
dependency's own code (code this project doesn't own and can't fix), or
a dependency itself has a pre-existing warning under a newer compiler.
The failure mode is exactly the kind that's cheap to prevent now and
expensive later — "expensive" here meaning *recurring, confusing CI
breakage disconnected from any change the project actually made*, at
exactly the point where the project has enough external dependencies and
contributors for it to happen regularly and for the actual cause to be
non-obvious to whoever hits it. **Recommendation:** remove the blanket
`RUSTFLAGS` and rely on `cargo clippy --workspace --all-targets -- -D
warnings` (which is correctly scoped to workspace members only) plus the
workspace `[lints]` table for `cargo build`/`cargo test`.

### Finding 2.2 — `canary-ecs`'s component storage has no `Send + Sync` bound, but the target design requires it [High]

`World::insert<T: 'static>` (and the storage backing it,
`HashMap<TypeId, HashMap<u32, Box<dyn Any>>>`) has no `Send + Sync` bound
on `T`, and `Box<dyn Any>` itself is neither `Send` nor `Sync`. This means
the *current* `World` cannot be moved to or shared across threads at all
— which is invisible today (v0.0.1-pre1 is single-threaded by design) but
directly contradicts the target design in
[`core-runtime.md`](../architecture/core-runtime.md#threading--the-job-system):
a work-stealing job scheduler fundamentally needs component storage (and
therefore component types) to be `Send + Sync`. Adding that bound now,
before any component types exist in the wild outside this workspace,
costs nothing. Adding it after plugins and downstream code have defined
component types that *aren't* `Send + Sync` (interior-mutable types
using `Rc`/`Cell` without thought, for instance) means a breaking change
that surfaces as a wall of compiler errors in code this project doesn't
control. **This is a small, mechanical fix, not "major implementation
code"** — it's listed here as a finding and recommendation, not
implemented in this review, per this review's own scope constraint; see
"Recommended changes before v0.0.1 development continues" below.

### Finding 2.3 — Error types crossing the plugin boundary are strings-shaped, not code-shaped [Medium]

`SubsystemError` (`Box<dyn std::error::Error + Send + Sync>`) and the
`thiserror`-derived error enums are good, idiomatic Rust for *this
project's own* code calling *its own* code. They're a weaker fit for
errors that need to be introspected by tooling this project doesn't
control — future marketplace review automation, cross-language plugin
hosts, telemetry — because the only stable-looking thing about a
`Display`-formatted error is its text, and text is not a contract anyone
signed up to keep stable. Once external tooling starts pattern-matching
on error *messages* (which it will, informally, the first time someone
needs to distinguish failure causes programmatically and no better
option exists), the message text becomes a de facto frozen API nobody
agreed to freeze. **Recommendation:** before the plugin/marketplace
surface grows further, define a small set of stable, explicit error
*codes* (not just enum variants — actual stable identifiers, e.g. a
`u32` or short string tag) for anything a plugin author, marketplace
tool, or cross-language host might need to branch on, independent of the
human-readable message.

### Finding 2.4 — `missing_docs` is applied uniformly to binary crates [Low]

Minor: the workspace `[lints]` table applies `missing_docs = "warn"` to
every crate including `canary-runtime` and `xtask` (both binaries with no
external API surface to document). Harmless today; worth scoping the
lint to library crates specifically once it becomes noise rather than
signal.

### Finding 2.5 — `rustfmt.toml` specified nightly-only options while the project targets `stable`, and already-committed code failed its own format check [High]

Caught by actually running `cargo fmt --all -- --check` during this
review, not by reading the config: `rustfmt.toml` set
`imports_granularity` and `group_imports`, both of which remain
nightly-only rustfmt options — rustfmt silently warned and did not apply
them, on the exact `stable`-channel toolchain `rust-toolchain.toml`
commits this project to. Separately, and more concretely, the already-
committed code in `engine/canary-core/src/app.rs` and
`tools/xtask/src/main.rs` did not itself pass `cargo fmt --all --
--check` — meaning the very first thing a new contributor is told to run
([`CONTRIBUTING.md`](../../CONTRIBUTING.md#development-workflow)) would
have failed on a clean checkout, before they'd changed a single line.
This is a small thing individually, but it's a credibility problem for a
project whose entire pitch is professional-grade rigor from the first
commit, and it's exactly the kind of gap that only shows up by running
the tools rather than reading about them. **Fixed directly in this
review**: the two nightly-only options were removed from `rustfmt.toml`,
and `cargo fmt --all` was run across the workspace (a mechanical,
behavior-preserving change — not "implementation code" by any reasonable
reading, and verified via `cargo build --workspace` and `cargo test
--workspace` afterward to confirm nothing broke).

---

## 3. Plugin ABI (Tier B, native)

### Finding 3.1 — The Tier B vtable has no version field and no extension mechanism [Critical]

This is, in this reviewer's judgment, the single most consequential
technical finding in this review. `PluginHandle`/`PluginVTable`
(`engine/canary-plugin-api/src/abi.rs`) is a fixed-size `#[repr(C)]`
struct with exactly four function pointers and no version tag anywhere
in it. Two concrete, foreseeable needs already named elsewhere in this
project's own docs — state serialization for hot reload
([`scripting-system.md`](../architecture/scripting-system.md#hot-reload))
and capability introspection
([`plugin-system.md`](../architecture/plugin-system.md#the-plugin-trait-surface-canary-plugin-api))
— would require adding fields to this struct. Adding a field changes the
struct's size and layout. A plugin compiled against the current layout,
loaded by a host built against a hypothetical future layout (or vice
versa), doesn't get a compile error or even a clean runtime error: it
gets undefined behavior, most likely a crash or memory corruption, deep
inside FFI code, in a plugin this project may not control the source of.
This is exactly backwards from how a stable ABI meant to last a decade
should fail: it should fail loudly and immediately (a version mismatch
the loader detects and rejects with a clear error), not silently and
eventually. The cost of fixing this is currently as close to zero as it
will ever be — no plugin exists in the wild yet depending on the current
layout. **This finding produced [ADR 0009](../decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md).**

### Finding 3.2 — No plugin manifest format exists, and the marketplace depends on one existing eventually [High]

The `Plugin` trait currently exposes only `name()`. There is no metadata
format (author, version, engine-compatibility range, declared
capabilities as data rather than as a Rust-only enum) that external
tooling — a future marketplace indexer, a dependency resolver, a security
reviewer — could read without executing the plugin. Building that
tooling later, without a manifest format decided now, means either
retrofitting a manifest onto plugins that already exist without one, or
building marketplace tooling that has to load and execute a plugin just
to learn its name and version. **Recommendation:** define a minimal,
versioned manifest schema (even a simple TOML/JSON sidecar file) as a
documentation task before Era 6 work starts, so plugin authors and
marketplace tooling have a stable target from day one of that era rather
than a moving one.

### Finding 3.3 — "Trusted" for Tier B has no verification mechanism [High]

[`plugin-system.md`](../architecture/plugin-system.md#tier-b--trusted-native-c-abi)
correctly identifies Tier B as trusted-by-design with no sandboxing, but
"trusted" currently means only "a human decided to load it" — there's no
signing, checksum registry, or provenance mechanism described anywhere.
That's a reasonable state for a solo-architect foundation; it's a real
supply-chain risk the moment Tier B plugins are distributed to anyone
beyond the person who compiled them. This doesn't need solving now, but
it should be named as an open requirement for "Tier B distribution" in
[`future-roadmap.md`](../roadmap/future-roadmap.md) rather than silently
assumed to be someone else's future problem.

### Finding 3.4 — No engine/plugin compatibility declaration mechanism [Medium]

Nothing currently lets a plugin declare "I require canary >= 0.0.1, <
0.1.0." Combined with ADR 0006's versioning scheme (pre-1.0, anything can
break), this means a plugin ecosystem — even an internal one, before any
public marketplace — has no way to detect a mismatched plugin/engine pair
except by crashing at load time or misbehaving silently. Worth deciding
before Tier A plugins exist in any number, since retrofitting version
gating after plugins that don't declare compatibility already exist is
much messier than requiring it from the first plugin onward.

---

## 4. ECS strategy

### Finding 4.1 — Component identity (`TypeId`) does not survive the language boundary [Critical]

This is the second most consequential technical finding in this review,
and it's specific to the ECS in a way Finding 3.1 is specific to the
native ABI. `canary-ecs` identifies component types using
`std::any::TypeId`, which is a Rust-compiler-internal concept: it isn't
guaranteed stable even across two separate Rust compilations of
"the same" type, and it has no meaning at all to a Tier A plugin written
in C, Zig, or any other Component Model language — such a plugin
participates in a completely different type system (WIT's), one that
never touches Rust's `TypeId` space. [`plugin-system.md`](../architecture/plugin-system.md)
and [`scripting-system.md`](../architecture/scripting-system.md) both
commit to Tier A plugins declaring which components they read/write —
but as currently architected, there is no mechanism by which a
non-Rust plugin could refer to a component type at all, let alone have
that reference checked against a capability grant. If this isn't
resolved before the archetype ECS migration and the Tier A loader both
land, the likely failure mode is a rushed, poorly-designed translation
layer invented under implementation pressure once several other systems
already assume `TypeId` is *the* identity mechanism. **This finding
produced [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md)**,
marked `Proposed` rather than `Accepted` — this is a real unsolved design
problem, not a mechanical fix, and deserves prototyping alongside the
archetype migration rather than a desk decision now.

### Finding 4.2 — Generation counters can wrap, and wraparound is currently silent [Medium]

`Entity`'s generation field is a `u32`, bumped with `wrapping_add` on
every despawn. For a single dev session or even a normal game's runtime,
this is a non-issue. For a genuinely 10-year-scoped ambition that
includes "strong multiplayer foundations" — implying long-running,
persistent server processes — a single hot, frequently-recycled entity
slot (a bullet/projectile spawn point, say, recycled millions of times a
day for years) could plausibly wrap a 32-bit generation counter over a
long enough uptime, at which point two genuinely different entities
could, in principle, collide on the same (index, generation) pair and be
treated as the same entity — silent data corruption, not a crash, which
is the worst kind of bug to eventually debug. This is a low-*probability*
but high-*cost-if-it-happens* and currently *silent* risk. **Recommendation:**
either widen the generation counter (e.g. 64-bit entity IDs), or at
minimum retire a slot permanently after wraparound instead of recycling
it, and either way make the choice a deliberate, documented one rather
than an accident of `wrapping_add` being the easiest thing to type.

### Finding 4.3 — Change detection is named as a requirement in three places but designed in none [Medium]

[`core-runtime.md`](../architecture/core-runtime.md#ecs-architecture),
[`networking.md`](../architecture/networking.md#replication-is-an-ecs-concept-not-a-side-channel),
and this project's own hot-reload story all depend on "change detection
query filters" existing — but nowhere is there an actual design for *how*
change is tracked (a per-component dirty bit? a global tick counter
compared per-component, à la Bevy? an observer/event model?). This
matters because change detection tends to be threaded deeply through
storage layout (which is exactly what the archetype migration is about
to redesign) — under-specifying it now risks the archetype migration
locking in a storage layout that makes the *later* change-detection
design more awkward than it needed to be. **Recommendation:** design
change detection as part of the archetype migration itself, not as a
follow-up bolted on after — it should be a first-class input to that
migration's design, not an afterthought to it.

### Finding 4.4 — No stated position on entity hierarchy/relationships [Low]

Parent/child transform hierarchies are one of the more infamous hard
problems in ECS design (multiple engines, Bevy included, have gone
through public redesigns of exactly this). Having no current position is
fine at this stage — but it's worth naming explicitly as a known
open question for the archetype-migration era, rather than letting the
first crate that needs a hierarchy (probably rendering or physics) invent
one under its own local pressure.

### Finding 4.5 — System ordering beyond data-conflict detection is unaddressed [Low]

[`core-runtime.md`](../architecture/core-runtime.md#threading--the-job-system)
describes automatic parallelization from data-access declarations, which
handles *data* conflicts, but says nothing about *semantic* ordering
constraints that aren't data conflicts (e.g., "apply input before physics,
even though they don't touch the same components"). Most mature ECS
designs need an explicit ordering/labeling mechanism alongside automatic
conflict detection. Worth flagging as underspecified rather than assuming
data-conflict analysis alone is sufficient.

---

## 5. Language independence strategy

### Finding 5.1 — Overlaps with Finding 4.1 (component identity); see [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md).

### Finding 5.2 — WIT interface versioning strategy is undecided [Medium]

The Component Model has its own real, working story for interface
versioning (semver'd WIT packages). Nothing in this project's docs says
*how* Canary will use it — package naming scheme, what counts as a
breaking WIT change versus additive, how a Tier A plugin declares which
interface version it targets. This is safe to leave unresolved only
because Tier A isn't implemented yet; it should be resolved *before*
Tier A lands, not discovered by the first two plugins that need
mutually-incompatible interface versions.

### Finding 5.3 — Host/component call-boundary performance is asserted, not measured [Medium]

[`scripting-system.md`](../architecture/scripting-system.md#performance-expectations)
recommends batching calls across the host/component boundary as a
mitigation for per-call overhead, which is sound general guidance — but
it's guidance derived from how WASM/Component Model boundaries generally
behave, not from a benchmark of *this* engine's actual data-marshaling
pattern for, say, ten thousand entities' transforms crossing that
boundary once a frame. If that turns out too slow for real hot-path
gameplay scripting even with batching, it affects a foundational
strategic bet, not an implementation detail. **Recommendation:** prototype
and benchmark the host/component data-crossing pattern early in the Tier
A work — before API surface is designed around an assumed cost model —
rather than treating the performance story as settled.

### Finding 5.4 — "Language-agnostic" may be overpromised relative to today's realistic tooling [Low]

[`project-goals.md`](../vision/project-goals.md) and
[`README.md`](../../README.md) both list a broad set of languages as
Tier A targets. This project's own research
([`technology-evaluations.md`](../research/technology-evaluations.md))
is honest that Rust/C/C++/Zig/TinyGo are the *realistic* Component Model
targets as of mid-2026, and that Python/C#/JS tooling is much less
mature. The gap between the vision-level claim and the research-level
nuance is small today but is exactly the kind of thing that produces
disappointed users later ("the docs said any language" followed by "there's
no usable Python toolchain yet"). **Recommendation:** have the vision-level
docs point to the research doc's more precise, currently-true claim,
rather than only stating the aspirational version.

---

## 6. Scripting strategy

### Finding 6.1 — Hot reload's "serialization contract" has no actual design [Medium]

[`scripting-system.md`](../architecture/scripting-system.md#hot-reload)
describes swapping a script component and "re-hydrating its declared
state from a serialization contract the component defines," without
specifying what that contract looks like, or how state migrates across a
reload where the script's own state *shape* changed (added a field,
changed a type). This is a well-known hard problem (several engines'
hot-reload stories are notoriously fragile exactly here) and deserves a
real design before implementation starts, not a one-sentence description
treated as sufficient.

### Finding 6.2 — Unifying visual scripting under the WASM target may conflict with the "immediate feedback" UX principle [Medium]

[`scripting-system.md`](../architecture/scripting-system.md#designer-facing-ergonomics-vs-systems-programmer-ergonomics)
commits to visual scripting graphs compiling to the same WASM component
target as textual languages — architecturally elegant, one execution
model instead of two. But [`ux-principles.md`](../ui/ux-principles.md#feedback-should-be-immediate-wherever-technically-possible)
separately commits to near-immediate feedback as a core UX requirement.
If compiling a visual-graph edit to WASM takes even a second or two, that
directly violates the UX principle, for exactly the audience (visual
scripting users) least likely to tolerate it. Nothing currently reconciles
this tension — it's simply not mentioned. **Recommendation:** name this
tradeoff explicitly when visual scripting design starts, and consider
whether an interpreted fast-path for in-editor iteration (with WASM
compilation reserved for "bake"/ship) is needed, rather than assuming the
unified-target architecture and the immediate-feedback requirement will
turn out to be compatible by default.

---

## 7. Future marketplace architecture

Findings 3.2, 3.3, and 3.4 above are, at root, marketplace-architecture
findings (manifest format, trust/signing, compatibility declarations) —
cross-referenced rather than repeated. Additional marketplace-specific
findings:

### Finding 7.1 — No naming-collision policy for published plugins [Low]

What happens when two marketplace plugins both want to be called
`InventorySystem`? Not urgent (Era 6 is far off), but worth a placeholder
line in [`future-roadmap.md`](../roadmap/future-roadmap.md) so it's a
named open question rather than something the first marketplace
squabble decides by precedent.

### Finding 7.2 — No stated position on monetization [Low]

Free-only vs. paid plugins vs. revenue share is a governance question
more than a technical one, but having zero stated position risks a
community conflict when someone first asks to sell a plugin, with no
existing framework to point to. Belongs in `GOVERNANCE.md` as a
"to be decided, here's who decides it" note, not a technical ADR.

---

## 8. Build system

### Finding 8.1 — Same issue as Finding 2.1 (`RUSTFLAGS`); cross-referenced, not repeated.

### Finding 8.2 — `xtask check` silently diverges from what CI actually gates on [Medium]

`tools/xtask`'s `check` command deliberately skips `clippy` (documented
reason: the `clippy` component isn't guaranteed to be installed locally).
But CI *does* gate on clippy with `-D warnings`. This means a contributor
can run the local "pre-flight check" command, see it pass, push, and then
be surprised by a CI failure the local tooling structurally couldn't have
caught. At small scale this is a minor annoyance; at thousands-of-
contributors scale, a local check command that doesn't actually predict
CI outcomes is a recurring source of friction and lost time across many
people, not just one. **Recommendation:** have `xtask check` detect
whether `clippy` is available and run it when present, warning clearly
when it isn't, rather than unconditionally skipping it. (Not implemented
in this review — it's a small but real code change, out of this review's
"no major implementation code" scope; tracked as a recommended follow-up.)

### Finding 8.3 — No CI build-cache strategy is discussed [Low]

Not urgent at six crates and one contributor. Will matter for CI cost and
iteration speed at real scale (dozens of crates, many daily PRs). Worth a
forward-looking note in [`build-system.md`](../development/build-system.md)
now so it's an anticipated decision rather than a fire drill when CI
minutes or wait times become a visible problem.

---

## 9. Versioning

### Finding 9.1 — Workspace crate versioning (lockstep) was decided in `Cargo.toml`, never in an ADR [High]

Every workspace crate uses `version.workspace = true`, meaning all
`canary-*` crates share one version number, bumped together. This is a
reasonable, well-precedented choice (Bevy's own many crates do the same,
specifically so "which versions are compatible" never requires a
compatibility matrix) — but it was never written down as a decision, nor
were alternatives (independent per-crate SemVer; a hybrid) considered on
the record. This is the clearest instance of the cross-cutting theme
noted at the top of this review. **This finding produced
[ADR 0008](../decisions/architecture-decision-records/0008-workspace-crate-versioning-lockstep.md)**,
formalizing the existing choice rather than changing it.

### Finding 9.2 — Pre-1.0 change severity has no communication convention [Low]

[ADR 0006](../decisions/architecture-decision-records/0006-versioning-scheme.md)
correctly states that anything before `v0.0.1` (unqualified) can break
without notice. In practice, once anyone outside this project depends on
a `-preN` build — which will happen the moment external contributors or
early adopters exist, regardless of the formal stability promise — a
CHANGELOG entry that doesn't distinguish "cosmetic API rename" from
"we redesigned the ECS storage layer" imposes a real cost on everyone
tracking the project. **Recommendation:** adopt a lightweight severity
marker in CHANGELOG entries (e.g. a `[breaking]` tag) even though strict
SemVer doesn't formally apply yet — communicating churn severity is
useful long before it's contractually required.

---

## Recommended sequencing before v0.0.1 development continues

In priority order, factoring in both severity and how cheaply each item
can still be fixed today versus later:

1. **Read and adopt [`GOVERNANCE.md`](../../GOVERNANCE.md)** (Finding 1.1)
   — the highest-leverage item in this review, and the cheapest to act on
   immediately (it's a document, not a redesign).
2. **Remove the blanket `RUSTFLAGS: "-D warnings"` from CI** (Finding 2.1)
   — a config-only fix, essentially free, prevents a real recurring-
   fragility risk.
3. **Decide and implement the plugin ABI version field + extension
   mechanism** (Finding 3.1 / [ADR 0009](../decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md))
   — small code change, and its cost only rises from here as any plugin
   ecosystem starts to exist.
4. **Add `Send + Sync` bounds to ECS component storage** (Finding 2.2) —
   small code change, same "cost only rises" logic.
5. **Prototype the component-identity mechanism** ([ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md),
   Finding 4.1) alongside — not after — the archetype ECS migration, since
   the two designs need to fit together.
6. **Adopt a DCO requirement** (Finding 1.2) before contributor count
   grows past "trivial to ask retroactively."
7. **Check and reserve key crates.io names** (Finding 1.3) — time-
   sensitive in a way nothing else on this list is; another party could
   take a needed name at any time regardless of this project's own pace.
8. Everything else in this review is real but lower-urgency: safe to
   track in [`risk-register.md`](risk-register.md) and address as the
   relevant subsystem (change detection, WIT versioning, hot-reload
   contract, manifest format) actually starts being built, rather than
   solved speculatively now.

This review did not modify Rust program logic, per its own scope (no
major implementation code) — items 3 and 4 above are recommendations for
the next development session, not changes already made. The one
exception is mechanical and behavior-preserving: `cargo fmt --all` was
run to fix Finding 2.5 (already-committed code that failed its own
formatting check), verified afterward with a full `cargo build
--workspace` and `cargo test --workspace` to confirm nothing changed
except whitespace.
