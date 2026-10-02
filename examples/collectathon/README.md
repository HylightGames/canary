# Collectathon

The `v0.1.0` integration-proof sample: a one-room 2D collectathon played
against Canary's documented public APIs only. One physics-driven player
moves on mapped digital actions, collects three authored shards, reaches
the goal, and shows score/goal status in a `CanaryUI` HUD. One HUD button
sends game intent through the documented simulation boundary; pickup state
triggers a sound loaded from disk.

Skeleton status (WP2): playable game logic is in — physics-driven
movement on mapped actions, touch plus collect-edge pickup collection
with a pickup voice, HUD score/goal with a reset button, asset loading
into stores, windowed presentation with audio-device bring-up (headless
fallback), and the deterministic headless twin. WP3 adds the
multi-process harnesses: the authoritative replication server/client, the
two-client collaboration CLI, the Tier A guest probe, and the snapshot
determinism proof.

## Setup

From a fresh checkout, with the `stable` toolchain from `rust-toolchain.toml`:

```sh
cargo build -p collectathon
```

## Launch

| Binary | Command | Role |
| --- | --- | --- |
| Windowed game | `cargo run -p collectathon --bin collectathon` | Full game |
| Headless twin | `cargo run -p collectathon --bin collectathon_headless` | Deterministic scripted run |
| Server | `cargo run -p collectathon --bin collectathon_server` | Authoritative replication (QUIC handshake → snapshot → delta → input → ack) |
| Client | `cargo run -p collectathon --bin collectathon_client -- --server ADDR --cert CERT` | Replication client (converge, frame-tagged input, ack) |
| Collab CLI | `cargo run -p collectathon --bin collectathon_collab_client` | Shared-authored-edit CLI (accept/deny/conflict/sync over `CollabSessionHost`) |

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
loaded `tone-mono-8k.wav` on each collection. Audio opens the OS default
device when one exists and plays decode-only headless otherwise.

## Asset licenses

See `assets/LICENSES.md`. Mesh/texture/audio fixtures are byte copies of
the hand-generated `canary-assets` fixtures; `room.json` and `guest.wat`
are original to this example. Everything is MIT.

## Live-evidence checklist (WP3 paths run; WP4 owns the platform matrix)

- [ ] Windowed playthrough on <platform>: command, expected result, evidence.
- [ ] Headless deterministic run: command, expected result, evidence.
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
- [ ] Audio-device playback on <platform>: command, expected result, evidence.
