#!/usr/bin/env python3
"""Exercise Edit's real native input adapter and arena types on a host boundary.

The production module is copied unchanged into an isolated Cargo fixture. Two
test-only accessors reset its process-global state and expose timeout conversion;
the read/resize/UTF-8 algorithms and actual stdext arena containers are retained.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "edit/crates/edit/src/sys/trueos_terminal.rs"
STDEXT = ROOT / "edit/crates/stdext"
ACCESSORS = r'''

#[cfg(test)]
pub fn reset_for_test() {
    unsafe {
        STATE = State {
            generation: 0, columns: 0, rows: 0,
            inject_resize: false, restore_terminal: false,
            utf8_buf: [0; 4], utf8_len: 0,
        };
    }
}

#[cfg(test)]
pub fn wait_millis_for_test(timeout: time::Duration) -> u64 { wait_millis(timeout) }
'''

HARNESS = r'''
#![forbid(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]
#[path = "production.rs"] mod production;

#[cfg(test)] mod tests {
    use super::production;
    use std::{collections::VecDeque, sync::Mutex, time::{Duration, Instant}};
    use stdext::arena::Arena;

    enum Read {
        Bytes(Vec<u8>),
        Error(isize),
        Oversized,
    }
    struct Boundary {
        surface: (u64, u32, u32),
        surface_status: i32,
        claim_status: i32,
        claim_epoch: u64,
        generation: u32,
        reads: VecDeque<Read>,
        events: Vec<&'static str>,
        waits: Vec<(u32, u64, u32)>,
        wait_status: i32,
        race_after_read: Option<Vec<u8>>,
        surface_on_wait: Option<(u64, u32, u32)>,
        spurious_wakes: usize,
    }
    static BOUNDARY: Mutex<Boundary> = Mutex::new(Boundary {
        surface: (1, 120, 40), surface_status: 0,
        claim_status: 0, claim_epoch: 1, generation: 7,
        reads: VecDeque::new(), events: Vec::new(), waits: Vec::new(),
        wait_status: 0, race_after_read: None, surface_on_wait: None,
        spurious_wakes: 0,
    });

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_blueprint_terminal_lease_current_v1(
        expected: u64, epoch: *mut u64,
    ) -> i32 {
        assert_eq!(expected, 0);
        let mut b = BOUNDARY.lock().unwrap();
        b.events.push("claim");
        if b.claim_status == 0 { unsafe { epoch.write(b.claim_epoch); } }
        b.claim_status
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_blueprint_terminal_surface_snapshot_v1(
        generation: *mut u64, columns: *mut u32, rows: *mut u32,
    ) -> i32 {
        let mut b = BOUNDARY.lock().unwrap();
        b.events.push("surface");
        if b.surface_status == 0 {
            unsafe { generation.write(b.surface.0); columns.write(b.surface.1); rows.write(b.surface.2); }
        }
        b.surface_status
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn trueos_cabi_shell_attached_read_v1(out: *mut u8, capacity: usize) -> isize {
        let mut b = BOUNDARY.lock().unwrap();
        b.events.push("read");
        match b.reads.pop_front() {
            Some(Read::Bytes(bytes)) => {
                assert!(bytes.len() <= capacity);
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()); }
                bytes.len() as isize
            }
            Some(Read::Error(code)) => code,
            Some(Read::Oversized) => capacity as isize + 1,
            None => {
                // Publish immediately after the empty probe: only observing
                // before probing preserves this producer's generation.
                if let Some(bytes) = b.race_after_read.take() {
                    b.generation += 1;
                    b.reads.push_back(Read::Bytes(bytes));
                }
                0
            }
        }
    }

    #[unsafe(no_mangle)]
    extern "C" fn trueos_cabi_shell_attached_observe_v1() -> u32 {
        let mut b = BOUNDARY.lock().unwrap();
        b.events.push("observe");
        b.generation
    }

    #[unsafe(no_mangle)]
    extern "C" fn trueos_cabi_shell_attached_wait_after_v1(observed: u32, timeout_ms: u64) -> i32 {
        let mut b = BOUNDARY.lock().unwrap();
        b.events.push("wait");
        let generation = b.generation;
        b.waits.push((observed, timeout_ms, generation));
        assert!(b.waits.len() < 32, "adapter failed to make progress or respect its deadline");
        if b.wait_status < 0 { return b.wait_status; }
        if b.spurious_wakes != 0 {
            b.spurious_wakes -= 1;
            return 1;
        }
        if let Some(surface) = b.surface_on_wait.take() {
            b.surface = surface;
            b.surface_status = 0;
            b.generation += 1;
            return 1;
        }
        if generation != observed { return 1; }
        assert_ne!(timeout_ms, u64::MAX, "test boundary must arrange an infinite-wait wake");
        drop(b);
        std::thread::sleep(Duration::from_millis(timeout_ms));
        0
    }

    fn setup() -> Arena {
        production::reset_for_test();
        *BOUNDARY.lock().unwrap() = Boundary {
            surface: (1, 120, 40), surface_status: 0,
            claim_status: 0, claim_epoch: 1, generation: 7,
            reads: VecDeque::new(), events: Vec::new(), waits: Vec::new(),
            wait_status: 0, race_after_read: None, surface_on_wait: None,
            spurious_wakes: 0,
        };
        stdext::arena::init(64 * 1024).unwrap();
        Arena::new(64 * 1024).unwrap()
    }
    fn read(arena: &Arena, timeout: Duration) -> Option<String> {
        production::read_stdin(arena, timeout).map(|text| text.to_string())
    }
    fn queue(bytes: &[u8]) {
        BOUNDARY.lock().unwrap().reads.push_back(Read::Bytes(bytes.to_vec()));
    }
    fn prime(arena: &Arena) {
        assert_eq!(read(arena, Duration::ZERO).as_deref(), Some("\x1b[8;40;120t"));
        assert!(!production::take_terminal_restore());
        let mut b = BOUNDARY.lock().unwrap();
        b.events.clear(); b.waits.clear();
    }

    #[test]
    fn claim_requires_success_and_nonzero_epoch() {
        let _arena = setup();
        assert!(production::switch_modes().is_ok());
        BOUNDARY.lock().unwrap().claim_status = -3;
        assert!(production::switch_modes().is_err());
        let mut b = BOUNDARY.lock().unwrap(); b.claim_status = 0; b.claim_epoch = 0; drop(b);
        assert!(production::switch_modes().is_err());
        assert!(production::reopen_stdin_if_redirected().unwrap().is_none());
    }

    #[test]
    fn initial_resize_precedes_input_and_explicit_injection_reuses_geometry() {
        let arena = setup(); queue(b"input");
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("\x1b[8;40;120tinput"));
        assert!(!production::take_terminal_restore());
        production::inject_window_size_into_stdin();
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("\x1b[8;40;120t"));
    }

    #[test]
    fn zero_timeout_still_probes_and_returns_available_bytes() {
        let arena = setup(); prime(&arena); queue(b"ready");
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("ready"));
        let b = BOUNDARY.lock().unwrap();
        assert_eq!(b.events, ["observe", "surface", "read"]);
        assert!(b.waits.is_empty());
    }

    #[test]
    fn zero_input_is_timeout_not_eof_and_later_input_survives() {
        let arena = setup(); prime(&arena);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some(""));
        queue(b"later");
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("later"));
    }

    #[test]
    fn observes_before_surface_and_empty_read_so_racing_input_cannot_be_lost() {
        let arena = setup(); prime(&arena);
        BOUNDARY.lock().unwrap().race_after_read = Some(b"raced".to_vec());
        assert_eq!(read(&arena, Duration::MAX).as_deref(), Some("raced"));
        let b = BOUNDARY.lock().unwrap();
        assert_eq!(b.events, ["observe", "surface", "read", "wait", "observe", "surface", "read"]);
        assert_eq!(b.waits, [(7, u64::MAX, 8)]);
    }

    #[test]
    fn wake_without_input_reprobes_surface_and_restores_same_size_reentry() {
        let arena = setup(); prime(&arena);
        BOUNDARY.lock().unwrap().surface_on_wait = Some((2, 120, 40));
        assert_eq!(read(&arena, Duration::MAX).as_deref(), Some("\x1b[8;40;120t"));
        assert!(production::take_terminal_restore());
        assert!(!production::take_terminal_restore());
    }

    #[test]
    fn native_wait_error_returns_none_after_probes() {
        let arena = setup(); prime(&arena);
        BOUNDARY.lock().unwrap().wait_status = -9;
        assert!(read(&arena, Duration::MAX).is_none());
        assert_eq!(BOUNDARY.lock().unwrap().events, ["observe", "surface", "read", "wait"]);
    }

    #[test]
    fn read_transport_error_and_oversized_count_are_rejected() {
        let arena = setup(); prime(&arena);
        BOUNDARY.lock().unwrap().reads.push_back(Read::Error(-3));
        assert!(read(&arena, Duration::ZERO).is_none());
        BOUNDARY.lock().unwrap().reads.push_back(Read::Oversized);
        assert!(read(&arena, Duration::ZERO).is_none());
    }

    #[test]
    fn invalid_surface_is_rejected_before_reading_bytes() {
        let arena = setup(); prime(&arena);
        BOUNDARY.lock().unwrap().surface_status = -6;
        assert!(read(&arena, Duration::ZERO).is_none());
        assert_eq!(BOUNDARY.lock().unwrap().events, ["observe", "surface"]);
        BOUNDARY.lock().unwrap().surface_status = 0;
        BOUNDARY.lock().unwrap().surface = (0, 120, 40);
        assert!(read(&arena, Duration::ZERO).is_none());
    }

    #[test]
    fn spurious_wakes_reprobe_until_a_real_surface_change() {
        let arena = setup(); prime(&arena);
        let mut b = BOUNDARY.lock().unwrap();
        b.spurious_wakes = 2;
        b.surface_on_wait = Some((2, 120, 40));
        drop(b);
        assert_eq!(read(&arena, Duration::MAX).as_deref(), Some("\x1b[8;40;120t"));
        assert_eq!(BOUNDARY.lock().unwrap().waits.len(), 3);
        assert!(production::take_terminal_restore());
    }

    #[test]
    fn finite_wait_rounds_up_and_rechecks_deadline() {
        let arena = setup(); prime(&arena);
        let timeout = Duration::from_millis(2);
        let start = Instant::now();
        assert_eq!(read(&arena, timeout).as_deref(), Some(""));
        assert!(start.elapsed() >= timeout);
        let b = BOUNDARY.lock().unwrap();
        // A descheduled host can already have reached the deadline before
        // the first wait. Every wait actually performed must use finite ceil.
        assert!(b.waits.iter().all(|(_, ms, _)| (1..=2).contains(ms)));
    }

    #[test]
    fn millisecond_conversion_preserves_infinite_and_finite_boundaries() {
        for (duration, expected) in [
            (Duration::ZERO, 0), (Duration::from_nanos(1), 1),
            (Duration::from_micros(999), 1), (Duration::from_millis(1), 1),
            (Duration::from_nanos(1_000_001), 2),
            (Duration::from_secs(u64::MAX), u64::MAX - 1),
            (Duration::MAX, u64::MAX),
        ] {
            assert_eq!(production::wait_millis_for_test(duration), expected);
        }
    }

    #[test]
    fn split_utf8_survives_empty_polls_and_completes_without_replacement() {
        let arena = setup(); prime(&arena);
        queue(&[b'A', 0xce]);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("A"));
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some(""));
        queue(&[0xbb, 0xf0, 0x9f]);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("λ"));
        queue(&[0xa6]);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some(""));
        queue(&[0x80, b'Z']);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("🦀Z"));
    }

    #[test]
    fn parked_surface_keeps_last_geometry_and_utf8_carry() {
        let arena = setup(); prime(&arena); queue(&[0xce]);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some(""));
        BOUNDARY.lock().unwrap().surface_status = -4;
        production::inject_window_size_into_stdin();
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("\x1b[8;40;120t"));
        assert!(!production::take_terminal_restore());
        BOUNDARY.lock().unwrap().surface_status = 0; queue(&[0xbb]);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("λ"));
    }

    #[test]
    fn same_size_new_generation_resets_previous_surface_utf8_carry() {
        let arena = setup(); prime(&arena); queue(&[0xf0, 0x9f]);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some(""));
        BOUNDARY.lock().unwrap().surface = (2, 120, 40); queue(b"fresh");
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("\x1b[8;40;120tfresh"));
        assert!(production::take_terminal_restore());
        assert!(!production::take_terminal_restore());
    }

    #[test]
    fn geometry_change_and_large_dimensions_use_existing_resize_protocol() {
        let arena = setup(); prime(&arena);
        BOUNDARY.lock().unwrap().surface = (1, 100, 32);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("\x1b[8;32;100t"));
        assert!(!production::take_terminal_restore());
        BOUNDARY.lock().unwrap().surface = (2, u32::MAX, u32::MAX);
        assert_eq!(read(&arena, Duration::ZERO).as_deref(), Some("\x1b[8;65535;65535t"));
        assert!(production::take_terminal_restore());
    }
}
'''


def main():
    with tempfile.TemporaryDirectory(prefix="edit-native-terminal-") as directory:
        directory = Path(directory)
        (directory / "production.rs").write_text(SOURCE.read_text() + ACCESSORS)
        (directory / "lib.rs").write_text(HARNESS)
        (directory / "Cargo.toml").write_text(
            '[package]\nname="edit-native-terminal-test"\nversion="0.0.0"\nedition="2024"\n'
            '[workspace]\n[lib]\npath="lib.rs"\n[dependencies]\n'
            f'stdext={{path={json.dumps(str(STDEXT))}}}\n'
        )
        env = dict(os.environ)
        env["RUSTUP_TOOLCHAIN"] = "nightly-2026-07-10"
        env["CARGO_TARGET_DIR"] = str(directory / "target")
        for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER"):
            env.pop(name, None)
        env["RUSTFLAGS"] = '--check-cfg=cfg(target_os,values("trueos"))'
        for profile in ([], ["--release"]):
            subprocess.run(
                ["cargo", "test", "--offline", "--manifest-path", str(directory / "Cargo.toml"),
                 *profile, "--", "--test-threads=1"],
                cwd=directory, env=env, check=True, timeout=90,
            )


if __name__ == "__main__":
    main()
