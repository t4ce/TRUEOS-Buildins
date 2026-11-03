//! A small, owned QuickJS engine. Host APIs require explicit installation.
//!
//! Native values stay behind an opaque C adapter compiled against the same
//! upstream header. Engine ownership stays on one local executor task; dropping
//! an engine releases its context, pending jobs and runtime together.
use std::{
    cell::{Cell, RefCell},
    ffi::{CStr, CString, c_char, c_int, c_void},
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind},
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
    started: Instant,
}

const HOST_JSON_BYTES: usize = 1024 * 1024;
type HostHandler = dyn FnMut(&[u8]) -> Result<Vec<u8>, String>;
struct HostState {
    handler: RefCell<Box<HostHandler>>,
    response: RefCell<Vec<u8>>,
}
type ModuleNormalizer = dyn FnMut(&str, &str) -> Result<String, String>;
struct ModuleState {
    normalize: RefCell<Box<ModuleNormalizer>>,
    output: RefCell<Vec<u8>>,
}
unsafe extern "C" fn normalize_module(
    opaque: *mut c_void,
    base: *const c_char,
    name: *const c_char,
    output: *mut *const c_char,
    length: *mut usize,
) -> c_int {
    // SAFETY: C passes terminated strings and the installed stable state.
    let state = unsafe { &*opaque.cast::<ModuleState>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let base = unsafe { CStr::from_ptr(base) }
            .to_str()
            .map_err(|_| "Invalid module base UTF-8".to_owned())?;
        let name = unsafe { CStr::from_ptr(name) }
            .to_str()
            .map_err(|_| "Invalid module specifier UTF-8".to_owned())?;
        state
            .normalize
            .try_borrow_mut()
            .map_err(|_| "Module resolver reentry".to_owned())?(base, name)
    }))
    .unwrap_or_else(|_| Err("Module resolver panicked".into()));
    let (status, text) = match result {
        Ok(s) => (0, s),
        Err(e) => (-1, e),
    };
    let mut buffer = state.output.borrow_mut();
    *buffer = text.into_bytes();
    buffer.push(0);
    unsafe {
        *output = buffer.as_ptr().cast();
        *length = buffer.len() - 1;
    }
    status
}
/// An import Promise waiting for host-side source preparation. No native value
/// or resolver function escapes the owned engine.
#[derive(Clone, Debug)]
pub struct ImportRequest {
    pub id: u64,
    pub base: String,
    pub specifier: String,
}
struct DependencyList {
    names: Vec<String>,
    invalid: bool,
}
unsafe extern "C" fn dependency_item(opaque: *mut c_void, text: *const c_char, length: usize) {
    let list = unsafe { &mut *opaque.cast::<DependencyList>() };
    let bytes = unsafe { std::slice::from_raw_parts(text.cast::<u8>(), length) };
    match std::str::from_utf8(bytes) {
        Ok(value) if value.len() <= 4096 && list.names.len() < 256 => list.names.push(value.into()),
        _ => list.invalid = true,
    }
}

