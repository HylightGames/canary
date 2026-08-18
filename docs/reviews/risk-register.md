# Risk Register

A living list of tracked architectural risks. Unlike
[the dated reviews](README.md#index), this document is meant to be
**edited in place** as risks are mitigated, downgraded, or newly
discovered — update it rather than leaving it as a historical snapshot of
one review. When a review finds something new, add it here; when
something is fixed, update its status rather than deleting the row (a
risk that was real and got fixed is useful history for "why does this
code look like this").

**Severity** reflects cost-if-ignored at scale, not just current impact.
**Status**: Open / In Progress / Mitigated / Accepted (a risk can be
knowingly accepted rather than fixed — that's a legitimate outcome as
long as it's a decision, not a default).

| ID | Area | Risk | Severity | Status | Notes / tracking |
|---|---|---|---|---|---|
| R-01 | Governance | No succession plan; single point of failure for all architectural decisions | Critical | Mitigated | [`GOVERNANCE.md`](../../GOVERNANCE.md) added Aug 2026; still names only one active maintainer — re-assess once a second maintainer joins |
| R-02 | Build/CI | `RUSTFLAGS: "-D warnings"` in CI can fail on dependency-only warnings, unrelated to this project's own code | Critical | Open | Review §2.1/§8.1; fix is config-only, recommended immediately |
| R-03 | Plugin ABI | Tier B vtable has no version field or extension mechanism; future additions risk silent ABI breakage | Critical | Mitigated | [ADR 0009](../decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md) implemented for `v0.0.1`; validated by a test that compiles a real C plugin with a wrong version and confirms rejection |
| R-04 | ECS | Component identity (`TypeId`) has no meaning across the Rust/WASM/cross-language boundary | Critical | Mitigated | Fixed for `v0.0.2`: [ADR 0010](../decisions/architecture-decision-records/0010-component-identity-across-language-boundary.md) (Accepted); first cut prototyped alongside the archetype migration (`CanaryComponent`, `World::register_component`/`type_id_for_schema`) — the actual Tier A consumer is still future work |
| R-05 | Rust architecture | ECS component storage lacks `Send + Sync` bounds; contradicts the target parallel job-system design | High | Mitigated | Fixed for `v0.0.1`: `World::insert` now requires `T: Send + Sync`; a compile-time guard test (`world_is_send_and_sync`) prevents regression |
| R-06 | Governance/Legal | No DCO/CLA; contribution provenance unestablished | High | Open | Review §1.2; cheap now, effectively impossible to backfill across thousands of historical commits later |
| R-07 | Naming/Administrative | Bare `canary` and `canary-rs` crate names already taken on crates.io; a future meta-crate can't use the plain name | High | Open | Review §1.3, verified via web search Aug 2026; `canary-engine` availability unconfirmed — needs a direct, authoritative check |
| R-08 | Plugin ABI / Marketplace | No plugin manifest format; marketplace tooling has nothing to read without executing plugin code | High | Open | Review §3.2; resolve before Era 6 (marketplace) work starts |
| R-09 | Plugin ABI / Marketplace | "Trusted" Tier B plugins have no signing/provenance/verification mechanism | High | Open | Review §3.3; name as an open requirement in `future-roadmap.md`, not urgent pre-marketplace |
| R-10 | Naming/Branding | "Canary" carries strong existing "nightly build channel" connotations (Chrome Canary, Xenia Canary) | Medium | Accepted | Review §1.4; not necessarily worth a rename, but should be a known, named tradeoff, not a surprise |
| R-11 | Rust architecture | Errors crossing the plugin boundary are `Display`-string-shaped, not stable-code-shaped | Medium | Open | Review §2.3; define stable error codes before external tooling starts informally parsing error text |
| R-12 | ECS | Entity generation counter (`u32`) can wrap on a long-lived, hot-recycled slot; wraparound is currently silent | Medium | Mitigated | Fixed for `v0.0.1`: generation widened to `u64`, moving wraparound from "plausible over years of uptime" to "not reachable by any realistic runtime" |
| R-13 | ECS | Change detection is required by 3+ subsystems but designed in none | Medium | Mitigated | Fixed for `v0.0.2`: implemented as `World::query_changed_since`, a per-column, per-row tick that survives archetype moves caused by unrelated components — designed alongside the archetype migration per Review §4.3 |
| R-14 | Language independence | WIT interface versioning strategy undecided | Medium | Open | Review §5.2; resolve before Tier A lands |
| R-15 | Language independence | Host/component call-boundary performance for bulk ECS data is asserted, not measured | Medium | Open | Review §5.3; benchmark early in Tier A work before API surface assumes a cost model |
| R-16 | Scripting | Hot-reload state "serialization contract" has no actual design | Medium | Open | Review §6.1 |
| R-17 | Scripting/UX | Visual scripting's WASM-compile-per-edit may conflict with the immediate-feedback UX principle | Medium | Open | Review §6.2; needs explicit reconciliation (e.g. interpreted fast path) when visual scripting design starts |
| R-18 | Build system | `xtask check` silently diverges from what CI actually gates on (skips clippy) | Medium | Mitigated | Fixed for `v0.0.1`: `xtask check` now detects clippy availability and runs it when present, warning clearly when it isn't, instead of unconditionally skipping |
| R-19 | Plugin ABI / Marketplace | No engine/plugin compatibility-range declaration mechanism | Medium | Open | Review §3.4 |
| R-20 | Repository structure | No `CODEOWNERS` / path-based review routing | Medium | Mitigated | Minimal, honest `CODEOWNERS` added Aug 2026 (single current maintainer); expand as maintainers are added |
| R-21 | Rust architecture | `missing_docs` lint applied uniformly to binary crates with no real public API | Low | Accepted | Review §2.4; harmless, revisit if it becomes noise |
| R-22 | Repository structure | Flat `engine/*` layout has no stated scaling plan past ~dozens of crates | Low | Open | Review §1.6 |
| R-23 | ECS | No stated position on entity hierarchy/relationships | Low | Open | Review §4.4; name as an open question for the archetype-migration era |
| R-24 | ECS | System ordering beyond automatic data-conflict detection is unaddressed | Low | Open | Review §4.5 |
| R-25 | Language independence | "Language-agnostic" vision-level claims may overpromise relative to today's realistic Tier A tooling (Python/C#/JS immaturity) | Low | Open | Review §5.4; point vision docs at the research doc's more precise claim |
| R-26 | Marketplace | No naming-collision policy for published plugins | Low | Open | Review §7.1 |
| R-27 | Governance | No stated position on marketplace monetization | Low | Open | Review §7.2; belongs in `GOVERNANCE.md` as "to be decided, here's who decides," not a technical ADR |
| R-28 | Build system | No CI build-cache strategy | Low | Open | Review §8.3; not urgent at current scale |
| R-29 | Versioning | Pre-1.0 CHANGELOG entries don't distinguish cosmetic churn from breaking redesigns | Low | Open | Review §9.2; adopt a lightweight `[breaking]` tag convention |
| R-30 | Versioning | Workspace crate lockstep versioning was implemented but undocumented as a decision | High | Mitigated | [ADR 0008](../decisions/architecture-decision-records/0008-workspace-crate-versioning-lockstep.md) added Aug 2026, formalizing the existing choice |
| R-31 | Build/CI | `rustfmt.toml` specified nightly-only options (silently not applied on the pinned `stable` toolchain); already-committed code failed its own `cargo fmt --check` | High | Mitigated | Review §2.5; fixed directly (mechanical, behavior-preserving) — nightly-only options removed, `cargo fmt --all` run, rebuild + full test pass confirmed afterward |

## Using this register

- New risks from any future review get appended here with the next `R-NN`
  ID, not inserted out of order.
- A risk moving from `Open` to `Mitigated` should say what changed and
  link to the ADR/PR/doc that did it, the way R-01, R-20, and R-30
  do above.
- `Accepted` is a legitimate terminal state — not everything needs
  fixing, but an accepted risk should say *why* it was accepted (see
  R-10, R-21), not just sit unexplained.
