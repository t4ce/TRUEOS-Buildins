//! Low-level full-pointer QuickJS API for interpreter hosts such as Codex Code Mode.
//!
//! # Safety
//! Values and pointers belong to one runtime on one thread. The caller must
//! balance ownership, keep callback state alive, and never unwind through C.
//! This uses Hull's pinned engine and header; it installs no browser host APIs.
#![allow(non_snake_case, unsafe_op_in_unsafe_fn)]
use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
pub struct JSRuntime {
    _private: [u8; 0],
}

#[repr(C)]
pub struct JSContext {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union JSValueUnion {
    pub int32: i32,
    pub float64: f64,
    pub ptr: *mut c_void,
    pub short_big_int: i64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct JSValue {
    pub u: JSValueUnion,
    pub tag: i64,
}

pub type JSValueConst = JSValue;

pub const JS_TAG_UNDEFINED: i64 = 3;
pub const JS_TAG_EXCEPTION: i64 = 6;
pub const JS_EVAL_TYPE_GLOBAL: c_int = 0;
pub const JS_EVAL_TYPE_MODULE: c_int = 1;
pub const JS_CFUNC_GENERIC: c_int = 0;

// Match the C adapter's assertions for the full-pointer representation.
const _: () = assert!(size_of::<JSValue>() == 16);
const _: () = assert!(std::mem::offset_of!(JSValue, tag) == 8);

impl JSValue {
    #[inline]
    pub fn undefined() -> Self {
        Self {
            u: JSValueUnion { int32: 0 },
            tag: JS_TAG_UNDEFINED,
        }
    }

    #[inline]
    pub fn is_exception(self) -> bool {
        self.tag == JS_TAG_EXCEPTION
    }
}

pub type JSCFunction =
    unsafe extern "C" fn(*mut JSContext, JSValue, c_int, *const JSValue) -> JSValue;
pub type JSHostPromiseRejectionTracker =
    unsafe extern "C" fn(*mut JSContext, JSValue, JSValue, c_int, *mut c_void);

unsafe extern "C" {
    pub fn JS_Eval(
        ctx: *mut JSContext,
        input: *const c_char,
        input_len: usize,
        filename: *const c_char,
        eval_flags: c_int,
    ) -> JSValue;
    pub fn JS_Call(
        ctx: *mut JSContext,
        func_obj: JSValueConst,
        this_obj: JSValueConst,
        argc: c_int,
        argv: *const JSValueConst,
    ) -> JSValue;
    pub fn JS_ExecutePendingJob(rt: *mut JSRuntime, pctx: *mut *mut JSContext) -> c_int;
    pub fn JS_FreeCString(ctx: *mut JSContext, ptr: *const c_char);
    pub fn JS_FreeContext(ctx: *mut JSContext);
    pub fn JS_FreeRuntime(rt: *mut JSRuntime);
    pub fn JS_GetContextOpaque(ctx: *mut JSContext) -> *mut c_void;
    pub fn JS_GetException(ctx: *mut JSContext) -> JSValue;
    pub fn JS_GetGlobalObject(ctx: *mut JSContext) -> JSValue;
    pub fn JS_IsJobPending(rt: *mut JSRuntime) -> c_int;
    pub fn JS_NewCFunction2(
        ctx: *mut JSContext,
        func: Option<JSCFunction>,
        name: *const c_char,
        length: c_int,
        cproto: c_int,
        magic: c_int,
    ) -> JSValue;
    pub fn JS_NewContext(rt: *mut JSRuntime) -> *mut JSContext;
    pub fn JS_NewError(ctx: *mut JSContext) -> JSValue;
    pub fn JS_NewPromiseCapability(ctx: *mut JSContext, resolving_funcs: *mut JSValue) -> JSValue;
    pub fn JS_NewRuntime() -> *mut JSRuntime;
    pub fn JS_NewStringLen(ctx: *mut JSContext, buf: *const c_char, buf_len: usize) -> JSValue;
    pub fn JS_SetContextOpaque(ctx: *mut JSContext, opaque: *mut c_void);
    pub fn JS_SetHostPromiseRejectionTracker(
        rt: *mut JSRuntime,
        cb: Option<JSHostPromiseRejectionTracker>,
        opaque: *mut c_void,
    );
    pub fn JS_SetPropertyStr(
        ctx: *mut JSContext,
        this_obj: JSValueConst,
        prop: *const c_char,
        val: JSValue,
    ) -> c_int;
    pub fn JS_Throw(ctx: *mut JSContext, obj: JSValue) -> JSValue;
    pub fn JS_ToCStringLen2(
        ctx: *mut JSContext,
        plen: *mut usize,
        val1: JSValueConst,
        cesu8: c_int,
    ) -> *const c_char;
    pub fn JS_ToFloat64(ctx: *mut JSContext, pres: *mut f64, val: JSValueConst) -> c_int;
    pub fn hull_quickjs_raw_free_value(ctx: *mut JSContext, value: JSValue);
    pub fn hull_quickjs_raw_dup_value(ctx: *mut JSContext, value: JSValue) -> JSValue;
    pub fn JS_SetInterruptHandler(
        rt: *mut JSRuntime,
        cb: unsafe extern "C" fn(*mut JSRuntime, *mut c_void) -> c_int,
        opaque: *mut c_void,
    );
    pub fn JS_SetMemoryLimit(rt: *mut JSRuntime, limit: usize);
    pub fn JS_SetMaxStackSize(rt: *mut JSRuntime, limit: usize);
    pub fn JS_PromiseState(ctx: *mut JSContext, promise: JSValue) -> c_int;
    pub fn JS_PromiseResult(ctx: *mut JSContext, promise: JSValue) -> JSValue;
    pub fn JS_ParseJSON(
        ctx: *mut JSContext,
        data: *const c_char,
        len: usize,
        filename: *const c_char,
    ) -> JSValue;
}

/// Evaluate source while satisfying QuickJS' requirement:
/// `input[input_len] == '\0'`.
///
/// Safety: caller must provide a live `ctx` and a valid C-string `filename`.
#[inline]
pub unsafe fn js_eval_bytes(
    ctx: *mut JSContext,
    input: &[u8],
    filename: *const c_char,
    eval_flags: c_int,
) -> JSValue {
    let mut nul_terminated = Vec::with_capacity(input.len().saturating_add(1));
    nul_terminated.extend_from_slice(input);
    nul_terminated.push(0);
    JS_Eval(
        ctx,
        nul_terminated.as_ptr() as *const c_char,
        input.len(),
        filename,
        eval_flags,
    )
}
