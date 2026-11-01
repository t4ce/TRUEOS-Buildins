use std::{env, fs, path::PathBuf};

// Keep the native value representation private to the matching QuickJS header.
// No JSValue or inline ownership operation crosses the Rust/C boundary.
const ADAPTER: &str = r#"
#include "quickjs.h"
#include <stdlib.h>
#include <string.h>

typedef struct HullQuickJs {
    JSRuntime *runtime;
    JSContext *context;
    char error[8192];
} HullQuickJs;

void *hull_quickjs_new(size_t heap_limit, size_t stack_limit,
                      JSInterruptHandler *interrupt, void *opaque) {
    HullQuickJs *engine = calloc(1, sizeof(*engine));
    if (!engine) return NULL;
    engine->runtime = JS_NewRuntime();
    if (!engine->runtime) { free(engine); return NULL; }
    JS_SetMemoryLimit(engine->runtime, heap_limit);
    JS_SetMaxStackSize(engine->runtime, stack_limit);
    JS_SetCanBlock(engine->runtime, 0);
    JS_SetInterruptHandler(engine->runtime, interrupt, opaque);
    engine->context = JS_NewContext(engine->runtime);
    if (!engine->context) { JS_FreeRuntime(engine->runtime); free(engine); return NULL; }
    return engine;
}

void hull_quickjs_free(void *pointer) {
    HullQuickJs *engine = pointer;
    JS_FreeContext(engine->context);
    JS_FreeRuntime(engine->runtime);
    free(engine);
}

int hull_quickjs_eval(void *pointer, const char *source, size_t length,
                      const char *origin, int module) {
    HullQuickJs *engine = pointer;
    JS_UpdateStackTop(engine->runtime);
    JSValue value = JS_Eval(engine->context, source, length, origin,
                           module ? JS_EVAL_TYPE_MODULE : JS_EVAL_TYPE_GLOBAL);
    if (JS_IsException(value)) return -1;
    // Discard the completion value without invoking user-defined coercion.
    JS_FreeValue(engine->context, value);
    return 0;
}

int hull_quickjs_pending_job(void *pointer) {
    HullQuickJs *engine = pointer;
    JSContext *context = engine->context;
    JS_UpdateStackTop(engine->runtime);
    int status = JS_ExecutePendingJob(engine->runtime, &context);
    if (status < 0 && context != engine->context) {
        JSValue exception = JS_GetException(context);
        JS_Throw(engine->context, exception);
    }
    return status;
}

int hull_quickjs_has_pending_jobs(const void *pointer) {
    const HullQuickJs *engine = pointer;
    return JS_IsJobPending(engine->runtime);
}

static void append_text(HullQuickJs *engine, JSValueConst value) {
    JSContext *context = engine->context;
    size_t length;
    const char *text = JS_ToCStringLen(context, &length, value);
    if (!text) {
        JSValue nested = JS_GetException(context);
        JS_FreeValue(context, nested);
        return;
    }
    size_t used = strlen(engine->error);
    size_t remaining = sizeof(engine->error) - used - 1;
    if (length > remaining) length = remaining;
    memcpy(engine->error + used, text, length);
    engine->error[used + length] = 0;
    JS_FreeCString(context, text);
}

const char *hull_quickjs_exception(void *pointer) {
    HullQuickJs *engine = pointer;
    JSContext *context = engine->context;
    JS_UpdateStackTop(engine->runtime);
    engine->error[0] = 0;
    JSValue exception = JS_GetException(context);
    append_text(engine, exception);
    if (JS_IsObject(exception)) {
        JSValue stack = JS_GetPropertyStr(context, exception, "stack");
        if (JS_IsException(stack)) {
            JSValue nested = JS_GetException(context);
            JS_FreeValue(context, nested);
        } else if (!JS_IsUndefined(stack)) {
            size_t used = strlen(engine->error);
            if (used < sizeof(engine->error) - 1) {
                engine->error[used] = '\n'; engine->error[used + 1] = 0;
            }
            append_text(engine, stack);
        }
        JS_FreeValue(context, stack);
    }
    JS_FreeValue(context, exception);
    if (!engine->error[0]) strcpy(engine->error, "JavaScript exception (message unavailable)");
    return engine->error;
}
"#;

const FORMATTER: &str = r#"
#include <stdarg.h>
#include <stddef.h>
#include <limits.h>
#define STB_SPRINTF_IMPLEMENTATION
#define STB_SPRINTF_STATIC
#include "stb_sprintf.h"
int hull_quickjs_vsnprintf(char *buffer, size_t count, const char *format, va_list arguments) {
    if (count > INT_MAX || !format || (!buffer && count)) return -1;
    if (!count) buffer = NULL;
    return stbsp_vsnprintf(buffer, (int)count, format, arguments);
}
int hull_quickjs_snprintf(char *buffer, size_t count, const char *format, ...) {
    va_list arguments; va_start(arguments, format);
    int result = hull_quickjs_vsnprintf(buffer, count, format, arguments);
    va_end(arguments); return result;
}
"#;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = manifest.join("vendor/quickjs");
    let adapter = out.join("hull_quickjs_adapter.c");
    fs::write(&adapter, ADAPTER).expect("write QuickJS opaque adapter");
    let mut build = cc::Build::new();
    build
        .include(&source)
        .file(&adapter)
        .define(
            "CONFIG_VERSION",
            Some("\"2833510432045f72915919cef680ba59e5153b16\""),
        )
        .flag_if_supported("-fno-stack-protector")
        .flag_if_supported("-U_FORTIFY_SOURCE")
        .define("_FORTIFY_SOURCE", Some("0"))
        .warnings(false);
    for name in [
        "quickjs.c",
        "libregexp.c",
        "libunicode.c",
        "cutils.c",
        "dtoa.c",
    ] {
        build.file(source.join(name));
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("trueos") {
        let formatter = out.join("hull_quickjs_formatter.c");
        fs::write(&formatter, FORMATTER).expect("write private QuickJS formatter");
        build
            .include(manifest.join("vendor/stb"))
            .file(formatter)
            .define("snprintf", Some("hull_quickjs_snprintf"))
            .define("vsnprintf", Some("hull_quickjs_vsnprintf"));
        build.flag_if_supported("-mno-red-zone");
    }
    build.compile("hull_quickjs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/quickjs");
    println!("cargo:rerun-if-changed=vendor/stb");
}
