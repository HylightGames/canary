/* Deliberately declares an ABI version this host does not support, to
 * prove `NativePluginLoader::load` actually rejects the mismatch (see
 * ../native_loader.rs) rather than the version check merely existing in
 * theory. Otherwise identical to example_plugin.c. See ADR 0009
 * (docs/decisions/architecture-decision-records/0009-plugin-abi-versioning-and-extensibility.md).
 */

#include <stddef.h>

/* Intentionally wrong: canary_plugin_api::abi::ABI_VERSION is 1. */
#define CANARY_WRONG_ABI_VERSION 999u

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

static const char* wrong_version_name(void* context) {
    (void)context;
    return "wrong-abi-version-plugin (C)";
}

static void wrong_version_noop(void* context) {
    (void)context;
}

static const void* wrong_version_get_extension(void* context, const char* name) {
    (void)context;
    (void)name;
    return NULL;
}

void canary_plugin_entry(CanaryPluginHandle* out) {
    out->abi_version = CANARY_WRONG_ABI_VERSION;
    out->context = NULL;
    out->vtable.name = wrong_version_name;
    out->vtable.on_load = wrong_version_noop;
    out->vtable.on_unload = wrong_version_noop;
    out->vtable.destroy = wrong_version_noop;
    out->vtable.get_extension = wrong_version_get_extension;
}
