# Examples

Empty for now, deliberately. A meaningful example (even "spinning cube")
needs a renderer and windowing backend, neither of which exist yet in
`v0.0.1` — see
[`docs/architecture/rendering.md`](../docs/architecture/rendering.md) and
[`docs/architecture/platform-abstraction.md`](../docs/architecture/platform-abstraction.md).

What belongs here once it's buildable:

- A headless ECS-only example (`examples/ecs_sandbox/` or similar) is
  actually possible *before* rendering lands, and is a reasonable first
  addition once `canary-ecs` grows past its current placeholder — see
  [`docs/roadmap/v0.0.1-roadmap.md`](../docs/roadmap/v0.0.1-roadmap.md).
- Once the RHI/render graph exist (Era 3,
  [`docs/vision/long-term-roadmap.md`](../docs/vision/long-term-roadmap.md)),
  a minimal windowed example belongs here.
- Each example should be a runnable binary (`cargo run --example <name>` or
  its own crate under this directory, depending on how large it gets — see
  [`docs/development/repository-structure.md`](../docs/development/repository-structure.md)
  if this directory needs its own workspace membership later) with a short
  `README.md` of its own explaining what it demonstrates.
