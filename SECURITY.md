# Security Policy

## Supported Versions

Canary Engine is pre-1.0 (see [versioning scheme](docs/decisions/architecture-decision-records/0006-versioning-scheme.md)).
Until `v1.0.0`, there is no long-term support window: security fixes land on
the `main` branch and are included in the next `pre` or point release. Once
the project reaches `v1.0.0`, this section will define a supported-version
table with backports for the current major line.

| Version           | Supported          |
| ------------------ | ------------------ |
| `main` / latest `v0.0.x-preN` | ✅ |
| Older pre-releases  | ❌ (upgrade instead) |

## Reporting a Vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Once this repository is published, use GitHub's private vulnerability
reporting feature (**Security → Report a vulnerability** on the repository
page) so the report stays confidential until a fix is available. This is the
preferred channel because it doesn't depend on a maintainer's personal inbox
staying current, and it gives GitHub's advisory database a structured record
once a fix ships.

When reporting, please include:

- A description of the vulnerability and its potential impact
- Steps to reproduce, or a minimal repro project/plugin if applicable
- The affected version/commit
- Whether the issue is in engine code, the plugin/WASM sandboxing boundary,
  or a third-party dependency

### Why the plugin sandbox boundary gets special attention

Canary's two-tier plugin architecture (see
[plugin-system.md](docs/architecture/plugin-system.md)) treats the WASM
component sandbox as a security boundary for untrusted, marketplace-distributed
content. Reports that demonstrate a sandbox escape, a capability-model bypass,
or a way for a WASM plugin to affect the host process outside its declared
capabilities are treated as critical severity by default, independent of
whatever severity the reporter suggests.

## Disclosure Process

1. Report received and privately acknowledged (target: within 5 business
   days, though this project currently has no paid on-call staff, so treat
   this as a goal, not an SLA).
2. Maintainers confirm, assess severity, and develop a fix on a private
   branch.
3. A new release is published with the fix; a GitHub Security Advisory is
   published crediting the reporter (unless anonymity is requested).
4. Public disclosure follows the release, generally on a coordinated
   timeline agreed with the reporter.

## Scope

This policy covers the `canary` repository itself (engine core, ECS, plugin
loader, build tooling). Vulnerabilities in third-party dependencies should
generally be reported upstream as well; if you're unsure whether something is
a Canary issue or an upstream one, report it here and we'll help route it.
