# `spinning-cube`

A rotating cube, rendered offscreen through the real RHI
(`canary-render` + `canary-render-vulkan`) and encoded to an animated
GIF:

![A spinning, colored cube](../../misc/screenshots/spinning_cube.gif)

This is the example [`../README.md`](../README.md) named, back in
`v0.0.1`, as the first thing that would belong here once rendering
existed. It's a demo, not a test — `canary-render-vulkan`'s own
`hello_triangle` integration test is what actually asserts on rendered
pixel values; this exists to look at.

## Run it

```sh
cargo run -p spinning-cube -- path/to/output.gif
```

Needs a real (if software) Vulkan ICD — this repository's own CI job
installs `mesa-vulkan-drivers` for exactly this reason; see
[`docs/architecture/platform-abstraction.md`](../../docs/architecture/platform-abstraction.md).
The output path defaults to `spinning_cube.gif` in the current directory
if omitted.

## What it demonstrates, and what it deliberately doesn't

Everything about *why* this is built the way it is — CPU-side rotation
and projection, a hand-rolled painter's algorithm instead of a depth
buffer, reusing `hello_triangle`'s own already-proven shader unmodified
— is explained in `src/main.rs`'s own module docs rather than
duplicated here. Short version: the RHI is deliberately minimal as of
`v0.0.6` (no uniforms, no depth buffer, no culling), so this example
does the honest thing given that scope instead of pretending otherwise.

Real rendering-engine features this example uses to build something
without needing engine changes:

- The same WGSL → SPIR-V compilation path (`naga`) as `canary-render-vulkan`'s
  own test.
- `RenderDevice::create_buffer` re-created once per frame — the RHI has
  "no story for updating a buffer's content after creation" yet, so a
  fresh buffer per frame is the correct approach given today's trait,
  not a workaround.
- The same offscreen color target reused across all 36 frames — only
  the vertex data changes per frame, so there's no need to recreate it.
