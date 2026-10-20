# Collectathon

The `v0.1.0` integration-proof sample: a one-room 2D collectathon played
against Canary's documented public APIs only. One physics-driven player
moves on mapped digital actions, collects three authored shards, reaches
the goal, and shows score/goal status in a `CanaryUI` HUD. One HUD button
sends game intent through the documented simulation boundary; pickup state
triggers a sound loaded from disk.

Skeleton status (WP2–WP3 done, WP4 in progress): playable game logic is in — physics-driven
movement on mapped actions, touch plus collect-edge pickup collection
with a pickup voice, HUD score/goal with a reset button, asset loading
into stores, windowed presentation with audio-device bring-up (headless
fallback), and the deterministic headless twin. The multi-process
harnesses are also in: the authoritative replication server/client, the
two-client collaboration CLI, the Tier A guest probe, and the snapshot
determinism proof (checklist below). What remains is WP4: the
live-device evidence rows (platform/command/result) need a real machine
run before `v0.1.0` can be cut — `v0.1.0` itself is NOT released.

## Setup

From a fresh checkout, with the `stable` toolchain from `rust-toolchain.toml`:

```sh
cargo build -p collectathon
```

### Linux native packages

Compiling the windowed game needs the same system libraries CI
installs — the Rust toolchain alone is not enough on a fresh Linux
checkout. All names below are the exact `apt` package names from
`.github/workflows/ci.yml` (build leg at `ci.yml:75`; Xvfb/llvmpipe
legs at `ci.yml:122,154,159,210`):

```sh
# Required to build the windowed game (winit X11/Wayland backends +
# cpal ALSA audio probe, same set as CI's lint-and-test job):
sudo apt-get update
sudo apt-get install -y libwayland-dev libxkbcommon-dev libxrandr-dev libxi-dev libxcursor-dev pkg-config libasound2-dev

# Only needed to reproduce the Xvfb / software-Vulkan CI legs locally
# (windowing-integration + rendering-integration jobs):
sudo apt-get install -y libxkbcommon-x11-0 xvfb mesa-vulkan-drivers
```

macOS and Windows need no `apt` step (their backends ship with the
OS/toolkit SDKs). `scripts/setup-check.sh` reports which of the above
are missing as `[warn]` rows — warnings only, never failures.

### Depending on Canary from your own crate (git/path route)

Canary is pre-`1.0` and `canary-runtime` is `publish = false`, so
there is currently no supported `crates.io` consumer route — do not
expect `cargo add canary-runtime` to resolve. The supported route is
a workspace-external crate depending through `git` (or a local
`path` checkout), pinning the same way this sample does in
`examples/collectathon/Cargo.toml`:

```toml
[dependencies]
canary-runtime = { git = "<this repository>", rev = "<pinned commit>" }
canary-ecs = { git = "<this repository>", rev = "<pinned commit>" }
# ... add only the `canary-*` crates your game uses; there is no
# facade or prelude crate, so each dependency is selected explicitly.
```

Replace `rev` with the `dev` commit you tested against. There is no
facade crate (deliberately — speculative surface is out of scope).

### Branch and tag policy

Work happens on `dev`; `stable` only receives reviewed release merges
(`feature/<name> --PR--> dev --release--> stable`), per
`docs/development/git-workflow.md`. Tags (`vX.Y.Z`) mark the release
commit and trigger `docs/release-notes/<tag>.md` publication via
`.github/workflows/release.yml`. `v0.1.0` itself is NOT released —
this note states the documented policy without claiming any tag
exists.

## First-game guide (dependency selection + boot order)

Starting from this sample when writing your own game:

1. **Select dependencies explicitly.** Copy the `canary-*` entries you
   need from `Cargo.toml` — not the whole list by default. Minimal
   headless simulation needs `canary-ecs`, `canary-scheduler`,
   `canary-runtime`, and `canary-state`; add `canary-physics` /
   `canary-transform` for movement, `canary-assets` / `canary-audio`
   for files and sound, `canary-platform` (`winit-backend`) +
   `canary-render-vulkan` (`presentation`) + `canary-ui-*` only for
   the windowed game. No facade re-exports these for you.
2. **Boot in this order** (see `src/bin/collectathon_headless.rs` for
   the headless spelling; the windowed `src/bin/collectathon.rs`
   follows the same order with a live window, Vulkan presentation,
   and audio-device bring-up):
   1. `assets` — `load_game_assets()` (mesh/texture/sound stores)
      first, so handles exist before anything references them.
   2. `room` — `load_room()` the authored `room.json`, then
      `AuthoredSpawner::new(&CollectathonDecoder,
      &resolve_asset).spawn(&mut world, &room)` to stage typed
      inserts.
   3. `world` — `register_game_components()`,
      `attach_simulation_components()` (wiring the loaded sound
      handle in), plus `world.insert_resource(sounds)`.
   4. `schedule` — `Schedule::new()` then register in this order:
      `register_physics_step`, `register_transform_propagation`,
      `register_gameplay(&actions)`, `register_audio_trigger`.
   5. `presentation` — `RuntimeBuilder::new().build(world)` plus the
      frame driver (`FrameDriver::new(backend, mapper)`; `NullBackend`
      headless, live Vulkan presenter windowed). One `drive_frame`
      per outer frame with a `FrameDelta` bridge for the physics
      step. Window close is the exit condition; a missing audio
      device logs `no audio device … continuing headless` and plays
      decode-only — a supported configuration, not a failure.

## Launch

