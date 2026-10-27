// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The editor's private bindings to TrueOS's Unix compatibility exports.
//!
//! This is the x86_64 TrueOS ABI, not a portable Unix binding. Keep these layouts
//! in sync with TRUEOS/src/unix_abi_shim.rs. File metadata goes through Rust std.
#![allow(non_camel_case_types)]

pub use std::ffi::c_int;
use std::ffi::{c_char, c_void};

#[cfg(not(all(target_arch = "x86_64", target_env = "musl")))]
compile_error!("the editor's TrueOS ABI bindings require x86_64/musl");

#[repr(C)]
#[derive(Clone, Copy)]
pub struct termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    padding: [u8; 3],
    c_ispeed: u32,
    c_ospeed: u32,
    // TrueOS tcgetattr/tcsetattr transfer 64 bytes, including this trailing word.
    reserved: u32,
}

#[repr(C)]
pub struct winsize {
    pub ws_row: u16,
    pub ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[repr(C)]
pub struct pollfd {
    pub fd: c_int,
    pub events: i16,
    pub revents: i16,
}

#[cfg(edit_icu_renaming_auto_detect)]
#[repr(C)]
pub struct Dl_info {
    pub dli_fname: *const c_char,
    dli_fbase: *mut c_void,
    dli_sname: *const c_char,
    dli_saddr: *mut c_void,
}

const _: () = {
    assert!(size_of::<termios>() == 64);
    assert!(align_of::<termios>() == 4);
    assert!(std::mem::offset_of!(termios, c_cc) == 17);
    assert!(size_of::<winsize>() == 8);
    assert!(size_of::<pollfd>() == 8);
    assert!(align_of::<pollfd>() == 4);
};

pub const STDIN_FILENO: c_int = 0;
pub const STDOUT_FILENO: c_int = 1;
pub const O_RDONLY: c_int = 0;
pub const O_NONBLOCK: c_int = 0o4000;
pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;
pub const TCSANOW: c_int = 0;
pub const TIOCGWINSZ: c_int = 0x5413;
pub const POLLIN: i16 = 1;
pub const EINTR: c_int = 4;
pub const EAGAIN: c_int = 11;
pub const ENOENT: c_int = 2;
pub const RTLD_LAZY: c_int = 1;

pub const IGNBRK: u32 = 0o000001;
pub const BRKINT: u32 = 0o000002;
pub const PARMRK: u32 = 0o000010;
pub const INPCK: u32 = 0o000020;
pub const ISTRIP: u32 = 0o000040;
pub const INLCR: u32 = 0o000100;
pub const IGNCR: u32 = 0o000200;
pub const ICRNL: u32 = 0o000400;
pub const IXON: u32 = 0o002000;
pub const OPOST: u32 = 0o000001;
pub const CSIZE: u32 = 0o000060;
pub const CS8: u32 = 0o000060;
pub const PARENB: u32 = 0o000400;
pub const ISIG: u32 = 0o000001;
pub const ICANON: u32 = 0o000002;
pub const ECHO: u32 = 0o000010;
pub const ECHONL: u32 = 0o000100;
pub const IEXTEN: u32 = 0o100000;

unsafe extern "C" {
    pub fn isatty(fd: c_int) -> c_int;
    pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    pub fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    pub fn tcgetattr(fd: c_int, out: *mut termios) -> c_int;
    pub fn tcsetattr(fd: c_int, actions: c_int, value: *const termios) -> c_int;
    pub fn ioctl(fd: c_int, request: c_int, ...) -> c_int;
    pub fn poll(fds: *mut pollfd, count: usize, timeout_ms: c_int) -> c_int;
    pub fn read(fd: c_int, out: *mut c_void, capacity: usize) -> isize;
    pub fn write(fd: c_int, bytes: *const c_void, len: usize) -> isize;
    pub fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    pub fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    #[cfg(edit_icu_renaming_auto_detect)]
    pub fn dladdr(address: *const c_void, out: *mut Dl_info) -> c_int;
}
