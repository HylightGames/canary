# Canary Engine release-readiness review

**Review date:** 2026-10-11  
**Reviewed revision:** `15cee829963c5b4063762d49f3e5e11f0b1d65b9` (`dev`, `docs: v0.1.0 release notes, checklist, readiness audit, sample guide`)  
**Repository:** `/home/cloudy/Documents/canary`  
**Review type:** full release-readiness audit with targeted remediation  
**Final status:** **NOT READY** for a public or generally distributed `v0.1.0` release

This report supersedes neither the dated historical audit at
[`2026-10-v0.1.0-readiness-audit.md`](2026-10-v0.1.0-readiness-audit.md) nor any
ADR. It records the state examined here and explicitly distinguishes findings
that remain open from findings fixed during this review.

## A. Review metadata and scope

### Baseline

The working tree was clean at the beginning of the review. The branch was
`dev`, 11 commits ahead of `origin/dev`, and the reviewed commit was
`15cee82`. During the parallel read-only audit, changes appeared in
`engine/canary-core/src/app.rs`, `engine/canary-render-ecs/src/lib.rs`, and
`examples/collectathon/src/scene.rs`. They were inspected and preserved; no
pre-existing work was reset or overwritten. This review subsequently made
focused changes in those files and in `engine/canary-runtime/src/lib.rs`.

The final working tree contains only those four source-file changes. No commit,
push, tag, package publication, release, external network probe, or destructive
operation was performed.

### Environment

- Linux 7.2.9-arch1-1, x86_64
- `rustc 1.98.1 (48a229cea 2026-09-01)`
- `cargo 1.98.1 (797e8a9bc 2026-08-05)`
- Toolchain selected by `rust-toolchain.toml`: current `stable`
- 49,382 filesystem entries were inventoried, including ignored build output;
  207 tracked first-party Rust source files were enumerated for focused review.

### Examined areas

The review covered:

- Workspace manifests, lockfile, toolchain, lint configuration, and dependency
  direction across all engine crates, examples, and `tools/xtask`.
- `canary-core` lifecycle, `canary-runtime` composition and frame driver,
  ECS/scheduler/transform, assets, physics, audio, input, state, networking,
  collaboration, localization, plugin Tier A/Tier B, UI, and rendering.
- Native Vulkan surface/presenter/resource ownership and unsafe boundaries.
- CI, security, Trivy, CodSpeed, release workflows, permissions, fixtures,
  release notes, checklists, packaging metadata, and setup scripts.
- Architecture documents, roadmap/status, ADRs, risk register, sample guides,
  and the prior dated readiness audit.
- Tests and regression paths, including malformed-input and resource-limit
  coverage where source review showed a relevant boundary.

The audit was evidence-driven but not a claim that every source line was read
manually. Generated `target/` content and third-party sources were not treated
as first-party code. macOS/Windows live runtime tests, real audio hardware,
real GPU hardware, Miri, fuzzing, semver compatibility tooling, and a clean
checkout were not available or were not run here.

## B. Executive assessment

### Decision: NOT READY

The repository is in substantially better shape than the older October audit:
the proof fixture, Wasmtime resolution, CI headless/QUIC/collaboration gate,
render-bridge integration, and documentation warning issues were addressed or
improved. The current local engineering gates are green.

That is not sufficient for a responsible public release. The following remain
release-blocking or release-scope conditions:

1. The safe Vulkan presentation API does not encode the lifetime or identity of
   the native window from which the surface was created. A safe caller can drop
   the window or pass a different provider while native handles remain in use.
   This is a native resource ownership/soundness defect.
2. Vulkan resources and encoders do not encode or validate device-instance
   provenance. A target, buffer, pipeline, texture, or encoder from one
   `VulkanDevice` can be passed to another same-typed device, producing invalid
   Vulkan handle use.
3. Release automation accepts arbitrary `v*` tags, does not verify the tag
   against the workspace version, branch, or required release evidence, and
   permits missing curated release notes. Third-party Actions are mutable tag
   references in write-capable jobs.
4. The generic QUIC server and collectathon server authenticate the server
   transport but not the client/application identity. The one-session example
   can also be held indefinitely by a connected slow client. These are
   acceptable only for a deliberately local proof/demo, not a public service.
5. The `v0.1.0` live-device evidence rows for windowed play and audio are still
   unchecked in the sample README and status document.

