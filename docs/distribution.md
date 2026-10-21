# Distribution

Where Canary is, and will be, published. This is a public registry of channels
and identifiers, not a setup guide: account creation, credentials, and
machine-specific steps stay out of the repository.

- Canonical source: <https://github.com/HylightGames/canary>
- Publisher identity: HylightGames
- Last checked: 2026-10-21

The ecosystem research and per-channel rationale live in
[`docs/research/distribution-audit.md`](research/distribution-audit.md). This
page records only the current plan and status.

## Naming

- Prefer `canary` for user-facing artifacts and package identifiers.
- Use `canary-engine` only where `canary` is already taken or the registry
  needs a distinct project name.
- `canary` on crates.io is an unrelated networking crate. The engine package
  there is `canary-engine`; internal workspace crates keep their `canary-*`
  names.
- The CLI package is `canary-cli`; the installed binary is `canary`. No
  user-facing CLI ships yet.
- Publisher-prefixed registries use `HylightGames.Canary`
  (WinGet/NuGet); npm uses the scoped `@hylightgames/canary`.

## Status vocabulary

| Status | Meaning |
| --- | --- |
| Existing | Already in the repository or produced by CI. |
| Planned | Identifier chosen, nothing registered or published. |
| Reserved | Identifier registered, but no artifact is published yet. |
| Automatic | Derived from another channel once that channel publishes. |
| Submitted | Registration or review is in flight. |
| Published | Artifact is publicly installable. |
| Not pursued | Deliberately deferred; no name is claimed. |

A 404 or an empty registry search means "no listing found", **not** a
reservation. Only an actual registration changes a status to Reserved or
Submitted.

## Availability check (2026-10-21)

| Registry | `canary` | `canary-engine` | Chosen identifier |
| --- | --- | --- | --- |
| crates.io | taken — unrelated networking crate | available | `canary-engine` |
| AUR | no package found | no package found | `canary-bin`, `canary-git`, `canary` |
| Snap Store | no snap found | no snap found | `canary` |
| Chocolatey | no listing found | no listing found | `canary` |
| Flathub | no app found | no app found | `io.github.HylightGames.Canary` |
| Nixpkgs | no `pkgs/by-name` entry | no `pkgs/by-name` entry | `canary` |
| vcpkg | no port found | no port found | `canary` |
| Conan Center | taken | available | `canary-engine` |
| PyPI | taken | available | `canary-engine` |
| npm | taken | not checked | `@hylightgames/canary` |
| NuGet | no listing found | no listing found | `HylightGames.Canary` |
| WinGet | n/a | n/a | `HylightGames.Canary` |
| MacPorts | appears taken | not checked | `canary-engine` (verify) |

Homebrew and Scoop are scoped to project-owned repositories, so their formula
and manifest names do not need a global availability check.

## Channel plan

| Tier | Channel | Identifier | Status | Notes |
| --- | --- | --- | --- | --- |
| P0 | GitHub Releases | `HylightGames/canary` | Existing | Tag-triggered; publishes notes only, no archives or checksums yet. |
| P0 | crates.io | `canary-engine` + publishable `canary-*` crates | Planned | 19 publishable crates; metadata and dry-runs first. |
| P0 | docs.rs | `docs.rs/canary-engine` | Automatic | Follows the crates.io publish. |
| P0 | cargo-binstall | `canary-cli` | Planned | Needs release archives plus package metadata. |
| P0 | GHCR | `ghcr.io/hylightgames/canary` | Planned | Headless/server image; no image workflow exists yet. |
| P1 | Homebrew tap | `HylightGames/homebrew-canary`, formula `canary` | Planned | Tap repository does not exist yet. |
| P1 | Scoop bucket | `HylightGames/scoop-bucket`, manifest `canary` | Planned | Bucket repository does not exist yet. |
| P1 | WinGet | `HylightGames.Canary` | Planned | No manifest directory yet. |
| P1 | Chocolatey | `canary` | Planned | No listing found; heaviest Windows channel. |
| P1 | AUR | `canary-bin` (prebuilt), `canary-git`, `canary` | Planned | No package found. |
| P1 | Nix flake | `github:HylightGames/canary` | Planned | No `flake.nix` in the repository yet. |
| P1 | Nixpkgs | `canary` | Not pursued | Prefer the flake; upstream PR burden is high. |
| P1 | OBS / APT / COPR | n/a | Not pursued | Later, once `.deb`/`.rpm` artifacts exist. |
| P2 | Flathub | `io.github.HylightGames.Canary` | Planned | Requires a distributable GUI application. |
| P2 | Snap Store | `canary` | Planned | Requires a distributable GUI application. |
| P2 | MacPorts | `canary-engine` | Planned (low priority) | `canary` appears taken; verify before submitting. |
| P2 | Direct installers | `Canary-<version>-<platform>` | Planned | MSI/MSIX/dmg/AppImage after signing exists. |
| P3 | vcpkg | `canary` | Not pursued | Needs a stable C ABI. |
| P3 | Conan Center | `canary-engine` | Not pursued | Needs a stable C ABI. |
| P3 | npm | `@hylightgames/canary` | Not pursued | Only with a real JavaScript artifact. |
| P3 | PyPI | `canary-engine` | Not pursued | Only with a real Python artifact. |
| P3 | NuGet | `HylightGames.Canary` | Not pursued | Only with a real .NET artifact. |
| P3 | Editor extensions | `HylightGames.*` | Not pursued | Only with a real extension. |
| P4 | Mirrors / Repology | n/a | Not pursued | Revisit after stable releases. |

Never publish an empty or placeholder package just to hold a name.

## Planned install targets

None of these commands work yet; they are the target user experience.

```sh
cargo install canary-cli
brew install hylightgames/tap/canary
scoop bucket add canary https://github.com/HylightGames/scoop-bucket
scoop install canary
winget install HylightGames.Canary
nix profile install github:HylightGames/canary
```

## Release gates

A channel may move to Published only when all of these hold:

1. One artifact is built once per tag; every adapter consumes those same bytes.
2. Archives are immutable and published with `SHA256SUMS` and an SBOM or
   provenance attestation.
3. Clean install, upgrade, reinstall, and uninstall are verified on the target
   OS before publishing.
4. Package metadata is correct: name, version, license, description,
   repository, and README.
5. Stable and prerelease versions are separated, and update automation has a
   named maintainer.
6. This page and the channel's own documentation are updated in the same
   change that publishes or retires the channel.

## Maintenance

The project maintainers own this registry. Before each release, re-check every
Planned or Submitted row and update the date in the heading of this document.
A channel that stops being maintained is marked Not pursued and its package is
deprecated or removed rather than left pointing at stale artifacts.
