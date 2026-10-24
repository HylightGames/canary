# ADR 0030: Trusted Window-Provider Contract for the Vulkan Surface Seam

Status: Accepted
Date: 2026-10-24

## Context

ADR 0029 closed the first round of Vulkan ownership gaps (`DeviceId`
provenance, `WrongWindow` refusal, fail-fast device-drop, documented
fatal RHI creation). The follow-up re-review found two remaining holes
in the provider boundary itself (`canary-platform/src/surface.rs`,
`canary-render-vulkan/src/surface.rs`, `presenter.rs`):

1. The safety requirement was unenforced by the public API.
   `SurfaceHandlesProvider` was a safe-to-implement trait, and the
   Vulkan backend passed the returned handles into
   `ash_window::create_surface` — an `unsafe` FFI call whose
   precondition is that both handles genuinely refer to live OS
   objects. A safe external implementation could supply fabricated
   handle values, turning that precondition violation into undefined
   behavior behind a safe API.
2. The liveness token could outlive its window.
   `liveness_lease()` returned strong `WindowLease` clones, and the
   presenter only downgraded its own copy. A caller retaining an extra
   strong clone kept the token alive after the owning window dropped,
   bypassing the intended `WindowGone` refusal.

## Decision

- `SurfaceHandlesProvider` is now an `unsafe trait` with a documented
  safety section: implementors must return handles that genuinely
  refer to the implementor's own live native window. Only trusted
  platform backends in `canary-platform` (today just the `winit`
  backend's window) implement it; test stubs opt in explicitly
  (`unsafe impl` plus a `SAFETY:` comment). `HeadlessWindow` still
  does not implement it, so the headless/windowed misuse stays a
  compile error.
- `liveness_lease()` now returns `WindowLeaseWeak` instead of a strong
  `WindowLease`. The owning window keeps the sole strong half, so
  downstream code observes death but cannot extend the token's life
  past the window by retaining its copy. Making the invalid state
  unrepresentable through safe public APIs, rather than relying only
  on the presenter's runtime check.
- A borrow-checker lifetime on the window is still rejected: `&mut`
  event polling conflicts with a stored shared borrow, so the
  per-frame `WindowGone`/`WrongWindow` checks remain runtime
  enforcement — now over a token that cannot be artificially
  prolonged.

## Consequences

- Fabricating a window provider is an explicit `unsafe impl` with an
  auditable `SAFETY:` comment; the audit surface is the (tiny)
  implementor set, not every caller.
- Negative tests pin both halves: a platform unit test proves retained
  (even cloned) weak halves die with the owner, and the
  windowed-presentation proof retains a liveness copy across the
  window drop, asserts it reads dead, and asserts the presenter
  refuses with `WindowGone`.
- Extends (does not rewrite) ADR 0029's "Window provenance" bullet;
  0029's deferred items (shared native teardown, typed `Result`
  creation) are unchanged.
