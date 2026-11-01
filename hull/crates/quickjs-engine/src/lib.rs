//! A small, owned QuickJS engine. This crate installs no host or browser APIs.
//!
//! Native values stay behind an opaque C adapter compiled against the same
//! upstream header. Engine ownership stays on one local executor task; dropping
//! an engine releases its context, pending jobs and runtime together.
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_char, c_int, c_void},
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub heap_bytes: usize,
    pub stack_bytes: usize,
    /// A synchronous evaluation that exceeds this slice throws an interruption
    /// exception. It does not resume the interrupted script on the next poll.
    /// This is cooperative: parsing, GC and native builtins can overrun it.
    /// Diagnostic conversion has an additional allowance of at most one ms.
    pub time_slice: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            heap_bytes: 32 * 1024 * 1024,
            stack_bytes: 1024 * 1024,
            time_slice: Duration::from_millis(25),
        }
    }
}

struct InterruptState {
    deadline: Cell<Instant>,
}
unsafe extern "C" fn interrupt(_runtime: *mut c_void, opaque: *mut c_void) -> c_int {
    // SAFETY: native engine lifetime is strictly contained in the lifetime of
    // this stable Box, and the engine is owned by one !Send/!Sync local task.
    let state = unsafe { &*opaque.cast::<InterruptState>() };
    (Instant::now() >= state.deadline.get()) as c_int
}

unsafe extern "C" {
    fn hull_quickjs_new(
        heap: usize,
        stack: usize,
        interrupt: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
        opaque: *mut c_void,
    ) -> *mut c_void;
    fn hull_quickjs_free(engine: *mut c_void);
    fn hull_quickjs_eval(
        engine: *mut c_void,
        source: *const c_char,
        length: usize,
        origin: *const c_char,
        module: c_int,
    ) -> c_int;
    fn hull_quickjs_pending_job(engine: *mut c_void) -> c_int;
    fn hull_quickjs_has_pending_jobs(engine: *const c_void) -> c_int;
    fn hull_quickjs_exception(engine: *mut c_void) -> *const c_char;
}