| Binary | Command | Role |
| --- | --- | --- |
| Windowed game | `cargo run -p collectathon --bin collectathon` (append ` -- 600` for a 600-frame scripted run) | Full game |
| Headless twin | `cargo run -p collectathon --bin collectathon_headless` | Deterministic scripted run |
| Server | `cargo run -p collectathon --bin collectathon_server` (options: `--bind 127.0.0.1:0`, `--identity-dir DIR`; `--help` prints usage) | Authoritative replication (QUIC handshake → snapshot → delta → input → ack) |
| Client | `cargo run -p collectathon --bin collectathon_client -- --server ADDR --cert CERT` | Replication client (converge, frame-tagged input, ack) |
| Collab CLI | `cargo run -p collectathon --bin collectathon_collab_client` (options: `--project PATH`, `--permissions PATH`; `--help` prints usage) | Shared-authored-edit CLI (accept/deny/conflict/sync over `CollabSessionHost`) |

## Controls

- WASD or arrow keys: move (physics velocity, solver-integrated).
- Space or primary pointer: collect (gather nearby pickups at extended range).
- R or the HUD Reset button: restore every pickup and zero the score.
- Close the window: exit.

## Expected interaction

The player starts at the room center with score 0. Touching a shard (or
catching it in a collect press) collects it, arms its pickup voice, and
banks one point; collecting all three shards plus the goal completes the
room (`Goal: done`). The HUD shows `Score / Collected / Goal / Player`
trailing the scene by one frame, and the pickup sound plays from the
loaded `pickup.wav` on each collection. Audio opens the OS default
device when one exists and plays decode-only headless otherwise.

## Asset licenses

See `assets/LICENSES.md`. Mesh/texture fixtures are byte copies of the
hand-generated `canary-assets` fixtures; `pickup.wav` was generated for
this example (CC0-1.0); `room.json` and `guest.wat` are original to this
example. Everything else is MIT.

## Live-evidence checklist (WP3 paths run; WP4 owns the platform matrix)

Each item maps to the [`v0.1.0-plan.md`](../../docs/roadmap/v0.1.0-plan.md#v010--integration-proof)
exit evidence: a developer on the documented consumer path can build and
play the sample without private harness APIs (setup/launch/controls
above); the per-subsystem checklist (physics, rendering, assets, audio,
UI, state, networking, collaboration, plugins) is demonstrated in that
game (checked items below); CI covers the deterministic checks while the
unchecked items are the required live-device evidence, named per
platform rather than implied verified.

- [ ] Windowed playthrough on <platform>: `cargo run -p collectathon --bin collectathon` (or append ` -- 600` for a bounded run); expected: live window plays (move/collect/reset per Controls, HUD trails scene by one frame, pickup sound on collection); evidence: <to record>.
- [ ] Headless deterministic run: `cargo run -p collectathon --bin collectathon_headless`; expected: `collectathon_headless: score … collected …/… player (…)` after 90 scripted frames (hold-left from frame 0, collect edge frame 33, release frame 60; decode-only audio); evidence: <to record>.
- [x] Server/client replication run: the server prints `READY <addr>
  <identity-dir>`, serves one session (handshake → snapshot → delta →
  input → ack), and exits after a clean disconnect; the client converges
  on the snapshot, applies the delta, submits frame-tagged input, and
  awaits the ack. No prediction or rollback on this path (ADR 0027).
  ```sh
  cargo run -p collectathon --bin collectathon_server
  # READY 127.0.0.1:<port> /tmp/collectathon-server-<pid>
  cargo run -p collectathon --bin collectathon_client \
    --server 127.0.0.1:<port> --cert /tmp/collectathon-server-<pid>/server.der
  # snapshot converged on 6 entries across 5 entities
  # delta applied — 5 changes, 1 removals, 5 live keys
  # converged score=Some(1) … / ack seq=1 applied_tick=3 / clean disconnect
  ```
  The authoritative tick between snapshot and delta collects one shard
  (banking one point, visible as `score=Some(1)`) and destroys one pickup
  through the tombstone log.
- [x] Two-client collaboration run: owner accept, reader deny, stale
  conflict with refresh, rebased accept, reader sync tail — with gameplay
  schemas asserted absent from the project file.
  ```sh
  cargo run -p collectathon --bin collectathon_collab_client
  # owner edit accepted: seq=1 target_rev=1
  # reader edit denied: Forbidden (read-only role)
  # editor stale edit: RevisionConflict at rev=1 with refresh
  # editor rebased edit accepted: seq=2 target_rev=2
  # reader sync: tail of 2 ops, no snapshot (covered cursor)
  # collectathon_collab_client: two-client run converged
  ```
- [x] Tier A guest against the game world: the guest's `on-load` reads
  entity count plus the registered `collectathon.score@1` schema through
  `canary:plugin/ecs-read` under a read/write grant, the world is
  reclaimed, and the host overwrites the score through the registered
  codec and verifies it. No per-frame hooks (`OnLoad`/`OnUnload` only).
  ```sh
  cargo test -p collectathon --lib plugin
  # guest_on_load_reads_the_game_world_and_the_world_moves_home … ok
  # score_overwrite_through_the_codec_verifies … ok
  ```
- [x] Snapshot determinism: capture → restore → recapture over the game
  world is byte-stable, both into a fresh world and into the same world
  (LIFO despawn discipline).
  ```sh
  cargo test -p collectathon --lib state
  # capture_restore_recapture_is_byte_stable … ok
  ```
- [ ] Audio-device playback on <platform>: run the windowed game above on a machine with an OS default audio device; expected: `audio device opened (audible backend)` in the log and the loaded `pickup.wav` audible on each collection (silent machines log `no audio device … continuing headless` and play decode-only — a supported configuration, not a failure); evidence: <to record>.
