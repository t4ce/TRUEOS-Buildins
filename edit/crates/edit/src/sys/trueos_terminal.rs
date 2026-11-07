// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The launch-attached terminal supplies input, ownership and geometry directly.
//! This state, like Edit's Unix terminal state, belongs to the single editor loop.
#![allow(static_mut_refs)]

use std::fs::File;
use std::{io, time};

use stdext::arena::{Arena, scratch_arena};
use stdext::arena_format;
use stdext::collections::{BString, BVec};

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
}

struct State {
    generation: u64,
    columns: u16,
    rows: u16,
    inject_resize: bool,
    restore_terminal: bool,
    utf8_buf: [u8; 4],
    utf8_len: usize,
}

static mut STATE: State = State {
    generation: 0,
    columns: 0,
    rows: 0,
    inject_resize: false,
    restore_terminal: false,
    utf8_buf: [0; 4],
    utf8_len: 0,
};

// RestoreModes owns escape-mode cleanup; Blueprint lifecycle owns the lease.
// No Unix terminal-mode snapshot needs to survive until this guard is dropped.
pub struct Deinit;

pub fn init() -> Deinit {
    Deinit
}

pub fn reopen_stdin_if_redirected() -> io::Result<Option<File>> {
    // TRUEOS's compatibility isatty(0) is unconditionally true. Its attached
    // console has no /dev/tty rediscovery or redirected descriptor to reopen.
    Ok(None)
}

pub fn switch_modes() -> io::Result<()> {
    let mut epoch = 0;
    let code = unsafe { trueos_cabi_blueprint_terminal_lease_current_v1(0, &mut epoch) };
    if code == 0 && epoch != 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!("TRUEOS terminal claim failed ({code})")))
    }
}

fn check_terminal_resize() -> bool {
    let (mut generation, mut columns, mut rows) = (0, 0, 0);
    let code = unsafe {
        trueos_cabi_blueprint_terminal_surface_snapshot_v1(&mut generation, &mut columns, &mut rows)
    };
    match code {
        0 if generation != 0 && columns != 0 && rows != 0 => {
            unsafe {
                if generation != STATE.generation {
                    STATE.restore_terminal |= STATE.generation != 0;
                    STATE.inject_resize = true;
                    // A pending UTF-8 prefix belongs to its previous surface.
                    STATE.utf8_len = 0;
                } else if columns.min(u16::MAX as u32) as u16 != STATE.columns
                    || rows.min(u16::MAX as u32) as u16 != STATE.rows
                {
                    STATE.inject_resize = true;
                }
                STATE.generation = generation;
                STATE.columns = columns.min(u16::MAX as u32) as u16;
                STATE.rows = rows.min(u16::MAX as u32) as u16;
            }
            true
        }
        // Keep the last good surface while parked or detached. This query also
        // acknowledges explicit reentry when the terminal becomes active again.
        -5..=-1 => true,
        _ => false,
    }
}

pub fn take_terminal_restore() -> bool {
    unsafe { std::mem::replace(&mut STATE.restore_terminal, false) }
}

pub fn inject_window_size_into_stdin() {
    unsafe {
        STATE.inject_resize = true;
    }
}

fn wait_millis(timeout: time::Duration) -> u64 {
    if timeout == time::Duration::MAX {
        u64::MAX
    } else {
        timeout.as_nanos().div_ceil(1_000_000).min(u128::from(u64::MAX - 1)) as u64
    }
}

/// Returns None on transport failure, an empty string on timeout, or input.
/// Resize uses the existing parser's CSI 8;rows;columns t notification.
pub fn read_stdin(arena: &Arena, timeout: time::Duration) -> Option<BString<'_>> {
    let started = time::Instant::now();
    let mut buf = BVec::empty();
    buf.reserve(arena, 4 * 1024);

    loop {
        // Observe BEFORE both readiness probes: neither input nor a surface
        // change racing the probes and native parking can be lost.
        let observed = unsafe { trueos_cabi_shell_attached_observe_v1() };
        if !check_terminal_resize() {
            return None;
        }
        let prefix = unsafe { STATE.utf8_len };
        let spare = buf.spare_capacity_mut();
        let capacity = spare.len() - prefix;
        let count = unsafe {
            trueos_cabi_shell_attached_read_v1(spare.as_mut_ptr().add(prefix).cast(), capacity)
        };
        if count < 0 || count as usize > capacity {
            return None;
        }
        let expired = timeout != time::Duration::MAX && started.elapsed() >= timeout;
        if count != 0 || unsafe { STATE.inject_resize } || expired {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    STATE.utf8_buf.as_ptr(),
                    spare.as_mut_ptr().cast(),
                    prefix,
                );
                buf.set_len(prefix + count as usize);
                STATE.utf8_len = 0;
            }
            break;
        }

        let remaining = if timeout == time::Duration::MAX {
            timeout
        } else {
            timeout.saturating_sub(started.elapsed())
        };
        let code =
            unsafe { trueos_cabi_shell_attached_wait_after_v1(observed, wait_millis(remaining)) };
        if code < 0 {
            return None;
        }
        // An empty read is not EOF. Reprobe after timeout or spurious wake.
    }

    if !buf.is_empty() {
        // Preserve Edit's incomplete UTF-8 carry across input batches.
        let lim = buf.len().saturating_sub(3);
        let mut off = buf.len() - 1;
        while off > lim && buf[off] & 0b1100_0000 == 0b1000_0000 {
            off -= 1;
        }
        let seq_len = match buf[off] {
            b if b & 0b1000_0000 == 0 => 1,
            b if b & 0b1110_0000 == 0b1100_0000 => 2,
            b if b & 0b1111_0000 == 0b1110_0000 => 3,
            b if b & 0b1111_1000 == 0b1111_0000 => 4,
            _ => 0,
        };
        if off + seq_len > buf.len() {
            unsafe {
                STATE.utf8_len = buf.len() - off;
                STATE.utf8_buf[..STATE.utf8_len].copy_from_slice(&buf[off..]);
            }
            buf.truncate(off);
        }
    }

    let mut result = BString::from_utf8_lossy(arena, buf);
    unsafe {
        if std::mem::replace(&mut STATE.inject_resize, false)
            && STATE.columns != 0
            && STATE.rows != 0
        {
            let scratch = scratch_arena(Some(arena));
            let (w, h) = (STATE.columns, STATE.rows);
            let sequence = arena_format!(&*scratch, "\x1b[8;{h};{w}t");
            result.replace_range(arena, 0..0, &sequence);
        }
    }
    Some(result)
}
