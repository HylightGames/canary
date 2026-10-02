;; `collectathon` Tier A guest: reads the game world's registered `Score`
;; component through `canary:plugin/ecs-read` on `on-load`.
;;
;; On load the guest caches the world entity count, probes handle `(0, 0)`
;; (valid on a freshly spawned world — generations start at zero), and scans
;; every live entity for the registered `collectathon.score@1` schema via
;; `has-component`, caching the first entity index that carries it (or -1).
;; The host observes the calls' success through the scoped `on-load` outcome
;; plus its own world state after reclaim; the cached probes exist so a
;; host-side test can observe values that genuinely round-tripped through a
;; real component instance.
;;
;; Signature notes: `entity-count` is `() -> u32`; `is-valid-entity` takes
;; the `entity-handle` record `(u32, u64)`; `has-component` appends the
;; `schema-id` string. `bool` crosses as `i32`. Import instance types may
;; only mention outer named types, and records/strings in signatures must
;; resolve through an exported (`eq`) type — so the component declares the
;; handle once, aliases it into the instance type, re-exports it, and the
;; function signatures reference the export (the `wit-component` output
;; shape). The string bytes live in a dedicated memory module instantiated
;; *before* the lowering (so `canon lower` can name the memory string
;; arguments are read from); the guest core module itself never touches
;; memory — it passes the static address and length as constants. No
;; per-frame hooks: `on-load` reads, `on-unload` is a no-op —
;; `PluginPhase::OnLoad`/`OnUnload` only.
(component
  ;; Component-level type the import signatures alias inward: import
  ;; instance types may only mention outer named types, and the instance
  ;; shape itself is declared once as a component-level instance type (the
  ;; `wit-component` output shape), not inline in the import.
  (type $entity-handle
    (record (field "index" u32) (field "generation" u64)))
  (type $ecs-read-instance (instance
    (alias outer 1 $entity-handle (type $imported-handle))
    (export "entity-handle" (type (eq 0)))
    (export "entity-count" (func (result u32)))
    (export "is-valid-entity"
      (func (param "entity" 1) (result bool)))
    (export "has-component"
      (func (param "entity" 1)
            (param "schema-id" string)
            (result bool)))
  ))
  (import "canary:plugin/ecs-read@0.1.0"
    (instance $ecs-read-import (type $ecs-read-instance)))

  ;; Guest-visible memory holding the probed schema id
  ;; ("collectathon.score@1" is 20 bytes, stored at offset 0). Instantiated
  ;; with no imports so the `has-component` lowering below can name it.
  (core module $memory-module
    (memory (export "memory") 1)
    (data (i32.const 0) "collectathon.score@1")
  )
  (core instance $memory-instance (instantiate $memory-module))

  (core module $guest
    (import "host" "entity-count" (func $entity_count (result i32)))
    (import "host" "is-valid-entity" (func $is_valid_entity (param i32 i64) (result i32)))
    (import "host" "has-component" (func $has_component (param i32 i64 i32 i32) (result i32)))
    (global $cached-count (mut i32) (i32.const 0))
    (global $cached-first-valid (mut i32) (i32.const 0))
    (global $cached-score-index (mut i32) (i32.const -1))
    (func (export "on-load")
      (local $i i32)
      (global.set $cached-count (call $entity_count))
      (global.set $cached-first-valid (call $is_valid_entity (i32.const 0) (i64.const 0)))
      (local.set $i (i32.const 0))
      (block $done
        (loop $scan
          (br_if $done (i32.ge_u (local.get $i) (global.get $cached-count)))
          (if (i32.and
                (call $is_valid_entity (local.get $i) (i64.const 0))
                (call $has_component (local.get $i) (i64.const 0) (i32.const 0) (i32.const 20)))
            (then
              (global.set $cached-score-index (local.get $i))
              (br $done)))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $scan))))
    (func (export "on-unload"))
    (func (export "last-entity-count") (result i32)
      (global.get $cached-count))
    (func (export "first-entity-valid") (result i32)
      (global.get $cached-first-valid))
    (func (export "score-entity-index") (result i32)
      (global.get $cached-score-index))
  )

  (core func $entity_count_lowered
    (canon lower (func $ecs-read-import "entity-count")))
  (core func $is_valid_entity_lowered
    (canon lower (func $ecs-read-import "is-valid-entity")))
  (core func $has_component_lowered
    (canon lower (func $ecs-read-import "has-component") (memory (core memory $memory-instance "memory"))))

  (core instance $guest-instance (instantiate $guest
    (with "host" (instance
      (export "entity-count" (func $entity_count_lowered))
      (export "is-valid-entity" (func $is_valid_entity_lowered))
      (export "has-component" (func $has_component_lowered))
    ))
  ))

  (func $on_load_lifted (canon lift (core func $guest-instance "on-load")))
  (func $on_unload_lifted (canon lift (core func $guest-instance "on-unload")))
  (func $last_entity_count_lifted (result u32)
    (canon lift (core func $guest-instance "last-entity-count")))
  (func $first_entity_valid_lifted (result u32)
    (canon lift (core func $guest-instance "first-entity-valid")))
  (func $score_entity_index_lifted (result s32)
    (canon lift (core func $guest-instance "score-entity-index")))

  (instance $lifecycle_export
    (export "on-load" (func $on_load_lifted))
    (export "on-unload" (func $on_unload_lifted))
    (export "last-entity-count" (func $last_entity_count_lifted))
  )
  (export "canary:plugin/lifecycle@0.1.0" (instance $lifecycle_export))
  (instance $probe_export
    (export "first-entity-valid" (func $first_entity_valid_lifted))
    (export "score-entity-index" (func $score_entity_index_lifted))
  )
  (export "collectathon:guest/probe@0.1.0" (instance $probe_export))
)