pub struct Engine {
    native: NonNull<c_void>,
    interrupt: Box<InterruptState>,
    limits: Limits,
    _local_owner: PhantomData<Rc<()>>,
}
impl Engine {
    pub fn new() -> Result<Self, String> {
        Self::with_limits(Limits::default())
    }
    pub fn with_limits(limits: Limits) -> Result<Self, String> {
        if limits.heap_bytes == 0 || limits.stack_bytes == 0 || limits.time_slice.is_zero() {
            return Err("QuickJS limits must be nonzero".into());
        }
        let deadline = Instant::now()
            .checked_add(limits.time_slice)
            .ok_or("QuickJS time slice is too large")?;
        let mut state = Box::new(InterruptState {
            deadline: Cell::new(deadline),
        });
        // SAFETY: the Box allocation stays stable for the entire native lifetime.
        let native = unsafe {
            hull_quickjs_new(
                limits.heap_bytes,
                limits.stack_bytes,
                interrupt,
                (&mut *state as *mut InterruptState).cast(),
            )
        };
        Ok(Self {
            native: NonNull::new(native).ok_or("QuickJS runtime/context allocation failed")?,
            interrupt: state,
            limits,
            _local_owner: PhantomData,
        })
    }
    fn begin_call(&self) {
        self.interrupt
            .deadline
            .set(Instant::now() + self.limits.time_slice);
    }
    fn exception(&self) -> String {
        // Error conversion can invoke user code. Give diagnostics a separate,
        // small cooperative allowance rather than another full evaluation slice.
        self.interrupt
            .deadline
            .set(Instant::now() + self.limits.time_slice.min(Duration::from_millis(1)));
        // SAFETY: the C adapter returns its own live, NUL-terminated error buffer.
        unsafe { CStr::from_ptr(hull_quickjs_exception(self.native.as_ptr())) }
            .to_string_lossy()
            .into_owned()
    }
    /// Evaluate a classic script or native ES module. Module imports remain
    /// unsupported until a module loader is installed explicitly in the future.
    /// Completion values are discarded without coercion or host-side evaluation.
    pub fn eval(&mut self, source: &[u8], origin: &str, module: bool) -> Result<(), String> {
        let origin = CString::new(origin).map_err(|_| "QuickJS origin contains a NUL byte")?;
        // JS_Eval requires a trailing NUL even though it also receives the length.
        let mut terminated = Vec::with_capacity(source.len() + 1);
        terminated.extend_from_slice(source);
        terminated.push(0);
        self.begin_call();
        // SAFETY: the adapter owns the native context and receives valid inputs.
        let status = unsafe {
            hull_quickjs_eval(
                self.native.as_ptr(),
                terminated.as_ptr().cast(),
                source.len(),
                origin.as_ptr(),
                module as c_int,
            )
        };
        if status < 0 {
            Err(self.exception())
        } else {
            Ok(())
        }
    }
    /// Execute at most one pending promise job. A caller can yield its local
    /// Tokio task between jobs instead of draining a page's queue synchronously.
    pub fn execute_pending_job(&mut self) -> Result<bool, String> {
        self.begin_call();
        // SAFETY: the adapter owns every context and transfers exceptions back
        // into the engine's context before returning an error status.
        match unsafe { hull_quickjs_pending_job(self.native.as_ptr()) } {
            value if value < 0 => Err(self.exception()),
            0 => Ok(false),
            _ => Ok(true),
        }
    }
    pub fn has_pending_jobs(&self) -> bool {
        // SAFETY: the engine is live and cannot be shared across threads.
        unsafe { hull_quickjs_has_pending_jobs(self.native.as_ptr()) != 0 }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: called once; native contexts/jobs are freed before the interrupt
        // state Box is dropped. All JSValues remain private to the C adapter.
        unsafe { hull_quickjs_free(self.native.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_conversion_is_interrupted_and_context_recovers() {
        let mut engine = Engine::new().unwrap();
        let error = engine
            .eval(
                b"throw { toString() { while (true) {} }, get stack() { while (true) {} } }",
                "hostile-diagnostic.js",
                false,
            )
            .unwrap_err();
        assert!(!error.is_empty());
        engine
            .eval(
                b"if (6 * 7 !== 42) throw Error('recovery')",
                "after.js",
                false,
            )
            .unwrap();
    }

    #[test]
    fn persistent_state_is_local_to_one_engine() {
        let mut engine = Engine::new().unwrap();
        engine
            .eval(b"globalThis.answer = 40", "first.js", false)
            .unwrap();
        engine
            .eval(
                b"answer += 2; if (answer !== 42) throw Error('lost state')",
                "second.js",
                false,
            )
            .unwrap();
        let mut other = Engine::new().unwrap();
        other
            .eval(
                b"if (typeof answer !== 'undefined') throw Error('shared state')",
                "fresh.js",
                false,
            )
            .unwrap();
    }
    #[test]
    fn promises_are_pumped_explicitly_one_job_at_a_time() {
        let mut engine = Engine::new().unwrap();
        engine
            .eval(
                b"globalThis.done = false; Promise.resolve().then(() => done = true)",
                "promise.js",
                false,
            )
            .unwrap();
        assert!(engine.has_pending_jobs());
        engine
            .eval(
                b"if (done) throw Error('job ran synchronously')",
                "before.js",
                false,
            )
            .unwrap();
        assert!(engine.execute_pending_job().unwrap());
        engine
            .eval(
                b"if (!done) throw Error('job did not run')",
                "after.js",
                false,
            )
            .unwrap();
        assert!(!engine.has_pending_jobs());
        assert!(!engine.execute_pending_job().unwrap());
    }
    #[test]
    fn errors_keep_source_stack_and_allow_later_evaluation() {
        let mut engine = Engine::new().unwrap();
        let error = engine
            .eval(
                b"function fail() { throw new Error('visible failure'); } fail()",
                "failure.js",
                false,
            )
            .unwrap_err();
        assert!(error.contains("visible failure"), "{error}");
        assert!(error.contains("failure.js"), "{error}");
        engine.eval(b"1 + 1", "recovery.js", false).unwrap();
    }
    #[test]
    fn infinite_evaluation_is_interrupted() {
        let mut engine = Engine::with_limits(Limits {
            time_slice: Duration::from_millis(5),
            ..Limits::default()
        })
        .unwrap();
        let start = Instant::now();
        let error = engine
            .eval(b"while (true) {}", "loop.js", false)
            .unwrap_err();
        assert!(error.contains("interrupted"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(2));
        engine
            .eval(b"globalThis.afterInterrupt = true", "recovery.js", false)
            .unwrap();
    }
    #[test]
    fn no_host_apis_are_installed() {
        let mut engine = Engine::new().unwrap();
        engine.eval(b"for (const api of ['std', 'os', 'require', 'fetch', 'document', 'window', 'console']) { if (typeof globalThis[api] !== 'undefined') throw Error('exposed ' + api); }", "capabilities.js", false).unwrap();
    }
    #[test]
    fn completion_values_are_not_stringified() {
        let mut engine = Engine::new().unwrap();
        engine.eval(b"({ toString() { throw Error('completion coerced'); }, [Symbol.toPrimitive]() { throw Error('completion coerced'); } })", "completion.js", false).unwrap();
    }
    #[test]
    fn native_modules_without_imports_work_and_missing_imports_are_errors() {
        let mut engine = Engine::new().unwrap();
        engine
            .eval(
                b"globalThis.moduleRan = true; export const value = 42",
                "module.mjs",
                true,
            )
            .unwrap();
        while engine.execute_pending_job().unwrap() {}
        engine
            .eval(
                b"if (!moduleRan) throw Error('module did not execute')",
                "check.js",
                false,
            )
            .unwrap();
        let error = engine
            .eval(b"import value from 'missing-module'", "import.mjs", true)
            .unwrap_err();
        assert!(error.contains("missing-module"), "{error}");
    }
}