unsafe extern "C" fn host_call(
    opaque: *mut c_void,
    input: *const c_char,
    length: usize,
    output: *mut *const c_char,
    output_length: *mut usize,
) -> c_int {
    // SAFETY: the engine owns this stable Box until after the native context is
    // freed; C supplies valid request storage and output slots for this call.
    let state = unsafe { &*opaque.cast::<HostState>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        if length > HOST_JSON_BYTES {
            return Err("host request exceeds 1 MiB".into());
        }
        let input = unsafe { std::slice::from_raw_parts(input.cast::<u8>(), length) };
        let mut handler = state
            .handler
            .try_borrow_mut()
            .map_err(|_| "host handler is already executing".to_owned())?;
        handler(input)
    }))
    .unwrap_or_else(|_| Err("host handler panicked".into()));
    let (status, bytes) = match result {
        Ok(bytes) if bytes.len() <= HOST_JSON_BYTES => (0, bytes),
        Ok(_) => (-1, b"host response exceeds 1 MiB".to_vec()),
        Err(error) if error.len() <= HOST_JSON_BYTES => (-1, error.into_bytes()),
        Err(_) => (-1, b"host error exceeds 1 MiB".to_vec()),
    };
    let mut response = state.response.borrow_mut();
    *response = bytes;
    response.push(0); // JS_ParseJSON requires a terminator beyond its exact length.
    unsafe {
        *output = response.as_ptr().cast();
        *output_length = response.len() - 1;
    }
    status
}
unsafe extern "C" fn elapsed_ms(opaque: *mut c_void) -> f64 {
    // SAFETY: the stable interrupt state outlives its native context.
    unsafe { &*opaque.cast::<InterruptState>() }
        .started
        .elapsed()
        .as_secs_f64()
        * 1000.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel {
    Log,
    Debug,
    Info,
    Warn,
    Error,
}
impl ConsoleLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}
#[derive(Debug)]
pub struct ConsoleMessage {
    pub level: ConsoleLevel,
    pub text: String,
}
#[derive(Debug, Default)]
pub struct ConsoleBatch {
    pub messages: Vec<ConsoleMessage>,
    pub dropped: u64,
}
unsafe extern "C" fn interrupt(_runtime: *mut c_void, opaque: *mut c_void) -> c_int {
    // SAFETY: native engine lifetime is strictly contained in the lifetime of
    // this stable Box, and the engine is owned by one !Send/!Sync local task.
    let state = unsafe { &*opaque.cast::<InterruptState>() };
    (Instant::now() >= state.deadline.get()) as c_int
}

unsafe extern "C" {
    fn hull_quickjs_rejection_pop(engine: *mut c_void) -> c_int;
    fn hull_quickjs_install_modules(
        engine: *mut c_void,
        normalize: unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            *const c_char,
            *mut *const c_char,
            *mut usize,
        ) -> c_int,
        opaque: *mut c_void,
    ) -> c_int;
    fn hull_quickjs_add_module(
        engine: *mut c_void,
        name: *const c_char,
        source: *const c_char,
        length: usize,
    ) -> c_int;
    fn hull_quickjs_module_dependencies(
        engine: *mut c_void,
        source: *const c_char,
        length: usize,
        origin: *const c_char,
        item: unsafe extern "C" fn(*mut c_void, *const c_char, usize),
        opaque: *mut c_void,
    ) -> c_int;
    fn hull_quickjs_import_pop(
        engine: *mut c_void,
        id: *mut u64,
        base: *mut c_char,
        name: *mut c_char,
    ) -> c_int;
    fn hull_quickjs_import_complete(engine: *mut c_void, id: u64, error: *const c_char) -> c_int;
    fn hull_quickjs_load_module(engine: *mut c_void, name: *const c_char) -> c_int;
    fn hull_quickjs_new(
        heap: usize,
        stack: usize,
        interrupt: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
        opaque: *mut c_void,
    ) -> *mut c_void;
    fn hull_quickjs_free(engine: *mut c_void);
    fn hull_quickjs_bind_host_handler(
        engine: *mut c_void,
        name: *const c_char,
        handler: unsafe extern "C" fn(
            *mut c_void,
            *const c_char,
            usize,
            *mut *const c_char,
            *mut usize,
        ) -> c_int,
        opaque: *mut c_void,
    ) -> c_int;
    fn hull_quickjs_discard_exception(engine: *mut c_void);
    fn hull_quickjs_install_console(engine: *mut c_void) -> c_int;
    fn hull_quickjs_console_pop(
        engine: *mut c_void,
        text: *mut c_char,
        capacity: usize,
        level: *mut c_int,
    ) -> c_int;
    fn hull_quickjs_console_dropped(engine: *mut c_void) -> u64;
    fn hull_quickjs_install_clock(
        engine: *mut c_void,
        now: unsafe extern "C" fn(*mut c_void) -> f64,
        opaque: *mut c_void,
    ) -> c_int;
    fn hull_quickjs_eval(
        engine: *mut c_void,
        source: *const c_char,
        length: usize,
        origin: *const c_char,
        module: c_int,
    ) -> c_int;
    fn hull_quickjs_bind_message_handler(engine: *mut c_void, global_name: *const c_char) -> c_int;
    fn hull_quickjs_dispatch_json(engine: *mut c_void, json: *const c_char, length: usize)
    -> c_int;
    fn hull_quickjs_pending_job(engine: *mut c_void) -> c_int;
    fn hull_quickjs_has_pending_jobs(engine: *const c_void) -> c_int;
    fn hull_quickjs_exception(engine: *mut c_void) -> *const c_char;
}

