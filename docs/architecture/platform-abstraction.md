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

v0.0.1 shipped only the **trait definitions** (`Window`, `InputSource`)
plus a `NullWindow`/headless implementation used by `canary-runtime` and
by tests. It deliberately did **not** add `winit` (or any real
windowing/graphics dependency) yet.

**The reason given at the time — since corrected, not carried forward
unverified.** `v0.0.1`'s reasoning was that its sandbox had "no display
server, GPU drivers, or the system libraries... a real windowing
dependency would need," making the dependency impossible to build or
test there. Checked directly while scoping `v0.0.4`, rather than
assumed still true: this sandbox has `Xvfb` (a virtual X server)
already installed, `libx11-dev` already installed, and installable
Wayland/software-Vulkan (`llvmpipe`/`lavapipe`) support — and a real
`winit` `0.30.13` window was created, driven through several redraw
cycles, and closed cleanly against `Xvfb` as part of that scoping work.
See [`docs/roadmap/v0.0.4-roadmap.md`](../roadmap/v0.0.4-roadmap.md) for
the full verification and what it does and doesn't prove. The
*implementation* is still `v0.0.4`'s job, not done yet — only the
"cannot actually build or test" premise is what's now known to be
false, in this sandbox at least; a different environment implementing
that release should still confirm the same packages/behavior rather
than assume this note travels with it.
