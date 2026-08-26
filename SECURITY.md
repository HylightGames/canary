# Security Policy

Canary takes security seriously, particularly at boundaries where engine code interacts with untrusted content, external data, or native code.

This policy explains what is considered a security issue, which versions are supported, how vulnerabilities should be reported, and how reports are handled.

Canary is currently pre-1.0, so this policy will evolve as the engine gains additional capabilities and its threat model becomes more concrete.

## Supported versions

Canary follows the versioning scheme defined in [ADR 0006](docs/decisions/architecture-decision-records/0006-versioning-scheme.md).

Before `v1.0.0`, Canary does not maintain a long-term support branch. Security fixes are applied to the active development line and included in the next appropriate pre-release or stable release.

| Version                              | Supported                       |
| ------------------------------------ | ------------------------------- |
| `main` / current development version | ✅                               |
| Latest released `v0.0.x`             | ✅ where practical               |
| Older pre-releases                   | ❌ Upgrade to the latest version |

Once Canary reaches `v1.0.0`, this section will be updated with an explicit supported-version policy and, where appropriate, security backport rules.

---

## Reporting a vulnerability

**Do not disclose security vulnerabilities through public GitHub issues, discussions, or pull requests.**

Use GitHub's private vulnerability reporting mechanism:

**Repository → Security → Report a vulnerability**

This is the preferred reporting channel because it keeps the report private during investigation and provides a structured record that can later be associated with a GitHub Security Advisory.

If private vulnerability reporting is unavailable for any reason, do not publish the vulnerability publicly. Use the project's documented private security contact if one is provided by the repository.

### What to include

A useful report should include, where possible:

* A clear description of the vulnerability
* The potential security impact
* Reproduction steps
* A minimal reproduction project, plugin, or test case where practical
* The affected version, release, or commit
* The affected platform or environment
* Whether the issue involves engine code, a plugin boundary, project data, networking, build infrastructure, or a third-party dependency
* Any known prerequisites or limitations for exploitation

Please provide enough detail for the issue to be reproduced without requiring unnecessary exposure of sensitive information.

---

## Security boundaries

Canary contains several classes of code and data with different trust assumptions.

### Untrusted content

Canary is designed to support content that should not automatically be trusted, including community and marketplace extensions.

The planned WebAssembly Component plugin system is intended to provide a sandboxed execution boundary for this class of content.

A vulnerability that allows untrusted WebAssembly code to:

* escape its sandbox
* obtain capabilities it was not granted
* access host resources outside its declared interface
* corrupt or arbitrarily manipulate the host process
* execute unauthorized native code

is considered a high-priority security issue and will normally be treated as **critical**.

The exact capability model and sandbox guarantees are defined by the implementation and relevant architecture documentation. Planned security properties must not be interpreted as guarantees for components that are not yet implemented.

### Trusted native plugins

Canary's native plugin mechanism is a **trusted-code boundary**, not a sandbox.

Native plugins using the C ABI execute with the privileges granted to the host process. A native plugin that is already trusted is therefore not expected to provide the same isolation guarantees as a sandboxed WebAssembly component.

Reports should clearly distinguish:

* An unsafe or malicious native plugin behaving as designed because it is trusted
* A vulnerability in the native plugin ABI or loader that allows unintended behavior

### Project data

Malformed, malicious, or deliberately crafted project files are within security scope when processing them can cause unintended effects such as:

* Memory corruption
* Code execution
* Unauthorized file or system access
* Sandbox or capability bypasses
* Denial of service

### Networking

As Canary's networking systems are implemented, vulnerabilities involving authentication, authorization, message validation, remote code execution, denial of service, or state manipulation may fall within the scope of this policy.

Unimplemented or purely hypothetical networking behavior is not considered a current security guarantee.

### Build and release infrastructure

Security issues affecting the project's source distribution, release process, CI, package publication, signing, or build infrastructure may also be reported privately.

---

## Severity

Canary does not currently maintain a formal CVSS-based severity policy. Reports are assessed based on practical exploitability and impact.

Particular attention is given to vulnerabilities that cross a trust boundary.

