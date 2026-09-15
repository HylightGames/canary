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

## What else belongs here once it's buildable

- A headless ECS-only example (`examples/ecs-sandbox/` or similar),
  now genuinely possible given `v0.0.7`/`v0.0.8`'s multi-component
  queries, typed resources, and the `canary-scheduler` — no renderer or
  windowing needed for this one.
- A real *windowed* example (not just an offscreen render like
  `spinning-cube`) needs `canary-render-vulkan` to grow swapchain/window
  -surface presentation — deliberately out of scope as of `v0.0.6`; see
  that release's own roadmap doc.

Each example should be a runnable binary (`cargo run --example <name>`
or its own crate under this directory, depending on how large it gets —
see
[`docs/development/repository-structure.md`](../docs/development/repository-structure.md))
with a short `README.md` of its own explaining what it demonstrates.
