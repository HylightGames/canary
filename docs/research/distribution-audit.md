# Distribution-ecosystem audit (2026-10-03)

Status: audit only. No release authorized. Naming constraint: `canary-engine` is
FREE on crates.io (API 404, verified 2026-10-03); `canary` is taken by an
unrelated networking lib — never suggest it.

Guiding distinction (audit now, release plan decides): being able to publish
somewhere is not the same as having a useful thing to distribute there.
`cargo install` installs executable targets, not libraries; `cargo-binstall`
is separate binary-distribution work (Release assets + metadata), not a free
consequence of publication. Four artifact categories, each with its own
package names/manifests/artifacts:

1. Engine libraries — consumed as dependencies (`cargo add`).
2. Developer tooling — installed and run from a terminal.
3. Game/runtime artifacts — embedded in or used to run a project.
4. End-user applications — installed through desktop-oriented channels.

## A. Channel inventory

Sources: official docs fetched 2026-10-03 (librarian lane). Costs/burdens as documented.

### Rust-native (fit now for libs)

- **crates.io** (`canary-engine`, `canary-*`): GitHub login + verified email, API
  token first publish (OIDC/Trusted Publishing after); needs
  `license`+`description`+`repository`(+`readme`) in Cargo.toml, dry-run clean,
  ≤10 MB `.crate`; names first-come, immutable (`cargo yank` hides only).
  Free. Owner: crate owner (`cargo owner --add github:org:team`). Burden: low
  per release (bump, changelog, tag, publish; `cargo-release`/`release-plz`).
- **docs.rs**: automatic on publish; optional `[package.metadata.docs.rs]`.
  Free, very low burden (fix failed builds only).
- **`cargo install`**: installs `[[bin]]` targets only — lib crates are not
  installable. No user-facing bin exists yet (libs + examples + `xtask`);
  plan a `canary-cli`/server bin before this channel means anything.
- **`cargo-binstall`**: needs prebuilt Release archives + optional
  `[package.metadata.binstall]` (`pkg-url`, `bin-dir`, `pkg-fmt`, per-target
  overrides). Only after the first real bin + Release assets; do not add
  metadata prematurely.

### GitHub Releases (fit now for bins)

Tag + Actions workflow; archive layout
`<name>-<target>-v<version>.{tgz,txz,zip>` with top-level `bin-dir`;
`SHA256SUMS` file; SBOM (CycloneDX/SPDX) + provenance via
`actions/attest-build-provenance` (SLSA Build L3, auto-on for public repos);
pre-releases via `prerelease:true`; separate debug symbols (`*.dSYM`,
`*.pdb`, split `*.debug`). Doctrine: **build once, publish everywhere** —
one CI build, same digest reused for binstall archives, manager manifests,
and OCI images; never rebuild per channel. Today `release.yml` is
notes-only: no binaries, checksums, SBOM, signing, or crates.io step.

### OS / version-manager channels (later, first CLI)

- **Homebrew**: own tap first (your repo, no review, low burden); core only if
  a CLI becomes notable (high bar: source builds, core CI, review).
- **Scoop**: JSON manifest + `checkver`/`autoupdate`; ideal for portable Rust
  bins. **WinGet**: YAML manifest PR to `winget-pkgs`, bot + human merge;
  MSI/MSIX preferred. **Chocolatey**: heaviest Windows option (moderation
  queue per version) — prioritize Scoop/WinGet first.
- **Nixpkgs**: PR-only, source builds, `cargoHash` churn, high review burden —
  only on demand (own flake/`cargo2nix` cheaper). **AUR**: `-bin` suffix
  required for prebuilt deliverables; source pkg optional.
- **mise/aqua/asdf**: CLIs only, near-zero burden once Release assets are
  stable; zero value for libs.

### System packages, sandboxed stores, app stores

- **`.deb`/`.rpm`**: FPM stopgap acceptable (low); native (`debian/`,
  `.spec`, `dh-cargo`/`rust2rpm`) high burden — only on sys-integration
  demand. Declare Vulkan/audio sys-deps (`libvulkan1`, `libasound2`).
- **Flatpak/Flathub, Snap, AppImage**: editor-only, later. Vulkan via
  `device=dri`/portal constraints; Snap classic-confinement friction.
- **Mac App Store / Microsoft Store**: much later/likely never for the
  engine ($99/yr Apple, signing, Metal-only, review cycles) — sideload
  via cargo/Releases suffices.

### Server / containers (fit soon)

GHCR (free public) + optional Docker Hub mirror: multi-arch
(`amd64`/`arm64`), multi-stage `cargo chef`, non-root `USER`, `HEALTHCHECK`,
minimal runtime, `cosign` + attestations, tag policy (`:vX.Y.Z`+`:latest`+
`:sha`). First containerizable surface: headless net server/sims —
image wraps the same Release digest (§ doctrine). Worthwhile only because
headless is genuinely useful, not because the project uses Rust.

### Specialty (mostly no-fit, recorded)