For a local proof run bound to loopback, with trusted callers and the Vulkan
API treated as experimental, the repository can be described as **green with
explicit conditions**. That narrower statement must not be confused with
public release readiness.

## C. Findings register

Severity is based on practical impact and prerequisites, not on whether a
finding is easy to fix.

### ARCH-001 — Vulkan presenter does not encode the window lifetime or identity

**Severity:** Critical for the safe API contract  
**Status:** Open; release blocker  
**Where:** `engine/canary-render-vulkan/src/presenter.rs:20-26,50-73,85-103,176-188`

The module documentation says `VulkanPresenter` borrows the window for its
whole lifetime, but the struct has no lifetime parameter, `PhantomData`, or
stored provider. `new` copies raw native handles and returns an owned presenter.
Each presentation method accepts an arbitrary provider again.

A safe caller can construct a presenter from window A, drop A, and later call
presentation; or keep A alive and pass provider B. The Vulkan surface and
swapchain retain handles originating from A while extent/generation/handles
for later operations are read from the supplied provider. `WindowHandles`
explicitly documents that the pointers are invalid after the window dies
(`engine/canary-platform/src/surface.rs:25-29`).

**Impact:** Native surface handles can be used after their owning OS objects
are destroyed or combined with a different window. This crosses an unsafe FFI
lifetime boundary through a safe Rust API and can yield driver-defined failure
or undefined behavior.

**Recommended remediation:** Make presenter ownership/lifetime and same-window
identity a type-level invariant, or make construction/presentation use a
window-owned presenter token that cannot outlive the provider. At minimum,
retain an identity-bearing owner and reject mismatched providers; documentation
alone is insufficient.

**Verification:** Source-level reproduction is direct. No native exploit or
destructive test was run.

### ARCH-002 — Vulkan resources lack device-instance provenance

**Severity:** High  
**Status:** Open; release blocker for safe multi-device use  
**Where:** `engine/canary-render-vulkan/src/color_target.rs:24-40,69-80`,
`engine/canary-render-vulkan/src/presenter.rs:208-252`,
`engine/canary-render-vulkan/src/device.rs:769-811`,
`engine/canary-render-vulkan/src/encoder.rs`

`VulkanColorTarget` retains an `Rc<ash::Device>` to keep a device alive, but no
owner identity is exposed or checked. `present_color_target` accepts any
`VulkanColorTarget` and records its image using the presenter's device. The
same unbranded associated resource types are accepted by encoder operations.

A safe caller can create two `VulkanDevice` values, create a target or buffer
on device A, and pass it to a presenter/encoder from device B. The Rust types
match, but Vulkan handles are owned by different `VkDevice` instances.

**Impact:** Invalid native-handle use can trigger validation failures, driver
crashes, or undefined behavior.

**Recommended remediation:** Brand resources and encoders by an owning device
instance, or carry a private identity token and reject mismatches at every
cross-resource operation. `Rc` lifetime retention is not provenance checking.
Add negative tests using two logical devices where the platform permits it and
unit-level owner-token tests otherwise.

### SEC-001 — QUIC server admits unauthenticated clients

**Severity:** High when network-exposed; medium for the local proof  
**Status:** Open; public-deployment blocker  
**Where:** `engine/canary-net/src/transport.rs:141-159`,
`examples/collectathon/src/bin/collectathon_server.rs:236-280`

The server configuration uses `with_no_client_auth`. The handshake verifies
protocol/version fields, but no client certificate, application credential,
identity, or admission token is required. TLS authenticates the server
transport, not the peer.

Any network-reachable client able to complete the server handshake can receive
authoritative state and submit whatever inputs the protocol accepts. The
architecture and roadmap acknowledge identity as future/deployment policy,
which makes this an explicit scope limitation rather than a hidden protocol
feature.

**Recommended remediation:** Require mTLS or an authenticated application
handshake before sending `Welcome` or snapshots; make unauthenticated mode
explicitly loopback/proof-only and reject public binds without a policy.

### REL-001 — Release workflow does not validate tag, version, branch, or evidence

**Severity:** High  
**Status:** Open; release blocker  
**Where:** `.github/workflows/release.yml:17-20,29-56,58-85`

The workflow triggers on any tag matching `v*`. It does not verify SemVer, the
workspace package version, an annotated tag, ancestry from `stable`, or a green
CI/release-gate result. The build matrix runs only release build/test. It does
not run the headless/QUIC/collaboration release gate, `fmt`, Clippy, WASI, or
live/integration targets.