Examples include:

| Class                       | Typical concern                                                        |
| --------------------------- | ---------------------------------------------------------------------- |
| Sandbox escape              | Untrusted WASM gains unintended host capabilities                      |
| Code execution              | Attacker-controlled data or content executes unauthorized code         |
| Memory safety               | Crafted input causes memory corruption in native engine code           |
| Privilege/capability bypass | Content gains access beyond its declared authority                     |
| Supply-chain compromise     | Malicious dependency, build artifact, or release process modification  |
| Denial of service           | Crafted content or network input reliably crashes or exhausts the host |

Severity may depend on the exact conditions required for exploitation.

A vulnerability does not need to match one of these examples exactly to be reportable.

---

## Disclosure process

Canary aims for responsible and coordinated disclosure.

The general process is:

1. **Private report** — the vulnerability is submitted through the private reporting channel.
2. **Triage** — maintainers reproduce the issue where possible, determine its scope, and assess severity.
3. **Remediation** — a fix, mitigation, or other appropriate response is developed privately when disclosure before remediation would create unnecessary risk.
4. **Release** — affected versions are fixed or users are given appropriate mitigation guidance.
5. **Advisory** — a GitHub Security Advisory or equivalent public notice may be published when appropriate.
6. **Disclosure** — public disclosure is coordinated with the reporter where practical.

Because Canary is currently a small, pre-1.0 project with no dedicated security or on-call staff, the response process is best-effort rather than an SLA.

We will not promise response times that the project cannot reliably maintain.

---

## Fixes and releases

Security fixes should be accompanied by appropriate regression tests and documentation when practical.

Depending on severity and affected versions, a fix may be released as:

* A normal pre-release or point release
* A security-focused patch release
* A mitigation documented before a complete fix is available

When a vulnerability affects an older release that is no longer supported, upgrading to the current supported version is the expected remediation path.

---

## Third-party dependencies

Canary depends on third-party crates, tools, and other software.

If a vulnerability exists entirely within a third-party dependency, it should generally also be reported to that project's maintainers or security process.

However, **please report it to Canary as well when the dependency creates a meaningful vulnerability in Canary's supported use of it**, particularly when:

* Canary exposes the vulnerable functionality directly
* Canary configures the dependency in an unsafe way
* Canary's integration creates an additional attack path
* The issue affects Canary's trust boundaries
* You are unsure where responsibility lies

Maintainers will coordinate with upstream projects when appropriate.

---

## Out of scope

Not every bug is a security vulnerability.

The following are generally handled through the normal issue tracker unless they create a genuine security impact:

* Ordinary crashes with no security implications
* Performance problems
* Visual or rendering bugs
* Incorrect gameplay behavior
* API ergonomics
* Feature requests
* Documentation errors
* Vulnerabilities that exist only in unrelated downstream projects

When in doubt, report privately rather than guessing that an issue is harmless.

---

## Security research

Good-faith security research is welcome.

Please avoid:

* Accessing or modifying data belonging to other users
* Disrupting project infrastructure
* Destroying or corrupting shared resources
* Publicly disclosing an exploitable vulnerability before maintainers have had a reasonable opportunity to respond
* Using a vulnerability to obtain credentials, secrets, or unrelated private information

Researchers should limit testing to systems and data they are authorized to access.

---

## Researcher credit

Canary intends to credit security researchers who responsibly disclose vulnerabilities, unless the researcher requests anonymity.

Credit may be included in the relevant security advisory or release notes.

We will not publicly disclose personal information beyond what the researcher has chosen to provide.

---

## Policy changes

This policy is expected to evolve alongside Canary's threat model.

As systems such as networking, project loading, marketplace distribution, code signing, and sandboxed extensions become implemented, their concrete security guarantees and reporting expectations should be documented here and in the relevant architecture documentation.

This document describes the project's **current security policy**, not a guarantee that every planned security boundary already exists.

For architectural security assumptions, see [`docs/architecture/`](docs/architecture/). For contribution and development practices, see [`CONTRIBUTING.md`](CONTRIBUTING.md).
