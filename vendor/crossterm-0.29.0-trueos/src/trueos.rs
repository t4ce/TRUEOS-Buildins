//! Native attached-terminal transport. None of these operations discovers or
//! configures Unix descriptors: the Blueprint terminal lease owns the route.

#[allow(unused_imports)]
use crate::alloc_prelude::*;
use crate::io::{self, Write};

unsafe extern "C" {
    fn trueos_cabi_blueprint_terminal_lease_current_v1(expected: u64, epoch: *mut u64) -> i32;
    fn trueos_cabi_blueprint_terminal_surface_snapshot_v1(
        generation: *mut u64,
        columns: *mut u32,
        rows: *mut u32,
    ) -> i32;
    fn trueos_cabi_shell_attached_read_v1(out: *mut u8, capacity: usize) -> isize;
    fn trueos_cabi_shell_attached_observe_v1() -> u32;
    fn trueos_cabi_shell_attached_wait_after_v1(observed: u32, timeout_ms: u64) -> i32;
    #[cfg(feature = "event-stream")]
    fn trueos_cabi_shell_attached_wake_v1() -> i32;
    fn trueos_cabi_shell_attached_write(bytes: *const u8, len: usize) -> usize;
    fn trueos_cabi_env_var(key: *const u8, key_len: usize, out: *mut u8, capacity: usize) -> isize;
    fn trueos_cabi_log(level: u32, target: *const u8, target_len: usize, message: *const u8, message_len: usize) -> i32;
}

pub(crate) fn log_error(message: core::fmt::Arguments<'_>) {
    let message = message.to_string();
    let target = b"crossterm";
    unsafe {
        trueos_cabi_log(1, target.as_ptr(), target.len(), message.as_ptr(), message.len());
    }
}

pub(crate) fn error(operation: &'static str, code: i32) -> io::Error {
    #[cfg(not(feature = "native-core"))]
    { io::Error::other(format!("TRUEOS {operation} failed ({code})")) }
    #[cfg(feature = "native-core")]
    {
        log_error(format_args!("TRUEOS {operation} failed ({code})"));
        io::Error::new(io::ErrorKind::Other, operation)
    }
}

pub(crate) fn claim() -> io::Result<()> {
    let mut epoch = 0;
    let code = unsafe { trueos_cabi_blueprint_terminal_lease_current_v1(0, &mut epoch) };
    if code == 0 && epoch != 0 {
        Ok(())
    } else {
        Err(error("terminal claim", code))
    }
}

pub(crate) fn surface() -> io::Result<Option<(u64, u32, u32)>> {
    let (mut generation, mut columns, mut rows) = (0, 0, 0);
    let code = unsafe {
        trueos_cabi_blueprint_terminal_surface_snapshot_v1(&mut generation, &mut columns, &mut rows)
    };
    match code {
        0 if generation != 0 && columns != 0 && rows != 0 => {
            Ok(Some((generation, columns, rows)))
        }
        // A source survives release, parking, and attachment changes. Retain
        // its last snapshot until a new active surface is available.
        -5..=-1 => Ok(None),
        0 => Err(io::Error::new(io::ErrorKind::InvalidData, "invalid TRUEOS terminal surface")),
        _ => Err(error("terminal surface", code)),
    }
}

pub(crate) fn read(out: &mut [u8]) -> io::Result<usize> {
    let count = unsafe { trueos_cabi_shell_attached_read_v1(out.as_mut_ptr(), out.len()) };
    if count < 0 {
        Err(error("terminal read", count as i32))
    } else if count as usize > out.len() {
        Err(io::Error::new(io::ErrorKind::InvalidData, "invalid TRUEOS terminal read count"))
    } else {
        Ok(count as usize)
    }
}

pub(crate) fn observe() -> u32 {
    unsafe { trueos_cabi_shell_attached_observe_v1() }
}