When `docs/release-notes/${GITHUB_REF_NAME}.md` is absent, the workflow emits a
warning and publishes auto-generated notes. This contradicts the documented
curated release-note requirement and permits a successful release whose Cargo
version, tag, tested commit, and release body do not agree.

**Recommended remediation:** Add a pre-publish verification job that checks
exact workspace/tag version equality, SemVer and tag ancestry/branch policy,
required release-note existence, and all required automated gates. Make the
publish job depend on that job. Decide explicitly whether annotated-tag
validation is enforced by automation or by branch policy.

### REL-002 — CI Actions use mutable tag references in privileged jobs

**Severity:** High supply-chain hardening gap  
**Status:** Open; release blocker for a hardened release process  
**Where:** `.github/workflows/release.yml:39,66,78`,
`.github/workflows/ci.yml` checkout steps,
`.github/workflows/trivy.yml:29,36,49`,
`.github/workflows/codspeed.yml:33,39,61`

Third-party and GitHub Actions are referenced by mutable tags such as
`actions/checkout@v7`, `softprops/action-gh-release@v3`, and other major/version
tags. The release job has `contents: write`; CodSpeed has `id-token: write`; and
Trivy has security-events permissions.

If an upstream tag is compromised or retagged, arbitrary action code runs in
those job contexts. This is a confirmed provenance/control weakness even
without assuming that an upstream tag is currently compromised.

**Recommended remediation:** Pin every action to a full commit SHA, retain an
update comment/Dependabot mechanism, and minimize per-job permissions. Review
whether the CodSpeed OIDC permission is needed for every trigger, especially
pull requests.

### REL-003 — One-session server has no post-accept or idle deadline

**Severity:** Medium availability; high if exposed publicly  
**Status:** Open  
**Where:** `examples/collectathon/src/bin/collectathon_server.rs:222-237,356-357`,
`engine/canary-net/src/transport.rs:54-57`

The example times out `accept`, then serves exactly one client. Handshake and
input reads have no deadline. A client can connect and stop before the next
frame, or send an incomplete/idle frame, holding the only session indefinitely
and preventing the legitimate client from connecting. Frame size bounds do not
bound wait time.

**Recommended remediation:** Apply handshake, per-frame, idle, and whole-session
deadlines; integrate `ConnectionPolicy` into the actual driver; reject or
replace an idle first client before consuming the sole service slot.

### SEC-002 — Generated TLS private key uses ordinary filesystem permissions

**Severity:** Medium local confidentiality  
**Status:** Open; sample should be hardened or explicitly disposable  
**Where:** `examples/collectathon/src/bin/collectathon_server.rs:190-203`

`server.key` is written with `std::fs::write`, which creates a new file with
normal `0666 & umask` permissions and does not tighten the mode of an existing
file. Under a common `022` umask, the newly created private key is typically
readable by group/other users. The caller may also select the identity
 directory.

**Impact:** Other local users/processes can obtain the server private identity,
which increases impersonation risk, especially if the server is bound beyond
loopback.

**Recommended remediation:** Create the key atomically with mode `0600` on
Unix, reject or repair unsafe existing permissions, and document the identity
directory as disposable proof-only material when production provisioning is
not supported.

### SEC-003 — Locale file loading follows symlinks inside a locale directory

**Severity:** Medium when locale packs are untrusted  
**Status:** Open; deployment-dependent  
**Where:** `engine/canary-loc/src/loader.rs`

The loader rejects a symlinked locale directory, but individual `.ftl` entries
are discovered/opened through normal filesystem operations. A pack containing a
symlinked entry can cause the loader to read content outside the intended pack.

**Recommended remediation:** Open entries with no-follow semantics or
canonicalize and enforce the root for every file, with regression tests for a
symlinked individual locale file. If packs are strictly trusted, state that
boundary explicitly at the public loader API.

### COR-001 — Physics collider/material state is not reconciled after body creation

**Severity:** Medium  
**Status:** Open  
**Where:** `engine/canary-physics/src/systems.rs:519-571`,
`engine/canary-physics/src/rapier_backend.rs:118-128,291-322`

Collider attachment occurs only in the body-creation branch. If attachment
fails, later ticks never retry when the ECS collider becomes valid. If a
collider or material is removed/replaced after creation, the old backend state
remains because no body-side replacement/removal path is invoked.

