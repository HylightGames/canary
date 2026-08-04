# Platform Abstraction

`canary-platform` is Layer 1 in [engine-overview.md](engine-overview.md) —
the only layer allowed to know it's running on Windows, macOS, Linux, or
(eventually) a console or mobile OS. Everything above it programs against
traits, never against `#[cfg(target_os = ...)]` scattered through gameplay
or subsystem code.

## What it abstracts

- **Windowing & surface creation** — opening a window (or, headless, not
  opening one) and handing the renderer a drawable surface handle.
- **Input** — keyboard, mouse, gamepad, and (later) touch, normalized into
  engine-defined event types rather than leaking OS-specific input codes
  upward.
- **Filesystem** — path handling and file I/O behind a trait, so packaged/
  sealed asset reads (see [asset-system.md](asset-system.md)) and loose-file
  dev-mode reads can share one call site in engine code.
- **Threads & time** — thread spawning primitives for the future job system
  (see [core-runtime.md](core-runtime.md#threading--the-job-system)) and a
  monotonic clock abstraction, since "what clock source is safe to use for
  fixed-timestep simulation" is quietly platform-specific.

## Why this is a real trait boundary and not just "we use `winit`"

The engine core, ECS, and every Layer 3 subsystem depend on
`canary-platform`'s **traits** (`Window`, `InputSource`, ...), never on a
specific windowing crate directly. This matters for three concrete,
non-hypothetical reasons:

1. **Headless operation.** A dedicated multiplayer server (see
   [networking.md](networking.md)) needs the ECS, physics, and networking
   subsystems to run with no window at all. If those subsystems depended on
   a concrete windowing crate rather than a trait, "headless" would require
   conditional compilation threaded through all of them instead of simply
   selecting a `NullWindow` implementation of the same trait.
2. **Testing.** Unit and integration tests need to exercise engine logic
   without a real display server (and CI runners are typically headless
   anyway) — the same `NullWindow`/headless implementation used for servers
   serves this need for free.
3. **Future platform targets.** Consoles and mobile platforms have
   fundamentally different windowing/input models; a trait boundary is what
   makes adding a new platform "implement the trait" rather than "audit
   every subsystem for OS assumptions."

## Chosen default backend (planned, not yet implemented)

For the real (non-headless) desktop backend, `winit` is the intended default
implementation of the windowing/input traits — it's the de facto standard
cross-platform windowing crate in the Rust ecosystem and is what `wgpu`'s
own examples and most Rust engines (Bevy included) integrate with, which
matters given [rendering.md](rendering.md)'s choice of `wgpu` as the initial
RHI backend. This is a planned decision, not yet implemented — see below.

## Status in this foundation

v0.0.1 ships only the **trait definitions** (`Window`, `InputSource`)
plus a `NullWindow`/headless implementation used by
`canary-runtime` and by tests. It deliberately does **not** add `winit` (or
any real windowing/graphics dependency) yet, for a concrete, practical
reason specific to this session: this foundation was built and validated in
a headless Linux container without a display server, GPU drivers, or the
system libraries (X11/Wayland, Vulkan loader) that a real windowing
dependency would need — pulling one in now would add a dependency this
environment cannot actually build or test, for a feature (opening a window)
that has no consumer yet anyway (there's no renderer to draw into it). Real
windowing is scoped as Era 2 follow-up work (see
[`docs/roadmap/v0.0.1-roadmap.md`](../roadmap/v0.0.1-roadmap.md)), to be
implemented and tested in an environment that actually has a display/GPU
stack to validate against.
