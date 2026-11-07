//! Blocking console/child-stream primitives for Tokio's existing async bridge.
//!
//! No descriptor allocation, errno slot, termios state, or Unix handle is used.
//! Empty attached-console input is temporary unavailability, never EOF. Like
//! Tokio's hosted stdin bridge, an already-running blocking read is not cancelled
//! when its async reader is dropped.
use crate::platform::io::{self, Read, Write};

unsafe extern "C" {
    fn trueos_cabi_write(stream: u32, data: *const u8, len: usize);
    fn trueos_cabi_shell_attached_read_v1(out: *mut u8, cap: usize) -> isize;
    fn trueos_cabi_shell_attached_observe_v1() -> u32;
    fn trueos_cabi_shell_attached_wait_after_v1(observed: u32, timeout_ms: u64) -> i32;
    fn trueos_cabi_blueprint_terminal_lease_current_v1(
        ready_epoch: u64,
        out_epoch: *mut u64,
    ) -> i32;
    fn trueos_cabi_blueprint_child_read_v1(handle: u64, out: *mut u8, cap: usize) -> isize;
    fn trueos_cabi_blueprint_child_send_v1(handle: u64, data: *const u8, len: usize) -> isize;
    fn trueos_cabi_blueprint_child_status_v1(handle: u64) -> i32;
}

#[derive(Debug)]
pub(crate) struct Stdin;
#[derive(Debug)]
pub(crate) struct Stdout;
#[derive(Debug)]
pub(crate) struct Stderr;

pub(crate) fn stdin() -> Stdin {
    Stdin
}
pub(crate) fn stdout() -> Stdout {
    Stdout
}
pub(crate) fn stderr() -> Stderr {
    Stderr
}

fn child_stdio() -> bool {
    crate::platform::env::var("TRUEOS_MCP_STDIO")
        .ok()
        .as_deref()
        == Some("1")
}

impl Read for Stdin {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if child_stdio() {
            loop {
                let count =
                    unsafe { trueos_cabi_blueprint_child_read_v1(0, out.as_mut_ptr(), out.len()) };
                if count > 0 {
                    return checked_count(count as usize, out.len());
                }
                // A child stream's negative read or terminal status means EOF,
                // matching the existing native child pipe contract.
                if count < 0 || unsafe { trueos_cabi_blueprint_child_status_v1(0) } >= 4 {
                    return Ok(0);
                }
                crate::platform::thread::sleep(core::time::Duration::from_millis(10));
            }
        }
        // Reads, rather than transcript writes, request the terminal lease.
        // Do not cache the epoch across parking/reentry.
        let mut epoch = 0;
        unsafe {
            trueos_cabi_blueprint_terminal_lease_current_v1(0, &mut epoch);
        }
        loop {
            // Observe before probing; input between probe and wait is retained.
            let observed = unsafe { trueos_cabi_shell_attached_observe_v1() };
            let count = unsafe { trueos_cabi_shell_attached_read_v1(out.as_mut_ptr(), out.len()) };
            if count > 0 {
                return checked_count(count as usize, out.len());
            }
            if count < 0 {
                let kind = if count == -2 {
                    io::ErrorKind::NotConnected
                } else {
                    io::ErrorKind::Other
                };
                return Err(io::Error::new(kind, "native attached terminal read failed"));
            }
            if unsafe { trueos_cabi_shell_attached_wait_after_v1(observed, u64::MAX) } < 0 {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "native attached terminal wait failed",
                ));
            }
        }
    }
}

fn checked_count(count: usize, capacity: usize) -> io::Result<usize> {
    if count <= capacity {
        Ok(count)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native stream returned an invalid byte count",
        ))
    }
}

impl Write for Stdout {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if child_stdio() {
            // Stay below the kernel's VM communication payload capacity.
            let bytes = &bytes[..bytes.len().min(64 * 1024)];
            loop {
                let count =
                    unsafe { trueos_cabi_blueprint_child_send_v1(0, bytes.as_ptr(), bytes.len()) };
                if count >= 0 {
                    return checked_count(count as usize, bytes.len());
                }
                if count != -3 {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "native child stdout is closed",
                    ));
                }
                crate::platform::thread::sleep(core::time::Duration::from_millis(10));
            }
        }
        unsafe {
            trueos_cabi_write(1, bytes.as_ptr(), bytes.len());
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Write for Stderr {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !bytes.is_empty() {
            unsafe {
                trueos_cabi_write(2, bytes.as_ptr(), bytes.len());
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