**Reproduction:** Create a body with an invalid collider, tick, replace it with
a valid collider, and tick again; the live body remains colliderless. Create a
valid collider, then remove or replace the ECS component; the old Rapier
collider remains. Existing tests cover the failed creation tick but not
reconciliation.

**Recommended remediation:** Track the backend-applied collider/material
fingerprint and reconcile add, replace, remove, and retry transitions each
system pass, or document components as immutable and enforce that contract.
Add transition tests.

### COR-002 — Runtime changes to `RigidBody.kind` are silently ignored

**Severity:** Medium  
**Status:** Open  
**Where:** `engine/canary-physics/src/systems.rs:519-571`,
`engine/canary-physics/src/components.rs:30-63`

`RigidBody.kind` is read only while creating the backend body. Replacing a
component from dynamic to fixed or kinematic leaves the existing Rapier body
dynamic. The public ECS component does not document immutability and there is
no transition API.

**Recommended remediation:** Recreate/update the backend body on kind changes,
or make the component immutable after registration and expose an explicit
transition operation.

### COR-003 — Invalid live Transform quaternion can poison ECS state

**Severity:** Medium  
**Status:** Open  
**Where:** `engine/canary-physics/src/systems.rs:279-312,582-609`

The z-twist helpers normalize quaternions without checking zero/non-finite
length. Creation validates pose data, but an already-live body can receive
`Quat::ZERO` or non-finite authored rotation; the sync path then normalizes it
and writes NaN back into `Transform`.

**Recommended remediation:** Validate authored rotations at the ECS-to-physics
boundary and preserve/skip invalid state rather than normalizing zero or
non-finite quaternions. Add zero, NaN, and infinity regression tests.

### COR-004 — Release render paths retain non-finite colors and UVs

**Severity:** Medium malformed-input robustness  
**Status:** Open  
**Where:** `engine/canary-render-ecs/src/extract.rs:407-415,441-446,495-498`,
`engine/canary-render-ecs/src/textured_renderable.rs:284-295,321-325,363-365`,
`engine/canary-render-ecs/src/camera.rs:313-317,340-344,358-361,406-420`

Color and UV finiteness checks are `debug_assert!` only. Public render items
can contain NaN or infinity, and release builds copy these values into GPU
vertex output. Finite out-of-range UVs are intentionally legal; the issue is
non-finite values.

**Recommended remediation:** Reject or sanitize non-finite colors/UVs at the
runtime boundary in both perspective and orthographic paths, and add release-
semantics regression tests.

### COR-005 — Tier A runtime scoped calls authorize by plugin name, not instance

**Severity:** Medium API-boundary/confused-deputy risk  
**Status:** Open  
**Where:** `engine/canary-runtime/src/lib.rs:377-403`

`call_plugin_scoped` finds a grant by comparing `plugin.name()` with a loaded
or registered name. It does not prove that the passed component is the exact
instance loaded and retained by this `Runtime`. A trusted host caller can pass
a separately loaded component with the same name and have it invoked under
the registered grant. Duplicate registration names are also not rejected.

This is not a guest directly calling a Rust API; it is a host API integrity
problem and a confused-deputy boundary. The smallest correction is an opaque
runtime-owned instance token/handle or identity check, plus duplicate-name
validation.

### COR-006 — Initialization panic cleanup was incomplete (fixed)

**Severity:** Medium lifecycle reliability  
**Status:** Fixed in this review  
**Where:** `engine/canary-core/src/app.rs:73-91,302-352`

Before remediation, `init_all` used `?` inside a loop and did not catch a panic
from a later initializer. `run_for` therefore skipped cleanup of previously
initialized subsystems. The fix catches the initializer panic, calls
`shutdown_all(initialized)`, and resumes the original panic. A regression test
asserts `a:init`, `b:init`, `a:shutdown`, and propagation of `b`'s panic.

**Remaining design question:** the failing initializer itself is not counted as
initialized if it panics after partial resource acquisition. The Subsystem
contract should decide whether partial initialization is rollback/Drop-only or
whether the failing subsystem receives a cleanup callback. That separate
question remains open.

### RENDER-001 — Infallible Vulkan resource creation can panic on expected driver failures

**Severity:** Medium  
**Status:** Accepted/deferred by current architecture; release decision needed  
**Where:** `engine/canary-render/src/lib.rs:87-144`,
`engine/canary-render-vulkan/src/{buffer,color_target,encoder,pipeline,texture}.rs`