pub struct Engine {
    native: NonNull<c_void>,
    interrupt: Box<InterruptState>,
    limits: Limits,
    budget_remaining: Cell<Option<Duration>>,
    host: Option<Box<HostState>>,
    modules: Option<Box<ModuleState>>,
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
            started: Instant::now(),
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
            budget_remaining: Cell::new(None),
            host: None,
            modules: None,
            _local_owner: PhantomData,
        })
    }
    /// Install a text-only console backed by 64 native records, at most 4096
    /// UTF-8 bytes each. The caller explicitly drains output outside guest calls.
    pub fn install_console(&mut self) -> Result<(), String> {
        let started = self.begin_call()?;
        // SAFETY: installation operates on the owned context only.
        let status = unsafe { hull_quickjs_install_console(self.native.as_ptr()) };
        self.finish_call(started, status)?;
        self.eval(include_bytes!("console.js"), "hull:console", false)
    }

    /// Install monotonic milliseconds since this engine was created. No timers.
    pub fn install_clock(&mut self) -> Result<(), String> {
        let started = self.begin_call()?;
        // SAFETY: the stable state and callback outlive the owned native context.
        let status = unsafe {
            hull_quickjs_install_clock(
                self.native.as_ptr(),
                elapsed_ms,
                (&*self.interrupt as *const InterruptState)
                    .cast_mut()
                    .cast(),
            )
        };
        self.finish_call(started, status)
    }

    /// Install one synchronous JSON endpoint. No native JS values escape C.
    /// Input and output are limited to 1 MiB each. The callback must do bounded
    /// work: its elapsed time is charged, but native work cannot be preempted.
    pub fn bind_host_handler(
        &mut self,
        global_name: &str,
        handler: impl FnMut(&[u8]) -> Result<Vec<u8>, String> + 'static,
    ) -> Result<(), String> {
        if self.host.is_some() {
            return Err("host handler is already installed".into());
        }
        let name =
            CString::new(global_name).map_err(|_| "host handler name contains a NUL byte")?;
        let mut state = Box::new(HostState {
            handler: RefCell::new(Box::new(handler)),
            response: RefCell::new(Vec::new()),
        });
        let started = self.begin_call()?;
        // SAFETY: Box storage is stable and retained on every successful native
        // installation, including if accounting expires immediately afterwards.
        let status = unsafe {
            hull_quickjs_bind_host_handler(
                self.native.as_ptr(),
                name.as_ptr(),
                host_call,
                (&mut *state as *mut HostState).cast(),
            )
        };
        if status >= 0 {
            self.host = Some(state);
        }
        self.finish_call(started, status)
    }

    /// Set a shared allowance for active native engine calls, including host
    /// callbacks and diagnostic conversion. Idle time between calls is excluded.
    /// Only the host can set this allowance; guest sources cannot renew it.
    pub fn set_execution_budget(&mut self, budget: Duration) {
        self.budget_remaining.set(Some(budget));
    }

    /// Install a pure URL resolver. Network/filesystem operations must happen
    /// before cache admission, never inside this callback. Dynamic imports wait
    /// for explicit host completion instead of retrying or reexecuting code.
    pub fn install_modules(
        &mut self,
        normalize: impl FnMut(&str, &str) -> Result<String, String> + 'static,
    ) -> Result<(), String> {
        if self.modules.is_some() {
            return Err("Module loader already installed".into());
        }
        let mut state = Box::new(ModuleState {
            normalize: RefCell::new(Box::new(normalize)),
            output: RefCell::new(Vec::new()),
        });
        let status = unsafe {
            hull_quickjs_install_modules(
                self.native.as_ptr(),
                normalize_module,
                (&mut *state as *mut ModuleState).cast(),
            )
        };
        if status < 0 {
            return Err("Module loader installation failed".into());
        }
        self.modules = Some(state);
        Ok(())
    }
    /// Store source without parsing or executing it. The host bounds aggregate
    /// bytes/count; individual module bodies are limited to 1 MiB here.
    pub fn add_module(&mut self, url: &str, source: &[u8]) -> Result<(), String> {
        if self.modules.is_none() {
            return Err("Module loader is not installed".into());
        }
        if source.len() > 1024 * 1024 || url.len() > 4096 {
            return Err("Module source/URL exceeds budget".into());
        }
        let name = CString::new(url).map_err(|_| "Module URL contains NUL")?;
        let status = unsafe {
            hull_quickjs_add_module(
                self.native.as_ptr(),
                name.as_ptr(),
                source.as_ptr().cast(),
                source.len(),
            )
        };
        if status < 0 {
            Err("Module source allocation failed".into())
        } else {
            Ok(())
        }
    }
    /// The upstream parser discovers static import/export-from requests, with
    /// no module execution or import resolution and no text rewriting.
    pub fn module_dependencies(
        &mut self,
        source: &[u8],
        origin: &str,
    ) -> Result<Vec<String>, String> {
        if source.len() > 1024 * 1024 {
            return Err("Module source exceeds 1 MiB".into());
        }
        let origin = CString::new(origin).map_err(|_| "Module origin contains NUL")?;
        let mut terminated = source.to_vec();
        terminated.push(0);
        let mut list = DependencyList {
            names: Vec::new(),
            invalid: false,
        };
        let started = self.begin_call()?;
        let status = unsafe {
            hull_quickjs_module_dependencies(
                self.native.as_ptr(),
                terminated.as_ptr().cast(),
                source.len(),
                origin.as_ptr(),
                dependency_item,
                (&mut list as *mut DependencyList).cast(),
            )
        };
        self.finish_call(started, status)?;
        if list.invalid {
            return Err("Module dependency list exceeds URL/count/UTF-8 bounds".into());
        }
        Ok(list.names)
    }
    /// Evaluate a prepared external entry through QuickJS's module map, so
    /// repeated script elements and import() share one execution/namespace.
    pub fn load_module(&mut self, url: &str) -> Result<(), String> {
        if self.modules.is_none() || url.len()>4096 {return Err("Module loader is absent or URL exceeds budget".into());}
        let name = CString::new(url).map_err(|_| "Module URL contains NUL")?;
        let started = self.begin_call()?;
        let status = unsafe { hull_quickjs_load_module(self.native.as_ptr(), name.as_ptr()) };
        self.finish_call(started, status)
    }
    pub fn take_import_requests(&mut self) -> Vec<ImportRequest> {
        let mut result = Vec::new();
        for _ in 0..32 {
            let mut base = [0u8; 4097];
            let mut name = [0u8; 4097];
            let mut id = 0;
            let status = unsafe {
                hull_quickjs_import_pop(
                    self.native.as_ptr(),
                    &mut id,
                    base.as_mut_ptr().cast(),
                    name.as_mut_ptr().cast(),
                )
            };
            if status != 1 {
                break;
            }
            let base = unsafe { CStr::from_ptr(base.as_ptr().cast()) }
                .to_string_lossy()
                .into_owned();
            let specifier = unsafe { CStr::from_ptr(name.as_ptr().cast()) }
                .to_string_lossy()
                .into_owned();
            result.push(ImportRequest {
                id,
                base,
                specifier,
            });
        }
        result
    }
    /// Drain at a Promise checkpoint, after queued catch handlers had a turn.
    /// Diagnostic string conversion stays under the existing engine allowance.
    pub fn take_unhandled_rejections(&mut self) -> Vec<String> {
        let mut result = Vec::new();
        for _ in 0..32 {
            let Ok(started) = self.begin_call() else {
                break;
            };
            let status = unsafe { hull_quickjs_rejection_pop(self.native.as_ptr()) };
            if status == 0 {
                break;
            }
            if let Err(error) = self.finish_call(started, -1) {
                result.push(error);
            }
        }
        result
    }
    pub fn complete_import(&mut self, id: u64, error: Option<&str>) -> Result<(), String> {
        let error = error
            .map(CString::new)
            .transpose()
            .map_err(|_| "Module error contains NUL")?;
        let started = self.begin_call()?;
        let status = unsafe {
            hull_quickjs_import_complete(
                self.native.as_ptr(),
                id,
                error.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            )
        };
        self.finish_call(started, status)
    }

    pub fn execution_budget_remaining(&self) -> Option<Duration> {
        self.budget_remaining.get()
    }

    pub fn execution_budget_exhausted(&self) -> bool {
        self.budget_remaining
            .get()
            .is_some_and(|remaining| remaining.is_zero())
    }

    pub fn take_console(&mut self) -> ConsoleBatch {
        let mut batch = ConsoleBatch::default();
        let mut bytes = [0u8; 4096];
        let mut level = 0;
        loop {
            // SAFETY: pop copies at most the supplied capacity; no JS runs here.
            let length = unsafe {
                hull_quickjs_console_pop(
                    self.native.as_ptr(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    &mut level,
                )
            };
            if length < 0 {
                break;
            }
            let mut text = String::from_utf8_lossy(&bytes[..length as usize]).into_owned();
            // QuickJS can emit lone UTF-16 surrogates as invalid UTF-8. Lossy
            // replacement may expand bytes, so enforce the text limit again.
            if text.len() > bytes.len() {
                let mut end = bytes.len();
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            batch.messages.push(ConsoleMessage {
                level: match level {
                    1 => ConsoleLevel::Debug,
                    2 => ConsoleLevel::Info,
                    3 => ConsoleLevel::Warn,
                    4 => ConsoleLevel::Error,
                    _ => ConsoleLevel::Log,
                },
                text,
            });
        }
        // SAFETY: reads and resets an owned native counter without executing JS.
        batch.dropped = unsafe { hull_quickjs_console_dropped(self.native.as_ptr()) };
        batch
    }
    fn begin_call(&self) -> Result<Instant, String> {
        let allowance = self
            .budget_remaining
            .get()
            .map_or(self.limits.time_slice, |remaining| {
                remaining.min(self.limits.time_slice)
            });
        if allowance.is_zero() {
            return Err("JavaScript execution budget exhausted".into());
        }
        let started = Instant::now();
        self.interrupt.deadline.set(started + allowance);
        Ok(started)
    }
    fn finish_call(&self, started: Instant, status: c_int) -> Result<(), String> {
        let result = if status < 0 {
            Err(self.exception(started))
        } else {
            Ok(())
        };
        if let Some(remaining) = self.budget_remaining.get() {
            self.budget_remaining
                .set(Some(remaining.saturating_sub(started.elapsed())));
        }
        if result.is_ok() && self.execution_budget_exhausted() {
            Err("JavaScript execution budget exhausted".into())
        } else {
            result
        }
    }
    fn exception(&self, started: Instant) -> String {
        // Error conversion can invoke user code. Give diagnostics a separate,
        // small cooperative allowance rather than another full evaluation slice.
        let diagnostic = self.limits.time_slice.min(Duration::from_millis(1));
        let diagnostic = self.budget_remaining.get().map_or(diagnostic, |remaining| {
            diagnostic.min(remaining.saturating_sub(started.elapsed()))
        });
        if diagnostic.is_zero() {
            // No guest coercion is allowed to obtain another execution slice.
            unsafe { hull_quickjs_discard_exception(self.native.as_ptr()) };
            return "JavaScript execution budget exhausted".into();
        }
        self.interrupt.deadline.set(Instant::now() + diagnostic);
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
        let started = self.begin_call()?;
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
        self.finish_call(started, status)
    }
    /// Pin an explicitly installed global function as the native message receiver.
    /// The configurable global property is removed after validation, so later
    /// page assignments cannot replace the pinned transport. This method does
    /// not install a receiver or evaluate any bootstrap code on its own.
    pub fn bind_message_handler(&mut self, global_name: &str) -> Result<(), String> {
        let name =
            CString::new(global_name).map_err(|_| "QuickJS handler name contains a NUL byte")?;
        let started = self.begin_call()?;
        // SAFETY: the adapter owns all native values, including the retained
        // function, and the property name stays live for the synchronous call.
        let status =
            unsafe { hull_quickjs_bind_message_handler(self.native.as_ptr(), name.as_ptr()) };
        self.finish_call(started, status)
    }
    /// Parse a JSON message as data and call the pinned native receiver once.
    /// Payload bytes are never interpolated into executable JavaScript source.
    /// Callback completion values are discarded without coercion; pending jobs
    /// remain explicit through `execute_pending_job`.
    pub fn dispatch_json(&mut self, json: &[u8]) -> Result<(), String> {
        let mut terminated = Vec::with_capacity(json.len() + 1);
        terminated.extend_from_slice(json);
        terminated.push(0);
        let started = self.begin_call()?;
        // SAFETY: the native JSON parser receives the exact byte length and a
        // live NUL-terminated buffer; all parsed values stay inside the adapter.
        let status = unsafe {
            hull_quickjs_dispatch_json(self.native.as_ptr(), terminated.as_ptr().cast(), json.len())
        };
        self.finish_call(started, status)
    }
    /// Execute at most one pending promise job. A caller can yield its local
    /// Tokio task between jobs instead of draining a page's queue synchronously.
    pub fn execute_pending_job(&mut self) -> Result<bool, String> {
        let started = self.begin_call()?;
        // SAFETY: the adapter owns every context and transfers exceptions back
        // into the engine's context before returning an error status.
        let status = unsafe { hull_quickjs_pending_job(self.native.as_ptr()) };
        self.finish_call(started, status)?;
        Ok(status != 0)
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

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn native_values_preserve_high_half_heap_pointers() {
        unsafe extern "C" {
            fn hull_quickjs_pointer_round_trip(bits: usize) -> usize;
        }
        // Includes the actual corrupted object address from the baremetal crash.
        for pointer in [
            0xffff_8001_0d88_2dc0,
            0xffff_8001_0d83_9710,
            0x0000_7fff_1234_5678,
        ] {
            // SAFETY: the adapter packs/unpacks this bit pattern without access.
            assert_eq!(unsafe { hull_quickjs_pointer_round_trip(pointer) }, pointer);
        }
    }

    #[test]
    fn dispatched_json_is_data_and_completion_is_not_coerced() {
        let mut engine = Engine::new().unwrap();
        engine.eval(br#"globalThis.receive = data => { globalThis.message = data; return { toString() { throw Error('completion coerced'); } }; }"#, "bridge.js", false).unwrap();
        engine.bind_message_handler("receive").unwrap();
        engine.dispatch_json(br#"{"text":"\"; globalThis.injected=true; // \\ \u2028","nested":{"__proto__":{"injected":true}},"list":[1,true,null]}"#).unwrap();
        engine.eval(br#"if (typeof injected !== 'undefined' || Object.prototype.injected !== undefined || message.list[0] !== 1 || message.list[2] !== null || !message.text.includes('globalThis.injected=true')) throw Error('JSON was not data')"#, "verify.js", false).unwrap();
    }
    #[test]
    fn pinned_receiver_survives_removed_or_overwritten_global() {
        let mut engine = Engine::new().unwrap();
        engine
            .eval(
                b"globalThis.calls=0; globalThis.receive=data=>calls+=data",
                "bridge.js",
                false,
            )
            .unwrap();
        engine.bind_message_handler("receive").unwrap();
        engine.eval(b"if (Object.hasOwn(globalThis,'receive')) throw Error('global remained'); globalThis.receive=()=>{throw Error('hijacked')}", "overwrite.js", false).unwrap();
        engine.dispatch_json(b"2").unwrap();
        engine
            .eval(
                b"delete globalThis.receive; if(calls!==2)throw Error('wrong receiver')",
                "remove.js",
                false,
            )
            .unwrap();
        engine.dispatch_json(b"3").unwrap();
        engine
            .eval(
                b"if(calls!==5)throw Error('receiver not retained')",
                "verify.js",
                false,
            )
            .unwrap();
    }
    #[test]
    fn receiver_errors_are_visible_and_later_dispatch_recovers() {
        let mut engine = Engine::new().unwrap();
        engine.eval(b"globalThis.receive=data=>{if(data.fail)throw Error('receiver failure');globalThis.received=data.value;}", "bridge-error.js", false).unwrap();
        engine.bind_message_handler("receive").unwrap();
        let error = engine.dispatch_json(br#"{"fail":true}"#).unwrap_err();
        assert!(error.contains("receiver failure"), "{error}");
        assert!(error.contains("bridge-error.js"), "{error}");
        engine.dispatch_json(br#"{"value":42}"#).unwrap();
        engine
            .eval(
                b"if(received!==42)throw Error('not recovered')",
                "verify.js",
                false,
            )
            .unwrap();
    }
    #[test]
    fn missing_receiver_bad_json_and_failed_rebinding_are_explicit() {
        let mut engine = Engine::new().unwrap();
        assert!(
            engine
                .dispatch_json(b"{}")
                .unwrap_err()
                .contains("not bound")
        );
        assert!(
            engine
                .bind_message_handler("missing")
                .unwrap_err()
                .contains("must be a function")
        );
        engine
            .eval(
                b"globalThis.receive=data=>globalThis.received=data",
                "bridge.js",
                false,
            )
            .unwrap();
        engine.bind_message_handler("receive").unwrap();
        assert!(
            engine
                .dispatch_json(b"{malformed")
                .unwrap_err()
                .contains("SyntaxError")
        );
        engine.eval(b"Object.defineProperty(globalThis,'locked',{value:()=>{throw Error('wrong receiver')},configurable:false})", "locked.js", false).unwrap();
        assert!(engine.bind_message_handler("locked").is_err());
        engine.dispatch_json(b"42").unwrap();
        engine
            .eval(
                b"if(received!==42)throw Error('failed rebind lost previous receiver')",
                "verify.js",
                false,
            )
            .unwrap();
    }

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
    fn unavailable_exception_message_replaces_previous_diagnostic_and_recovers() {
        let mut engine = Engine::new().unwrap();
        // Prime the reusable buffer so a missing terminator would expose stale
        // message or stack bytes after the shorter fixed fallback.
        let previous = engine
            .eval(b"throw new Error('x'.repeat(200))", "previous.js", false)
            .unwrap_err();
        assert!(previous.len() > 100);
        let error = engine
            .eval(
                b"throw { toString() { throw 0; }, get stack() { throw 0; } }",
                "unprintable.js",
                false,
            )
            .unwrap_err();
        assert_eq!(error, "JavaScript exception (message unavailable)");
        engine.eval(b"1 + 1", "recovery.js", false).unwrap();
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
