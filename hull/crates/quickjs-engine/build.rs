use std::{env, fs, path::PathBuf};

// Keep the native value representation private to the matching QuickJS header.
// No JSValue or inline ownership operation crosses the Rust/C boundary.
const ADAPTER: &str = r#"
#include "quickjs.h"
#include <stdlib.h>
#include <string.h>

#define CONSOLE_CAPACITY 64
#define CONSOLE_BYTES 4096
#define HOST_JSON_BYTES (1024 * 1024)
typedef struct ConsoleRecord {
    int level;
    size_t length;
    char text[CONSOLE_BYTES];
} ConsoleRecord;

typedef struct HullQuickJs {
    JSRuntime *runtime;
    JSContext *context;
    JSValue receiver;
    ConsoleRecord console[CONSOLE_CAPACITY];
    size_t console_head, console_count;
    uint64_t console_dropped;
    double (*now)(void *);
    void *clock_opaque;
    int (*host)(void *, const char *, size_t, const char **, size_t *);
    void *host_opaque;
    JSInterruptHandler *interrupt;
    void *interrupt_opaque;
    char error[8192];
} HullQuickJs;

void *hull_quickjs_new(size_t heap_limit, size_t stack_limit,
                      JSInterruptHandler *interrupt, void *opaque) {
    HullQuickJs *engine = calloc(1, sizeof(*engine));
    if (!engine) return NULL;
    engine->receiver = JS_UNDEFINED;
    engine->interrupt = interrupt;
    engine->interrupt_opaque = opaque;
    engine->runtime = JS_NewRuntime();
    if (!engine->runtime) { free(engine); return NULL; }
    JS_SetMemoryLimit(engine->runtime, heap_limit);
    JS_SetMaxStackSize(engine->runtime, stack_limit);
    JS_SetCanBlock(engine->runtime, 0);
    JS_SetInterruptHandler(engine->runtime, interrupt, opaque);
    engine->context = JS_NewContext(engine->runtime);
    if (!engine->context) { JS_FreeRuntime(engine->runtime); free(engine); return NULL; }
    JS_SetContextOpaque(engine->context, engine);
    return engine;
}

void hull_quickjs_free(void *pointer) {
    HullQuickJs *engine = pointer;
    JS_FreeValue(engine->context, engine->receiver);
    JS_FreeContext(engine->context);
    JS_FreeRuntime(engine->runtime);
    free(engine);
}

static JSValue host_call(JSContext *context, JSValueConst self,
                         int argc, JSValueConst *argv) {
    HullQuickJs *engine = JS_GetContextOpaque(context);
    if (!engine->host) return JS_ThrowInternalError(context, "host handler is unavailable");
    if (argc != 1) return JS_ThrowTypeError(context, "host handler requires one JSON value");
    JSValue json = JS_JSONStringify(context, argv[0], JS_UNDEFINED, JS_UNDEFINED);
    if (JS_IsException(json)) return json;
    if (JS_IsUndefined(json)) return JS_ThrowTypeError(context, "host request is not a JSON value");
    size_t length;
    const char *text = JS_ToCStringLen(context, &length, json);
    JS_FreeValue(context, json);
    if (!text) return JS_EXCEPTION;
    if (length > HOST_JSON_BYTES) {
        JS_FreeCString(context, text);
        return JS_ThrowRangeError(context, "host request exceeds 1 MiB");
    }
    const char *response = NULL;
    size_t response_length = 0;
    int status = engine->host(engine->host_opaque, text, length, &response, &response_length);
    JS_FreeCString(context, text);
    if (!response || response_length > HOST_JSON_BYTES)
        return JS_ThrowInternalError(context, "invalid host response");
    if (engine->interrupt(engine->runtime, engine->interrupt_opaque))
        return JS_ThrowInternalError(context, "interrupted");
    if (status < 0)
        return JS_ThrowInternalError(context, "%.*s", (int)(response_length < 8192 ? response_length : 8192), response);
    // Response storage belongs to Rust and remains valid through this parse.
    return JS_ParseJSON(context, response, response_length, "<host-response>");
}

int hull_quickjs_bind_host_handler(void *pointer, const char *name,
                                 int (*host)(void *, const char *, size_t, const char **, size_t *),
                                 void *opaque) {
    HullQuickJs *engine = pointer;
    JSContext *context = engine->context;
    JSValue global = JS_GetGlobalObject(context);
    int status = JS_SetPropertyStr(context, global, name,
                                  JS_NewCFunction(context, host_call, name, 1));
    JS_FreeValue(context, global);
    if (status >= 0) {
        engine->host = host;
        engine->host_opaque = opaque;
    }
    return status;
}

static JSValue console_write(JSContext *context, JSValueConst self,
                             int argc, JSValueConst *argv) {
    HullQuickJs *engine = JS_GetContextOpaque(context);
    if (engine->console_count == CONSOLE_CAPACITY) {
        if (engine->console_dropped != UINT64_MAX) engine->console_dropped++;
        return JS_UNDEFINED;
    }
    int level;
    if (argc < 2) return JS_ThrowTypeError(context, "console record requires level and text");
    if (JS_ToInt32(context, &level, argv[0])) return JS_EXCEPTION;
    size_t length;
    const char *text = JS_ToCStringLen(context, &length, argv[1]);
    if (!text) return JS_EXCEPTION;
    size_t used = length < CONSOLE_BYTES ? length : CONSOLE_BYTES;
    // Never split a UTF-8 sequence at the bounded native record boundary.
    if (used < length) while (used && ((unsigned char)text[used] & 0xc0) == 0x80) used--;
    ConsoleRecord *record = &engine->console[(engine->console_head + engine->console_count) % CONSOLE_CAPACITY];
    record->level = level;
    record->length = used;
    memcpy(record->text, text, used);
    engine->console_count++;
    JS_FreeCString(context, text);
    return JS_UNDEFINED;
}

