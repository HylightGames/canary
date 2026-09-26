# Architecture Decision Records

ADRs capture cross-cutting choices and their tradeoffs. They are append-only:
use the next number for a new decision, and amend or supersede an existing ADR
explicitly rather than silently rewriting its history. See
[ADR 0001](0001-record-format.md) for the record format.

This is a navigation index. Each linked record is authoritative for its own
status; check the record before relying on a decision.

| ADR | Decision |
|---|---|
| [0001](0001-record-format.md) | ADR record format and decision process |
| [0002](0002-primary-language-selection.md) | Primary language selection |
| [0003](0003-plugin-and-modding-architecture.md) | Plugin and modding architecture |
| [0004](0004-rendering-abstraction-strategy.md) | Rendering abstraction strategy; superseded in part by ADR 0016 |
| [0005](0005-build-system-and-tooling.md) | Build system and tooling |
| [0006](0006-versioning-scheme.md) | Versioning scheme |
| [0007](0007-networking-and-multiplayer-model.md) | Networking and multiplayer model |
| [0008](0008-workspace-crate-versioning-lockstep.md) | Workspace crate versioning in lockstep |
| [0009](0009-plugin-abi-versioning-and-extensibility.md) | Plugin ABI versioning and extensibility |
| [0010](0010-component-identity-across-language-boundary.md) | Component identity across language boundaries |
| [0011](0011-canaryui-abstraction-bootstrapped-on-egui.md) | CanaryUI abstraction and bootstrap backend |
| [0012](0012-project-state-as-a-versionable-graph.md) | Project state as a versionable graph |
| [0013](0013-live-collaboration-server-authoritative-topology.md) | Server-authoritative live-collaboration topology |
| [0014](0014-change-detection-as-shared-primitive.md) | Change detection as a shared primitive |
| [0015](0015-localization-format-and-key-mechanism.md) | Localization format and key mechanism |
| [0016](0016-native-rendering-backends.md) | Native rendering backends |
| [0017](0017-unified-transform-representation.md) | Unified transform representation |
| [0018](0018-asset-handles-and-synchronous-loading.md) | Asset handles and synchronous loading |
| [0019](0019-physics-backend-lineup.md) | Physics backend lineup |
| [0020](0020-pre-v0-3-architectural-locks.md) | Pre-v0.3 architectural locks |
| [0021](0021-amendments-to-pre-v0-3-locks.md) | Amendments to the pre-v0.3 locks |
| [0022](0022-constitution-clarifications-and-red-team.md) | Architectural constitution clarifications and review |
| [0023](0023-audio-bootstrap-rodio-behind-custom-trait.md) | Audio bootstrap behind a Canary-owned trait |

Planned decisions are listed as triggers in
[`docs/roadmap/future-roadmap.md`](../../roadmap/future-roadmap.md). They are
not pre-decided ADRs: write them when the milestone reaches the design work and
the alternatives can be evaluated against current code and evidence.
