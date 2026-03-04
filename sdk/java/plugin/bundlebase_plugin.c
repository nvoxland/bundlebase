/**
 * C bootstrap for Bundlebase plugin source — Panama bridge.
 *
 * This is a thin shim that:
 * 1. Exports the Bundlebase C ABI symbols (bundlebase_discover, etc.)
 * 2. Starts the JVM once at library load time
 * 3. Calls PluginExport.initialize() which registers Panama upcall stubs
 * 4. All subsequent ABI calls route through Panama function pointers
 *
 * JNI is used ONLY for JVM bootstrap. All data-path calls go through
 * Panama upcalls — no JNI method lookups on the hot path.
 *
 * Build with:
 *   gcc -shared -fPIC -o my_source.so plugin/bundlebase_plugin.c \
 *       -I$JAVA_HOME/include -I$JAVA_HOME/include/$(uname -s | tr A-Z a-z) \
 *       -L$JAVA_HOME/lib/server -ljvm
 */

#include <jni.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>

/* Arrow C Data Interface struct (forward declaration matching Bundlebase ABI) */
struct ArrowArrayStream {
    int (*get_schema)(struct ArrowArrayStream*, void* out);
    int (*get_next)(struct ArrowArrayStream*, void* out);
    const char* (*get_last_error)(struct ArrowArrayStream*);
    void (*release)(struct ArrowArrayStream*);
    void* private_data;
};

/* ---- Panama callback storage ---- */

typedef int32_t (*discover_fn_t)(const char*, char**);
typedef int32_t (*data_fn_t)(const char*, const char*, struct ArrowArrayStream*);
typedef int32_t (*stable_url_fn_t)(const char*, const char*, char**);

static discover_fn_t cb_discover = NULL;
static data_fn_t cb_data = NULL;
static stable_url_fn_t cb_stable_url = NULL;

/**
 * Called from Java via Panama downcall to register upcall stubs.
 * After this, all bundlebase_* calls delegate to these function pointers.
 */
void bundlebase_register_callbacks(
    discover_fn_t discover,
    data_fn_t data,
    stable_url_fn_t stable_url
) {
    cb_discover = discover;
    cb_data = data;
    cb_stable_url = stable_url;
}

/* ---- Exported Bundlebase C ABI functions ---- */

int32_t bundlebase_discover(const char *args_json, char **out_json) {
    if (!cb_discover) {
        if (out_json) *out_json = strdup("JVM not initialized");
        return -1;
    }
    return cb_discover(args_json, out_json);
}

int32_t bundlebase_data(
    const char *location_json,
    const char *args_json,
    struct ArrowArrayStream *out
) {
    if (!cb_data) return -1;
    return cb_data(location_json, args_json, out);
}

void bundlebase_free(char *ptr) {
    if (ptr) free(ptr);
}

int32_t bundlebase_stable_url(
    const char *location_json,
    const char *args_json,
    char **out_json
) {
    if (!cb_stable_url) return 0;
    return cb_stable_url(location_json, args_json, out_json);
}

/* ---- JVM bootstrap (one-time, at library load) ---- */

static JavaVM *jvm = NULL;

__attribute__((constructor))
static void init_jvm(void) {
    JavaVMInitArgs vm_args;
    JavaVMOption options[2];
    options[0].optionString = "-Djava.class.path=target/classes";
    options[1].optionString = "--enable-native-access=ALL-UNNAMED";
    vm_args.version = JNI_VERSION_10;
    vm_args.nOptions = 2;
    vm_args.options = options;
    vm_args.ignoreUnrecognized = JNI_TRUE;

    JNIEnv *env = NULL;
    jint rc = JNI_CreateJavaVM(&jvm, (void**)&env, &vm_args);
    if (rc != JNI_OK) return;

    jclass cls = (*env)->FindClass(env, "com/bundlebase/sdk/PluginExport");
    if (!cls) return;

    /* Call PluginExport.initialize(long registerAddr) — one JNI call, ever */
    jmethodID mid = (*env)->GetStaticMethodID(env, cls, "initialize", "(J)V");
    if (!mid) return;

    jlong addr = (jlong)(uintptr_t)&bundlebase_register_callbacks;
    (*env)->CallStaticVoidMethod(env, cls, mid, addr);

    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionDescribe(env);
        (*env)->ExceptionClear(env);
    }
}