The public RHI creation methods return resources rather than `Result`. The
Vulkan backend turns allocation, creation, and driver failures into panics.
This is documented in ADR 0024/release notes, but it conflicts with the general
error-handling standard for expected runtime failures.

A pre-1.0 release may deliberately retain this contract, but the release notes
must clearly state panic-on-driver-failure semantics and the next API decision
must be recorded before the public surface hardens.

### DOC-001 — Architecture and status documents contradict the implementation

**Severity:** Medium process/reliability  
**Status:** Partially fixed; remaining drift open  
**Where:** `engine/canary-runtime/src/lib.rs:32-40` (fixed here),
`docs/architecture/core-runtime.md`, `docs/architecture/engine-overview.md`,
`docs/decisions/architecture-decision-records/0028-authoritative-live-collaboration-operations.md`,
`docs/roadmap/status.md:11-19,722-797`

The runtime crate previously said the headless binary remained on the old
composition path even though `main.rs` uses `Runtime::drive_frame`. The crate
doc was corrected in this review. Other contradictions remain: architecture
docs still describe the complete frame driver as proposed; ADR 0028 still says
Proposed while status/code describe live collaboration as accepted/shipped; and
status text still says the sample/release note is unpushed or absent even though
those artifacts are present in the reviewed tree.

**Recommended remediation:** Reconcile status, architecture, ADR index/status,
and the release checklist on the reviewed commit. Treat stale architecture
claims as defects, not harmless historical prose.

### REL-004 — Release reproducibility depends on floating stable and host packages

**Severity:** Medium  
**Status:** Open policy decision  
**Where:** `rust-toolchain.toml:13-16`, `Cargo.toml:35-41`,
`docs/development/build-system.md:110-118`

The lockfile fixes dependency resolution but `stable` floats with the compiler;
Cargo, system Vulkan/window/audio packages, and Action implementations also
vary. The release workflow does not record or assert a compiler version. This
is an intentional current-stable policy, not a hidden bug, but it is not a
reproducible release toolchain.

**Recommended remediation:** Pin or record the compiler/toolchain for release
builds, or explicitly label releases as current-stable rebuilds and record the
exact toolchain in release evidence.

### REL-005 — Crates.io packaging remains incomplete

**Severity:** Medium for the future crate distribution channel; not a blocker
for the current notes-only GitHub release if crates.io is out of scope  
**Status:** Open  
**Where:** `engine/canary-state/Cargo.toml:1-7`,
`engine/canary-collab/Cargo.toml:13`,
`docs/research/distribution-audit.md:98-123`

The distribution audit identifies missing repository metadata and missing
explicit versions on internal path dependencies. No CI job performs a
`cargo publish --dry-run` by dependency tier. This blocks the documented future
crates.io channel and should be stated in the release decision.

### REL-006 — Live cross-platform/device evidence is incomplete

**Severity:** Medium  
**Status:** Open; blocks the claimed `v0.1.0` integration exit evidence  
**Where:** `.github/workflows/ci.yml:222-310`,
`examples/collectathon/README.md:151-210`,
`docs/roadmap/status.md:772-779`

CI runs live window/Vulkan proofs on Linux X11/Xvfb and software Vulkan. The
macOS/Windows matrix compiles/tests but does not run an equivalent live window
or presentation proof. The sample README still has unchecked windowed-play and
real audio-device rows. Release documentation must name Linux/software-Vulkan
as the live-verified environment rather than implying all desktop targets are
runtime-validated.

### REL-007 — Rustdoc is not a CI gate (partially improved)

**Severity:** Low  
**Status:** Open process gap  
**Where:** `.github/workflows/ci.yml`, release workflow

A local `cargo doc --workspace --no-deps` was initially warning-clean only
after this review fixed a private-module link and two redundant explicit links.
CI does not run `RUSTDOCFLAGS=-D warnings`, so future public API documentation
regressions can land unnoticed.

## D. Architecture assessment

The crate graph is acyclic and largely follows the project's stated direction:
ECS/platform are leaves, backend-neutral render traits do not expose Vulkan
third-party types, and Vulkan, Rapier, Rodio, Quinn, and Wasmtime stay behind
Canary-owned boundaries. `canary-runtime` owns composition and the sample uses
public APIs. Tier A and Tier B are explicitly different trust domains.