pub(crate) fn wait_after(observed: u32, timeout_ms: u64) -> io::Result<()> {
    let code = unsafe { trueos_cabi_shell_attached_wait_after_v1(observed, timeout_ms) };
    if code < 0 { Err(error("terminal wait", code)) } else { Ok(()) }
}

#[cfg(feature = "event-stream")]
pub(crate) fn wake() -> io::Result<()> {
    let code = unsafe { trueos_cabi_shell_attached_wake_v1() };
    if code < 0 { Err(error("terminal wake", code)) } else { Ok(()) }
}

pub(crate) fn env_var(key: &str) -> Option<String> {
    let len = unsafe { trueos_cabi_env_var(key.as_ptr(), key.len(), core::ptr::null_mut(), 0) };
    if len < 0 { return None; }
    let mut bytes = vec![0; len as usize];
    let count = unsafe {
        trueos_cabi_env_var(key.as_ptr(), key.len(), bytes.as_mut_ptr(), bytes.len())
    };
    if count < 0 || count as usize > bytes.len() { return None; }
    bytes.truncate(count as usize);
    String::from_utf8(bytes).ok()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Output;

fn write_output(bytes: &[u8]) -> io::Result<usize> {
    let count = unsafe { trueos_cabi_shell_attached_write(bytes.as_ptr(), bytes.len()) };
    if count > bytes.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid TRUEOS terminal write count"));
    }
    Ok(count)
}

#[cfg(feature = "native-core")]
static OUTPUT_LOCK: crate::platform::Mutex<()> = crate::platform::Mutex::new(());

#[cfg(feature = "native-core")]
pub struct OutputLock<'a> { _guard: crate::platform::MutexGuard<'a, ()> }

#[cfg(feature = "native-core")]
impl Output {
    pub fn lock(&self) -> OutputLock<'static> { OutputLock { _guard: OUTPUT_LOCK.lock() } }
}

#[cfg(feature = "native-core")]
impl Write for OutputLock<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> { write_output(bytes) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(feature = "native-core")]
        let _guard = OUTPUT_LOCK.lock();
        write_output(bytes)
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

