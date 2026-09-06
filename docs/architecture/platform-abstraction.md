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

## Chosen default backend

For the real (non-headless) desktop backend, `winit` is the default
implementation of the windowing/input traits (see [`WinitWindow`/`WinitInput`](../../engine/canary-platform/src/winit_backend.rs),
behind the `winit-backend` Cargo feature) — it's the de facto standard
cross-platform windowing crate in the Rust ecosystem. Implemented in
`v0.0.4`; see "Status in this foundation" below.

## Status in this foundation

`v0.0.1` shipped only the **trait definitions** (`Window`, `InputSource`)
plus a `HeadlessWindow`/`HeadlessInput` implementation used by
`canary-runtime` and by tests. It deliberately did **not** add `winit`
(or any real windowing/graphics dependency) yet, reasoning at the time
that its sandbox lacked the display server, GPU drivers, and system
libraries a real windowing dependency would need.

**That reasoning turned out to be wrong, checked directly rather than
carried forward unverified — and `v0.0.4` has since implemented the real
backend it was blocking.** This sandbox has `Xvfb` (a virtual X server),
`libx11-dev`, and the rest of the X11/Wayland development libraries
already installed or installable, and a real `winit 0.30.13` window was
created, driven through real poll cycles, gracefully closed via a genuine
OS-level signal, and fed real synthesized keyboard input — all
end-to-end against `Xvfb`, not mocked. Concretely, `v0.0.4` added:

- **`WinitWindow`/`WinitInput`**, alongside (not replacing)
  `HeadlessWindow`/`HeadlessInput`, behind a `winit-backend` Cargo
  feature that's off by default — the same "not privileged, not in the
  build unless used" pattern this project applies to physics and
  rendering backends (`physics.md`, [ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md)),
  applied here so headless-only consumers (dedicated servers, most
  tests) never pull in `winit`'s X11/Wayland/Win32/AppKit dependency
  tree. Confirmed mechanically, not just claimed: `cargo tree` without
  the feature shows no `winit`.
- **`Key` expanded** from a 3-variant placeholder to a realistic
  keyboard (letters, digits, function keys, modifiers with left/right
  distinguished, navigation, editing keys, and US-layout punctuation),
  modeled on physical key position (matching `winit::keyboard::KeyCode`)
  rather than the character produced, so WASD-style bindings stay under
  the same fingers regardless of keyboard layout.
- **A real, `#[ignore]`d-by-default integration test**
  (`engine/canary-platform/tests/winit_backend_window.rs`) proving all
  of the above against a live `Xvfb` display: window creation, several
  `poll_events()` cycles, a real keyboard press synthesized via the X11
  XTEST extension and correctly observed as `KeyPressed(Key::W)`, and a
  real ICCCM `WM_DELETE_WINDOW` `ClientMessage` correctly flipping
  `should_close()`. Run automatically in CI's Linux-only
  `windowing-integration` job (`.github/workflows/ci.yml`), not just
  documented as "you can run this manually."

**Two real `winit` constraints found the direct way, not assumed, while
building that test:** `winit::event_loop::EventLoop::new()` panics if
called off the process's actual main thread (which is where every
`cargo test` function runs by default) — worked around with a
Linux-only, explicitly test-only `WinitWindow::new_for_testing`
constructor, kept separate from the real `WinitWindow::new()` so
shipped game code never silently gets the same treatment. And `winit`
allows creating only **one** `EventLoop` per process, ever — including
across separate `#[test]` functions in the same test binary, since
`cargo test`'s default harness runs each test on its own thread within
one shared process, not a separate process per test. Both are documented
in `winit_backend.rs`'s and the integration test's own module docs.

**The `winit`/Wayland dependency pin set had drifted since it was first
verified while scoping `v0.0.4`, confirmed by re-running the same
verification at implementation time rather than trusting the earlier
pass:** two additional pins are now needed beyond the three originally
found (`wayland-protocols = "=0.32.9"`, `quick-xml = "=0.39.4"`,
`wayland-scanner = "=0.31.10"`) —
`wayland-protocols-plasma = "=0.3.9"` and
`wayland-protocols-wlr = "=0.3.9"`, each because their own newer
releases hard-require a `wayland-protocols` version above the pin
above. All five are documented, with the reasoning, in
[`build-system.md`](../development/build-system.md#the-rustc-175-sandbox-validation-floor).

**Extended further while scoping `v0.0.6`'s rendering bootstrap:** not
just "software Vulkan support is installable" but confirmed working —
installing `mesa-vulkan-drivers` and running `vulkaninfo --summary`
enumerated a real `PHYSICAL_DEVICE_TYPE_CPU` device (`llvmpipe`, Vulkan
API 1.4.318). This means a Vulkan RHI backend's *offscreen* rendering
(instance/device creation, buffers, pipelines, a real draw call) is
genuinely testable in this sandbox without real GPU hardware — see
[ADR 0016](../decisions/architecture-decision-records/0016-native-rendering-backends.md)
and [`v0.0.6-roadmap.md`](../roadmap/v0.0.6-roadmap.md). A *windowed*
Vulkan surface would additionally need the `Xvfb` setup already
confirmed above and real `WinitWindow` surface presentation, which
`v0.0.6`'s own scope explicitly defers — see that roadmap's "Not
blocked on `v0.0.4`" section.