The most important architectural weakness is not dependency direction; it is
ownership contracts at the native Vulkan boundary. The presenter and resource
provenance guarantees are documented but not represented in types or checked at
runtime. This is precisely the kind of invariant the architecture rules say
must be enforced rather than merely narrated.

Lifecycle behavior is now more robust for a panic in a later initializer, but
partial-initializer cleanup semantics remain underspecified. Runtime plugin
identity similarly uses names where instance identity is needed.

No wholesale redesign is recommended before addressing the two Vulkan ownership
findings. The smallest viable direction is type/identity branding, then focused
negative tests.

## E. Performance assessment

No performance optimization is claimed from this review. No new benchmark was
run because the release blockers are correctness, ownership, security, and
release-process issues rather than measured hot paths.

Existing project documentation records physics and scheduler measurements, but
those are historical evidence and were not re-established here. The main
unmeasured performance risks remain synchronous asset loading, per-frame CPU
baking/upload, and the one-session network driver. A future performance pass
should benchmark representative sample scenes in release mode and measure
frame/present latency, allocations, and server idle/slow-client behavior.

## F. Security assessment

### Performed

- Read-only source review of plugin loaders, capability linking, path/file
  confinement, locale/assets, authored-state parsing, network framing/limits,
  QUIC setup, credentials, TLS identity handling, unsafe/native boundaries,
  and CI permissions.
- Tracked-file and secret-pattern review without printing candidate contents.
- `cargo audit` reported zero known vulnerability advisories in the resolved
  lockfile. It reported the known unmaintained `ttf-parser` advisory.
- `cargo deny check advisories` was not a clean gate because the same
  unmaintained advisory is not ignored by the local/default configuration.
  The workflow's ignore comment describes a different dependency path than the
  current resolution and should be corrected.
- No real external target was probed, no credentials were rotated, and no
  private key contents were printed.

### Confirmed security findings

The actionable findings are SEC-001 (no client/application authentication),
SEC-002 (private-key file permissions), SEC-003 (individual locale symlinks),
and REL-002 (mutable privileged Actions). Tier B native plugin execution is an
explicit trusted-host boundary, not a sandbox escape finding. Tier A local
component parsing before per-instance budgets and the documented asset TOCTOU
window remain residual risks when an attacker controls local plugin/filesystem
inputs, but were not promoted to remotely exploitable defects here.

## G. Crash, chaos, and resilience assessment

### Executed

- The initialization-panic regression test exercised a subsystem panic after a
  prior subsystem initialized; cleanup and panic propagation now pass.
- Existing workspace tests and the xtask gate exercised malformed codecs,
  bounds, stale handles, state recovery, collaboration break-it cases, and
  loopback/session paths.
- No destructive resource exhaustion or external network test was performed.

### Not executed

- Ignored Xvfb/Vulkan tests were not rerun locally in this audit; their CI
  definitions and existing test source were inspected.
- No fuzzing, Miri run, adversarial native-plugin run, multi-device Vulkan
  negative test, slow-client network experiment, or real audio-device proof was
  available here.
- These limitations do not reduce the severity of the statically demonstrated
  ownership and release-workflow findings.

## H. Test and verification results

| Check | Command/procedure | Result | Limitations |
|---|---|---|---|
| Workspace build | `cargo build --workspace` | PASS | Development profile; release matrix not run locally |
| Workspace tests | `cargo test --workspace` | PASS | Ignored live/device tests excluded |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | PASS | Current host/feature resolution |
| Formatting | `cargo fmt --all -- --check` | PASS | — |
| Orchestrator | `cargo run -p xtask -- check` | PASS | Runs repository's local gates |
| Lifecycle regression | `cargo test -p canary-core app::tests::a_panicking_init_still_shuts_down_what_already_started -- --exact --nocapture` | PASS | Test intentionally emits the caught panic message |
| Workspace docs | `cargo doc --workspace --no-deps` | PASS after fixes | CI does not enforce `-D warnings` |
| WASI target | `cargo check -p canary-plugin-api --target wasm32-wasip2` | PASS | Only plugin-facing crate, as specified |
| Whitespace | `git diff --check` | PASS | Final tree includes uncommitted review changes |
| Dependency audit | `cargo audit` | PASS for vulnerabilities; warning for unmaintained `ttf-parser` | Advisory is intentionally baselined in workflow |
| Cargo deny | `cargo deny check advisories` | FAIL | Fails only on the unmaintained `ttf-parser` advisory in this environment |
| Live window/Vulkan/audio | Xvfb, software Vulkan, physical audio | NOT RUN | Environment limitation; CI has Linux software proofs, sample live rows remain open |
| Performance | Release benchmarks/profiling | NOT RUN | No measured optimization claim |

