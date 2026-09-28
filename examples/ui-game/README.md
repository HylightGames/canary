# `ui-game`

The `v0.0.13` first-game slice in a real window: mapped-action movement
(WASD/arrows), fire on Space or the HUD Fire button, and a readout HUD
over the scene — driven by the public runtime library
([`Runtime::drive_frame`](../../../engine/canary-runtime/src/frame_driver.rs)),
presented through the RHI plus blit-present.

## Run it

```sh
cargo run -p ui-game -- 600
```

Needs a real Vulkan ICD and a display server — the same requirement as
`canary-render-vulkan`'s `present_clear` proof test. With no argument the
game runs until the window closes; the optional argument caps the
outer-frame count (`600` is ~10 s at 60 Hz FIFO presentation). Move with
WASD or the arrow keys, fire with Space (gameplay action edge) or by
clicking the HUD Fire button (UI intent — the same `"fire"` simulation
boundary the headless harness honors, so both consumers share the
contract). On a Wayland session the game must run with
`env -u WAYLAND_DISPLAY` so `winit` takes the X11 path; otherwise the
window is invisible to X11 automation (`xdotool`).

## What it demonstrates, and what it deliberately doesn't

- The §5 frame order end to end: pump platform events → UI capture
  before gameplay mapping → simulation pass → extract → scene plus UI
  into one offscreen target → blit-present. The UI paints into the same
  open RHI pass as the scene (scene first, widgets over it); there is no
  second graphics path.
- The §4 contract: the HUD renders an immutable view (last frame's
  extracted position and shot count — the build pass runs before the
  pass, so the readouts trail the scene by exactly one outer frame), and
  the Fire button returns an intent the simulation applies at its
  boundary. Button-to-intent is `canary-ui-egui`'s own click proof,
  intent-to-effect is this crate's `game` unit tests, and the plumbing
  between is the frame driver's — three separate proofs, no coordinates
  smuggled into automated tests.
- What it doesn't do: fixed-step accumulation, pause, remapping,
  gamepad, HiDPI font re-rasterization (glyphs rasterize at 1.0 pixels
  per point and upscale with the viewport — honest, documented in
  `src/main.rs`), or window-resize scaling (a resize re-creates the
  offscreen target; the presenter never scales silently).

## Live proof record (X11 + NVIDIA, 2026-09-27)

One 600-frame windowed run plus scripted automation, all in a single
foreground command (background jobs do not survive agent turns here):

- 600/600 frames presented, 0 skipped; `ui_batches=3` steady; no device
  loss across the run.
- Held `D` (xdotool key) drove `x` from 0 to 232.3; `x` froze after
  key-up — no stuck input.
- Space edge fired once via the gameplay action path (`shots` 0 → 1).
- Three scripted clicks on the HUD Fire button drove `shots` 0 → 1 → 2
  → 3 — exactly one intent per click. Clicks must go through the real
  input path: `xdotool`'s `XSendEvent` button events never arrive (keys
  are fine), so the clicks used `python-Xlib`'s `xtest.fake_input`.
  Because fire is bound to pointer-primary with no pass-through, the
  +1-per-click also proves the same press did not double-fire through
  both the UI and gameplay paths.
- The tiling window manager retiles mid-run (the sample sizes its
  offscreen target from `presenter.extent()` and retries the
  draw/present on `ContentExtentMismatch`, which recovered a real
  extent-race frame live).
