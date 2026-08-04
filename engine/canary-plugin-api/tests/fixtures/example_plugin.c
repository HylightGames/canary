/* Minimal Tier B native plugin fixture -- written in C rather than Rust on
 * purpose: it exists to prove that Canary's native plugin ABI really is a
 * C ABI, not an accidentally-Rust-shaped one that only happens to work
 * because both sides are rustc. See
 * ../../../../docs/architecture/plugin-system.md#tier-b--trusted-native-c-abi
 * and ../native_loader.rs, which compiles this file with `cc` and loads it
 * via `NativePluginLoader` at test time.
 *
 * The struct layout below must match `canary_plugin_api::abi::PluginVTable`
 * / `PluginHandle` field-for-field (see ../../src/abi.rs) -- that's the
 * entire point: two independently-compiled pieces of code, in two
 * different languages, agreeing on a plain C struct layout instead of on
 * anything language-specific. `CANARY_ABI_VERSION` must match
 * `canary_plugin_api::abi::ABI_VERSION`.
 */

#include <stddef.h>

#define CANARY_ABI_VERSION 1u

typedef struct {
    const char* (*name)(void* context);
    void (*on_load)(void* context);
    void (*on_unload)(void* context);
    void (*destroy)(void* context);
    const void* (*get_extension)(void* context, const char* name);
} CanaryPluginVTable;

typedef struct {
    unsigned int abi_version;
    void* context;
    CanaryPluginVTable vtable;
} CanaryPluginHandle;

static const char* example_name(void* context) {
    (void)context;
    return "example-native-plugin (C)";
}

static void example_on_load(void* context) {
    (void)context;
}

static void example_on_unload(void* context) {
    (void)context;
}

static void example_destroy(void* context) {
    (void)context;
}

static const void* example_get_extension(void* context, const char* name) {
    (void)context;
    (void)name;
    return NULL; /* no extensions defined yet -- see ADR 0009 */
}

/* The required export (see canary_plugin_api::abi::ENTRY_SYMBOL). Fills
 * `*out` rather than returning a struct by value, matching the Rust side's
 * `PluginEntryFn` signature. Every field must be set -- the host reads
 * `abi_version` before trusting anything else. */
void canary_plugin_entry(CanaryPluginHandle* out) {
    out->abi_version = CANARY_ABI_VERSION;
    out->context = NULL;
    out->vtable.name = example_name;
    out->vtable.on_load = example_on_load;
    out->vtable.on_unload = example_on_unload;
    out->vtable.destroy = example_destroy;
    out->vtable.get_extension = example_get_extension;
}
