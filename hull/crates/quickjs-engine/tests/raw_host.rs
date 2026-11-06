#![cfg(feature = "raw")]

use hull_quickjs::raw::*;

// A real C-to-Rust callback exercises JSValue arguments and ownership across
// the full-pointer ABI used by Code Mode, independently of the browser adapter.
unsafe extern "C" fn echo(
    ctx: *mut JSContext,
    _: JSValue,
    argc: std::ffi::c_int,
    argv: *const JSValue,
) -> JSValue {
    assert_eq!(argc, 1);
    unsafe { hull_quickjs_raw_dup_value(ctx, *argv) }
}

#[test]
fn raw_host_callbacks_and_module_promises_use_hulls_owned_values() {
    unsafe {
        let rt = JS_NewRuntime();
        assert!(!rt.is_null());
        let ctx = JS_NewContext(rt);
        assert!(!ctx.is_null());
        let global = JS_GetGlobalObject(ctx);
        let function = JS_NewCFunction2(ctx, Some(echo), c"echo".as_ptr(), 1, JS_CFUNC_GENERIC, 0);
        assert_eq!(
            JS_SetPropertyStr(ctx, global, c"echo".as_ptr(), function),
            1
        );
        hull_quickjs_raw_free_value(ctx, global);
        let promise = js_eval_bytes(
            ctx,
            b"globalThis.result = (await Promise.resolve(echo({text: 'retained'}))).text;",
            c"raw-host.mjs".as_ptr(),
            JS_EVAL_TYPE_MODULE,
        );
        assert!(!promise.is_exception());
        let mut job_ctx = ctx;
        while JS_IsJobPending(rt) != 0 {
            assert!(JS_ExecutePendingJob(rt, &mut job_ctx) >= 0);
        }
        assert_eq!(JS_PromiseState(ctx, promise), 1);
        hull_quickjs_raw_free_value(ctx, promise);
        let value = js_eval_bytes(ctx, b"result", c"raw-host.js".as_ptr(), JS_EVAL_TYPE_GLOBAL);
        assert!(!value.is_exception());
        let retained = hull_quickjs_raw_dup_value(ctx, value);
        hull_quickjs_raw_free_value(ctx, value);
        let mut length = 0;
        let text = JS_ToCStringLen2(ctx, &mut length, retained, 0);
        assert!(!text.is_null());
        assert_eq!(
            std::slice::from_raw_parts(text.cast::<u8>(), length),
            b"retained"
        );
        JS_FreeCString(ctx, text);
        hull_quickjs_raw_free_value(ctx, retained);
        JS_FreeContext(ctx);
        JS_FreeRuntime(rt);
    }
}