- **LuaRocks**: does NOT solve Luau distribution (incompatible runtimes) —
  ship Luau via crates.io + docs + Release zips. **vcpkg/Conan**: need a
  stable C ABI — currently none (Rust-internal); revisit post-1.0 via
  `canary-plugin-api` C surface. **GitHub Actions Marketplace**: small fit
  now (composite `setup-canary`/`build-game` actions, no human review).
- **WASM registries** (`wa.dev`, warg/`wasm-pkg-tools`): experimental, only
  after a WASM-clean crate exists. **Game platforms/consoles**: demo game
  only, with access; not engine SDK.

## B. Artifact inventory (verified on `dev`, 2026-10-03)

25 workspace members, all `version.workspace = 0.0.16`, MIT, `repository`
workspace-wide **except `canary-state`** (missing `repository` line —
sole metadata outlier). No `homepage`/`documentation`/`readme` keys
anywhere; no per-crate LICENSE files (root `LICENSE` only); zero `git =`
deps (good).

Publishable set (19 engine libs; `publish=false` correctly excludes
`canary-runtime`, 3 examples, `xtask`):

- T0 leaves (any order): canary-core, canary-platform, canary-ecs,
  canary-render, canary-state (+fix repository line first), canary-loc,
  canary-net.
- T1: canary-scheduler (←ecs); canary-plugin-api (←ecs);
  canary-assets (←ecs); canary-ui-core (←platform); canary-input
  (←ecs,platform).
- T2: canary-transform (←ecs,scheduler); canary-collab (←state);
  canary-render-vulkan (←render); canary-ui-egui (←platform,render,ui-core).
- T3: canary-render-ecs (←assets,ecs,scheduler,transform,render);
  canary-physics (←ecs,scheduler,transform);
  canary-audio (←ecs,scheduler,transform,assets).

Global pre-publish work: add `version = "0.0.16"` (or caret) alongside each
`path` dep (cargo demands versions at publish) + the canary-state
repository line; optionally `readme`/`homepage`/`documentation` keys.

Executables (9 binaries, all `publish=false` — **end-user-app category is
empty**): headless boot-harness (`canary-runtime` bin), `xtask check`
(only genuine dev tool), `spinning-cube`, `ui-game`, and collectathon's 5
(`collectathon` windowed, `_headless` twin, `_server`/`_client` QUIC pair,
`_collab_client` edit CLI). Headless/CI-safe: xtask, headless twin,
server/client (UDP loopback), collab CLI. Need window/GPU/audio:
spinning-cube (Vulkan ICD), ui-game, windowed collectathon (Rodio
headless fallback).

## C. Release + security audit

- Ordering: T0→T3 tiers above; no cycles; no git deps.
- Metadata: fix list in B (repository line + versioned path deps).
- Reproducibility: `--locked` builds already in release.yml; add
  checksums + SBOM + attestations when binaries ship.
- License compliance: MIT workspace-wide; per-file provenance exists for
  fixtures (`collectathon/assets/LICENSES.md`); keep license-file
  packaging in mind at publish.
- Signing: SLSA attestations auto-available; `cosign` for OCI later.
- Tag↔artifact match: release.yml already gates on `v*` tags with
  per-tag notes (`docs/release-notes/<tag>.md`); extend to asset manifest
  when binaries attach.

## D. Automation plan

One canonical pipeline: tag `v*` → 3-OS matrix builds each artifact once →
attach archives + `SHA256SUMS` + SBOM + attestations to the GitHub Release
→ downstream channels (binstall metadata already in manifests; Scoop/WinGet/
AUR-bin/Homebrew-tap bumps; GHCR image from same digest) consume the same
bytes. `cargo-release`/`release-plz` for the T0→T3 crates.io sequence.
No per-channel rebuilds. Composite GHA (`setup-canary`) once Releases
carry bins.

## E. Phased plan

- **Release blockers (before any publish)**: canary-state `repository` line;
  `version` on all path deps; dry-run clean per tier (`cargo publish
  --dry-run` T0→T3); decide crate READMEs.
- **v0.1.0-appropriate**: `canary-engine` + 19 `canary-*` on crates.io
  (docs.rs follows free); Release workflow gains real archives for the
  collectathon/headless/server bins + checksums + attestations; tiny
  `setup-canary` action.
- **Low-effort follow-ups**: `[package.metadata.binstall]`; own Homebrew
  tap + Scoop bucket + AUR `-bin`; GHCR headless image; WinGet manifest.
- **Wait for a real product**: Chocolatey/Nixpkgs (on demand), `.deb`/`.rpm`
  (native), Flatpak/Snap/AppImage + both app stores (editor only),
  vcpkg/Conan (C ABI), WASM registries (WASM-clean crate), LuaRocks
  (never for Luau), console SDKs (access-gated).
- **Never**: publish `canary-runtime`/examples/`xtask`; label a sample
  executable as the engine; rebuild per channel; invent per-manager build
  pipelines.