The initial documentation run exposed one private intra-doc link in
`canary-render-ecs` and two redundant links in `collectathon`; those warnings
were corrected, and a fresh documentation build completed without warnings.

## I. Changes made during this review

1. **Panic-safe initialization cleanup** — `engine/canary-core/src/app.rs` now
   catches a subsystem `init` panic, shuts down previously initialized
   subsystems, and resumes the original panic. Added a regression test covering
   order and propagation. This resolves COR-006's primary failure mode.
2. **Rustdoc warning cleanup** — replaced the private `camera` intra-doc link in
   `engine/canary-render-ecs/src/lib.rs` with a non-linking module reference;
   simplified two redundant `record_baked_frame` links in
   `examples/collectathon/src/scene.rs`.
3. **Runtime documentation correction** — updated
   `engine/canary-runtime/src/lib.rs` to describe the current `drive_frame`
   consumer path instead of claiming the headless binary remained unmigrated.

No public API was intentionally changed. No dependencies, lockfile entries,
CI permissions, release tags, or deployment settings were changed.

## J. Outstanding issues and risk acceptance

The following must be resolved or explicitly accepted by the project owner
before a release decision changes to READY WITH CONDITIONS:

- ARCH-001 and ARCH-002: make native Vulkan window/resource ownership safe or
  explicitly remove the safe API claim and gate the backend as unsafe/experimental.
- REL-001 and REL-002: harden tag/version/evidence checks and pin Actions.
- SEC-001 and REL-003: keep the network proof loopback-only or add identity,
  admission, and deadline policy before any public bind.
- SEC-002: harden generated identity permissions.
- REL-006: record the required live window/audio evidence and state exactly
  which platforms were validated.
- COR-001 through COR-004: add ECS/render regression coverage and reconcile
  backend state transitions.
- DOC-001: reconcile ADR/status/architecture contradictions.
- Decide whether infallible Vulkan resource creation is an accepted pre-1.0
  contract, and record that acceptance in the release checklist.
- Decide whether crates.io packaging is in scope for this release. If yes,
  fix metadata/path-version blockers and add dry-run publication checks.

## K. Release checklist

- [x] Workspace build passes on the reviewed host.
- [x] Workspace tests pass on the reviewed host.
- [x] Workspace Clippy with `-D warnings` passes.
- [x] Formatting check passes.
- [x] `xtask check` passes.
- [x] Plugin WASI target check passes.
- [x] Workspace documentation builds without warnings after focused fixes.
- [x] No secrets or generated artifacts were added by this review.
- [ ] Native Vulkan presenter lifetime and same-window identity are enforced.
- [ ] Native Vulkan resource/encoder device provenance is enforced.
- [ ] Release workflow validates tag/version/branch and requires release notes.
- [ ] Privileged Actions are pinned to immutable commit SHAs.
- [ ] Public network identity/admission/deadline policy is implemented or the
      service is explicitly restricted to loopback proof use.
- [ ] Private identity files use restrictive permissions.
- [ ] `v0.1.0` live window/audio evidence is recorded on named platforms.
- [ ] Physics component replacement/removal and invalid-transform regressions
      are covered.
- [ ] Architecture/ADR/status contradictions are reconciled.
- [ ] Crates.io publish scope and dry-run evidence are decided.

## L. Final recommendation

Do not cut or publicly advertise `v0.1.0` from this revision yet. The local
quality gates are strong evidence that the current tree compiles, tests, and
lint-checks cleanly, but they do not establish safety of the native presenter,
Vulkan resource provenance, public network posture, or release automation.

The next responsible sequence is:

1. Fix or redesign ARCH-001 and ARCH-002, with negative tests at the safe API
   boundary.
2. Harden REL-001/REL-002 and decide whether the network sample is strictly
   loopback-only or gains authentication and deadlines.
3. Complete the sample's live evidence on named platforms/devices.
4. Add the physics/render regression tests and reconcile the architecture/ADR
   status documents.
5. Re-run all four hard gates, ignored integration suites, `cargo audit`,
   release dry-runs, and the final diff review on a clean checkout.

Until those conditions are met, the accurate release label is **NOT READY**,
with a narrower **local proof/demo passes automated gates** qualification.
