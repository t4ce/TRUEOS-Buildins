//! Native attached-terminal transport. None of these operations discovers or
//! configures Unix descriptors: the Blueprint terminal lease owns the route.

use std::io::{self, Write};

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

pub(crate) fn log_error(message: std::fmt::Arguments<'_>) {
    let message = message.to_string();
    let target = b"crossterm";
    unsafe {
        trueos_cabi_log(1, target.as_ptr(), target.len(), message.as_ptr(), message.len());
    }
}

pub(crate) fn error(operation: &'static str, code: i32) -> io::Error {
    io::Error::other(format!("TRUEOS {operation} failed ({code})"))
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
    let len = unsafe { trueos_cabi_env_var(key.as_ptr(), key.len(), std::ptr::null_mut(), 0) };
    if len < 0 { return None; }
    let mut bytes = vec![0; len as usize];
    let count = unsafe {
        trueos_cabi_env_var(key.as_ptr(), key.len(), bytes.as_mut_ptr(), bytes.len())
    };
    if count < 0 || count as usize > bytes.len() { return None; }
    bytes.truncate(count as usize);
    String::from_utf8(bytes).ok()
}

pub(crate) struct Output;

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = unsafe { trueos_cabi_shell_attached_write(bytes.as_ptr(), bytes.len()) };
        if count > bytes.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid TRUEOS terminal write count"));
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::{collections::VecDeque, sync::Mutex};

    pub(crate) static SERIAL: Mutex<()> = Mutex::new(());
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
    extern "C" fn trueos_cabi_shell_attached_wait_after_v1(_: u32, _: u64) -> i32 {
        STATE.lock().unwrap().wait_status
    }

    #[unsafe(no_mangle)]
    extern "C" fn trueos_cabi_shell_attached_wake_v1() -> i32 {
        STATE.lock().unwrap().generation += 1;
        0
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_shell_attached_write(bytes: *const u8, len: usize) -> usize {
        let mut state = STATE.lock().unwrap();
        let count = len.min(state.write_limit);
        state.output.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, count) });
        count
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_env_var(key: *const u8, key_len: usize, out: *mut u8, cap: usize) -> isize {
        let key = std::str::from_utf8(unsafe { std::slice::from_raw_parts(key, key_len) }).unwrap();
        let Ok(value) = std::env::var(key) else { return -1; };
        if cap != 0 {
            let count = value.len().min(cap);
            unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), out, count); }
        }
        value.len() as isize
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_log(level: u32, target: *const u8, target_len: usize, message: *const u8, message_len: usize) -> i32 {
        let target = unsafe { std::slice::from_raw_parts(target, target_len) }.to_vec();
        let message = unsafe { std::slice::from_raw_parts(message, message_len) }.to_vec();
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
        use std::io::Write;
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