int hull_quickjs_install_console(void *pointer) {
    HullQuickJs *engine = pointer;
    JSContext *context = engine->context;
    JSValue global = JS_GetGlobalObject(context);
    int status = JS_SetPropertyStr(context, global, "__hullConsoleWrite",
                                  JS_NewCFunction(context, console_write, "write", 2));
    JS_FreeValue(context, global);
    return status;
}

int hull_quickjs_console_pop(void *pointer, char *text, size_t capacity, int *level) {
    HullQuickJs *engine = pointer;
    if (!engine->console_count) return -1;
    ConsoleRecord *record = &engine->console[engine->console_head];
    if (capacity < record->length) return -2;
    memcpy(text, record->text, record->length);
    *level = record->level;
    int length = (int)record->length;
    engine->console_head = (engine->console_head + 1) % CONSOLE_CAPACITY;
    engine->console_count--;
    return length;
}

uint64_t hull_quickjs_console_dropped(void *pointer) {
    HullQuickJs *engine = pointer;
    uint64_t dropped = engine->console_dropped;
    engine->console_dropped = 0;
    return dropped;
}

static JSValue performance_now(JSContext *context, JSValueConst self,
                               int argc, JSValueConst *argv) {
    HullQuickJs *engine = JS_GetContextOpaque(context);
    return JS_NewFloat64(context, engine->now(engine->clock_opaque));
}

int hull_quickjs_install_clock(void *pointer, double (*now)(void *), void *opaque) {
    HullQuickJs *engine = pointer;
    engine->now = now;
    engine->clock_opaque = opaque;
    JSContext *context = engine->context;
    JSValue global = JS_GetGlobalObject(context);
    JSValue performance = JS_NewObject(context);
    if (JS_IsException(performance)) { JS_FreeValue(context, global); return -1; }
    int status = JS_SetPropertyStr(context, performance, "now",
                                  JS_NewCFunction(context, performance_now, "now", 0));
    if (status >= 0) status = JS_SetPropertyStr(context, global, "performance", performance);
    else JS_FreeValue(context, performance);
    JS_FreeValue(context, global);
    return status;
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

int hull_quickjs_bind_message_handler(void *pointer, const char *global_name) {
    HullQuickJs *engine = pointer;
    JSContext *context = engine->context;
    JS_UpdateStackTop(engine->runtime);
    JSValue global = JS_GetGlobalObject(context);
    JSValue receiver = JS_GetPropertyStr(context, global, global_name);
    if (JS_IsException(receiver)) { JS_FreeValue(context, global); return -1; }
    if (!JS_IsFunction(context, receiver)) {
        JS_FreeValue(context, receiver);
        JS_FreeValue(context, global);
        JS_ThrowTypeError(context, "message handler must be a function");
        return -1;
    }
    JSAtom atom = JS_NewAtom(context, global_name);
    if (atom == JS_ATOM_NULL) {
        JS_FreeValue(context, receiver);
        JS_FreeValue(context, global);
        return -1;
    }
    int deleted = JS_DeleteProperty(context, global, atom, JS_PROP_THROW);
    JS_FreeAtom(context, atom);
    JS_FreeValue(context, global);
    if (deleted <= 0) {
        JS_FreeValue(context, receiver);
        if (!deleted) JS_ThrowTypeError(context, "message handler global could not be removed");
        return -1;
    }
    // Commit only after validation and global removal; a failed rebind leaves
    // the previous pinned transport intact.
    JS_FreeValue(context, engine->receiver);
    engine->receiver = receiver;
    return 0;
}

int hull_quickjs_dispatch_json(void *pointer, const char *json, size_t length) {
    HullQuickJs *engine = pointer;
    JSContext *context = engine->context;
    JS_UpdateStackTop(engine->runtime);
    if (JS_IsUndefined(engine->receiver)) {
        JS_ThrowTypeError(context, "message handler is not bound");
        return -1;
    }
    // Parse data directly through the native JSON parser, never as source code.
    JSValue message = JS_ParseJSON(context, json, length, "<host-message>");
    if (JS_IsException(message)) return -1;
    JSValue result = JS_Call(context, engine->receiver, JS_UNDEFINED, 1, &message);
    JS_FreeValue(context, message);
    if (JS_IsException(result)) return -1;
    JS_FreeValue(context, result);
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

void hull_quickjs_discard_exception(void *pointer) {
    HullQuickJs *engine = pointer;
    JSValue exception = JS_GetException(engine->context);
    JS_FreeValue(engine->context, exception);
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
    if (!engine->error[0]) {
        static const char fallback[] = "JavaScript exception (message unavailable)";
        // Include the terminator and use the existing TRUEOS memcpy binding.
        memcpy(engine->error, fallback, sizeof(fallback));
    }
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