#[cfg(test)]
pub(crate) mod test_support {
    #[allow(unused_imports)]
    use crate::alloc_prelude::*;
    use std::{collections::VecDeque, sync::Mutex};

    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());
    #[cfg(feature = "native-core")]
    pub(crate) static ATTACHED_READY: std::sync::Condvar = std::sync::Condvar::new();
    pub(crate) struct State {
        pub claim_status: i32,
        pub surface_status: i32,
        pub surface: (u64, u32, u32),
        pub generation: u32,
        pub bytes: VecDeque<u8>,
        pub output: Vec<u8>,
        pub write_limit: usize,
        pub wait_status: i32,
        pub logs: Vec<(u32, Vec<u8>, Vec<u8>)>,
    }

    pub(crate) static STATE: Mutex<State> = Mutex::new(State {
        claim_status: 0, surface_status: 0, surface: (1, 120, 40),
        generation: 1, bytes: VecDeque::new(), output: Vec::new(),
        write_limit: usize::MAX, wait_status: 0, logs: Vec::new(),
    });

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_blueprint_terminal_lease_current_v1(_: u64, epoch: *mut u64) -> i32 {
        let state = STATE.lock().unwrap();
        if state.claim_status == 0 { unsafe { epoch.write(state.surface.0); } }
        state.claim_status
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_blueprint_terminal_surface_snapshot_v1(
        generation: *mut u64, columns: *mut u32, rows: *mut u32,
    ) -> i32 {
        let state = STATE.lock().unwrap();
        if state.surface_status == 0 {
            unsafe { generation.write(state.surface.0); columns.write(state.surface.1); rows.write(state.surface.2); }
        }
        state.surface_status
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_shell_attached_read_v1(out: *mut u8, cap: usize) -> isize {
        let mut state = STATE.lock().unwrap();
        let count = cap.min(state.bytes.len());
        for index in 0..count { unsafe { out.add(index).write(state.bytes.pop_front().unwrap()); } }
        count as isize
    }

    #[unsafe(no_mangle)]
    extern "C" fn trueos_cabi_shell_attached_observe_v1() -> u32 { STATE.lock().unwrap().generation }

    #[unsafe(no_mangle)]
    extern "C" fn trueos_cabi_shell_attached_wait_after_v1(observed: u32, timeout_ms: u64) -> i32 {
        let state = STATE.lock().unwrap();
        #[cfg(not(feature = "native-core"))]
        { let _ = (observed, timeout_ms); state.wait_status }
        #[cfg(feature = "native-core")]
        {
            if state.wait_status != 0 || state.generation != observed || timeout_ms == 0 {
                return state.wait_status;
            }
            if timeout_ms == u64::MAX {
                ATTACHED_READY.wait_while(state, |s| s.generation == observed).unwrap().wait_status
            } else {
                ATTACHED_READY.wait_timeout_while(state, std::time::Duration::from_millis(timeout_ms),
                    |s| s.generation == observed).unwrap().0.wait_status
            }
        }
    }

    #[unsafe(no_mangle)]
    extern "C" fn trueos_cabi_shell_attached_wake_v1() -> i32 {
        STATE.lock().unwrap().generation += 1;
        #[cfg(feature = "native-core")]
        ATTACHED_READY.notify_all();
        0
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_shell_attached_write(bytes: *const u8, len: usize) -> usize {
        let mut state = STATE.lock().unwrap();
        let count = len.min(state.write_limit);
        state.output.extend_from_slice(unsafe { core::slice::from_raw_parts(bytes, count) });
        count
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_env_var(key: *const u8, key_len: usize, out: *mut u8, cap: usize) -> isize {
        let key = core::str::from_utf8(unsafe { core::slice::from_raw_parts(key, key_len) }).unwrap();
        let Ok(value) = std::env::var(key) else { return -1; };
        if cap != 0 {
            let count = value.len().min(cap);
            unsafe { core::ptr::copy_nonoverlapping(value.as_ptr(), out, count); }
        }
        value.len() as isize
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_log(level: u32, target: *const u8, target_len: usize, message: *const u8, message_len: usize) -> i32 {
        let target = unsafe { core::slice::from_raw_parts(target, target_len) }.to_vec();
        let message = unsafe { core::slice::from_raw_parts(message, message_len) }.to_vec();
        STATE.lock().unwrap().logs.push((level, target, message));
        0
    }

    #[test]
    fn initialization_diagnostics_keep_the_error_in_native_logging() {
        let _serial = SERIAL.lock().unwrap();
        STATE.lock().unwrap().logs.clear();
        super::log_error(format_args!("input source initialization failed: {}", "not attached"));
        assert_eq!(STATE.lock().unwrap().logs,
            vec![(1, b"crossterm".to_vec(), b"input source initialization failed: not attached".to_vec())]);
    }

    #[test]
    fn native_batch_read_output_and_wait_error_contracts() {
        use crate::io::Write;
        let _serial = SERIAL.lock().unwrap();
        STATE.lock().unwrap().bytes.extend(b"batch");
        let mut bytes = [0; 1024];
        assert_eq!(super::read(&mut bytes).unwrap(), 5);
        assert_eq!(&bytes[..5], b"batch");
        assert_eq!(super::read(&mut bytes).unwrap(), 0);
        {
            let mut state = STATE.lock().unwrap();
            state.output.clear(); state.write_limit = 3;
        }
        super::Output.write_all(b"complete output").unwrap();
        assert_eq!(STATE.lock().unwrap().output, b"complete output");
        STATE.lock().unwrap().wait_status = -3;
        assert!(super::wait_after(super::observe(), 1).is_err());
        let mut state = STATE.lock().unwrap();
        state.wait_status = 0; state.write_limit = usize::MAX;
    }
}

#[cfg(test)]
mod native_runtime_test_hooks {
    #[allow(unused_imports)]
    use crate::alloc_prelude::*;
    use alloc::{collections::BTreeMap, sync::Arc};
    use core::ffi::c_void;
    use std::sync::{Condvar, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    struct Slot { generation: Mutex<u32>, changed: Condvar }
    fn slot(key: u64) -> Arc<Slot> {
        static SLOTS: OnceLock<Mutex<BTreeMap<u64, Arc<Slot>>>> = OnceLock::new();
        SLOTS.get_or_init(|| Mutex::new(BTreeMap::new())).lock().unwrap()
            .entry(key).or_insert_with(|| Arc::new(Slot { generation: Mutex::new(0), changed: Condvar::new() })).clone()
    }
    #[unsafe(no_mangle)]
    extern "Rust" fn trueos_platform_monotonic_nanos() -> u64 {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        // Leave room for the existing deadline tests to construct an earlier instant.
        60_000_000_000 + ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }
    #[unsafe(no_mangle)]
    extern "Rust" fn trueos_tokio_platform_wait_observe(key: u64) -> u32 {
        *slot(key).generation.lock().unwrap()
    }
    #[unsafe(no_mangle)]
    extern "Rust" fn trueos_tokio_platform_wait_after(key: u64, observed: u32, timeout_ms: u64) -> bool {
        let slot = slot(key); let generation = slot.generation.lock().unwrap();
        if *generation != observed { return true; }
        if timeout_ms == 0 { return false; }
        if timeout_ms == u64::MAX {
            *slot.changed.wait_while(generation, |v| *v == observed).unwrap() != observed
        } else {
            *slot.changed.wait_timeout_while(generation, Duration::from_millis(timeout_ms),
                |v| *v == observed).unwrap().0 != observed
        }
    }
    #[unsafe(no_mangle)]
    extern "Rust" fn trueos_tokio_platform_wake_all(key: u64) -> usize {
        let slot = slot(key); let mut generation = slot.generation.lock().unwrap();
        *generation = generation.wrapping_add(1); drop(generation);
        slot.changed.notify_all(); 0
    }
    #[unsafe(no_mangle)]
    extern "Rust" fn trueos_tokio_platform_wake_one(key: u64) -> bool {
        let slot = slot(key); let mut generation = slot.generation.lock().unwrap();
        *generation = generation.wrapping_add(1); drop(generation);
        slot.changed.notify_one(); true
    }

    #[cfg(feature = "event-stream")]
    fn threads() -> &'static Mutex<BTreeMap<usize, std::thread::JoinHandle<usize>>> {
        static THREADS: OnceLock<Mutex<BTreeMap<usize, std::thread::JoinHandle<usize>>>> = OnceLock::new();
        THREADS.get_or_init(|| Mutex::new(BTreeMap::new()))
    }
    #[cfg(feature = "event-stream")]
    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_thread_spawn(
        stack: usize, entry: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
        arg: *mut c_void, out: *mut usize,
    ) -> i32 {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed); let arg = arg as usize;
        let thread = std::thread::Builder::new().stack_size(stack)
            .spawn(move || unsafe { entry(arg as *mut c_void) as usize }).unwrap();
        threads().lock().unwrap().insert(id, thread);
        unsafe { out.write(id); } 0
    }
    #[cfg(feature = "event-stream")]
    #[unsafe(no_mangle)]
    extern "C" fn trueos_cabi_thread_join(handle: usize) -> i32 {
        let Some(thread) = threads().lock().unwrap().remove(&handle) else { return 22; };
        thread.join().unwrap(); 0
    }
    #[cfg(feature = "event-stream")]
    #[unsafe(no_mangle)]
    extern "C" fn trueos_cabi_thread_detach(handle: usize) -> i32 {
        if threads().lock().unwrap().remove(&handle).is_some() { 0 } else { 22 }
    }
}
