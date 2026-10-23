# Examples

## `spinning-cube`

A rotating cube rendered offscreen through the real RHI
(`canary-render` + `canary-render-vulkan`), encoded to an animated GIF —
see [`spinning-cube/README.md`](spinning-cube/README.md). This is the
example this file itself named, back in `v0.0.1`, as the first thing
that would belong here once rendering existed ("a meaningful example
(even 'spinning cube')..." — see this file's own history); it stayed a
placeholder until `v0.0.6`'s rendering bootstrap made it possible.

```sh
cargo run -p spinning-cube -- path/to/output.gif
```

## `ui-game`

The `v0.0.13` first-game slice in a real window: mapped-action movement,
fire on Space or the HUD button, and a readout HUD over the scene — see
[`ui-game/README.md`](ui-game/README.md). Needs a real Vulkan ICD and a
display server (that README covers the Wayland note).

```sh
cargo run -p ui-game -- 600
```

## `collectathon`

The `v0.1.0` integration-proof sample: a small playable 2D game built
only on Canary's public APIs, with a deterministic headless twin, the
authoritative server/client and collaboration CLIs, and a live-evidence
checklist — see [`collectathon/README.md`](collectathon/README.md).
Start with the [Setup](collectathon/README.md#setup) section; the
git/path dependency route is documented under
[Depending on Canary from your own crate](collectathon/README.md#depending-on-canary-from-your-own-crate-gitpath-route).

```sh
cargo run -p collectathon --bin collectathon
```

Each example should be a runnable binary (`cargo run --example <name>`
or its own crate under this directory, depending on how large it gets —
see
[`docs/development/repository-structure.md`](../docs/development/repository-structure.md))
with a short `README.md` of its own explaining what it demonstrates.
