#!/usr/bin/env python3
"""Check the actual native kernel offset hook/locale policy and QuickJS bridge.

Host fixtures supply VM transport and boot command line only. The kernel hook
is extracted verbatim, the full production locale module is compiled directly,
and the C-facing engine bridge is the production module. This is not a VM boot.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess

ENGINE = Path(__file__).resolve().parents[1]
TRUEOS = ENGINE.parents[2].parent / "TRUEOS"
TOOLCHAIN = "nightly-2026-07-10"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    platform = TRUEOS / "src/r/platform.rs"
    source = platform.read_text()
    start = source.index('pub extern "C" fn trueos_time_timezone_offset_seconds(')
    end = source.index("\n}", start) + 2
    production_hook = source[start:end]
    fixture = output / "native_timezone.rs"
    fixture.write_text('''#![allow(dead_code)]
use core::sync::atomic::{AtomicBool, AtomicI64, Ordering};
static GUEST: AtomicBool = AtomicBool::new(false);
static FAIL: AtomicBool = AtomicBool::new(false);
static LAST_UTC: AtomicI64 = AtomicI64::new(0);
mod limine { pub fn executable_cmdline() -> Option<&'static str> { None } }
#[path = ''' + json.dumps(str(TRUEOS / "src/locale.rs")) + '''] mod locale;
#[path = ''' + json.dumps(str(ENGINE / "src/native_time.rs")) + '''] mod engine_bridge;
mod hv {
    pub fn current_hull_guest_context_vm_id() -> Option<u64> {
        super::GUEST.load(super::Ordering::Relaxed).then_some(7)
    }
}
mod trueos_vm { pub mod vmcall {
    pub const STATUS_OK: u64 = 0;
    pub const OP_BP_TIMEZONE_OFFSET_SECONDS: u32 = 0x24c;
    pub fn call(op: u32, utc: u64, unused: u64) -> (u64, u64) {
        assert_eq!(op, OP_BP_TIMEZONE_OFFSET_SECONDS); assert_eq!(unused, 0);
        super::super::LAST_UTC.store(utc as i64, super::super::Ordering::Relaxed);
        if super::super::FAIL.load(super::super::Ordering::Relaxed) { return (1, 0); }
        (STATUS_OK, super::super::locale::utc_offset_seconds((utc as i64).max(0) as u64) as i64 as u64)
    }
} }
#[unsafe(no_mangle)]
''' + production_hook + '''
#[unsafe(no_mangle)] extern "C" fn trueos_time_unix_nanos() -> u64 { 0 }
#[unsafe(no_mangle)] unsafe extern "C" fn sys_rand(_: *mut u32, _: usize) {}

#[test]
fn native_offset_preserves_kernel_zone_dst_and_signed_vm_transport() {
    // ECMAScript's valid date extremes, negative epochs, and both 2026 Berlin
    // DST boundaries are exercised through the actual native kernel function.
    let berlin = [
        (i64::MIN, 3600), (-8_640_000_000_000, 3600), (-1, 3600), (0, 3600),
        (1_774_745_999, 3600), (1_774_746_000, 7200),
        (1_792_889_999, 7200), (1_792_890_000, 3600),
        (8_640_000_000_000, 7200),
    ];
    for guest in [false, true] {
        GUEST.store(guest, Ordering::Relaxed);
        assert!(locale::set_timezone_from_network("UTC"));
        for (utc, _) in berlin {
            assert_eq!(trueos_time_timezone_offset_seconds(utc), 0);
            assert_eq!(engine_bridge::hull_quickjs_native_timezone_offset_seconds(utc), 0);
        }
        assert!(locale::set_timezone_from_network("Europe/Berlin"));
        for (utc, expected) in berlin {
            assert_eq!(trueos_time_timezone_offset_seconds(utc), expected, "guest={guest}, utc={utc}");
            assert_eq!(engine_bridge::hull_quickjs_native_timezone_offset_seconds(utc), expected);
            if guest { assert_eq!(LAST_UTC.load(Ordering::Relaxed), utc); }
        }
    }
    FAIL.store(true, Ordering::Relaxed);
    assert_eq!(engine_bridge::hull_quickjs_native_timezone_offset_seconds(0), 0);
}
''')
    records = []

    def run(label, command):
        log = output / f"{label}.log"
        with log.open("w") as stream:
            stream.write(shlex.join(map(str, command)) + "\n")
            stream.flush()
            result = subprocess.run(list(map(str, command)), stdout=stream,
                                    stderr=subprocess.STDOUT, timeout=60, cwd="/tmp")
        records.append(dict(label=label, command=list(map(str, command)),
                            log=str(log), exit=result.returncode))
        if result.returncode:
            raise RuntimeError(log.read_text())

    locale_lib = output / "libtrueos_locale.rlib"
    run("compile-locale", ["rustup", "run", TOOLCHAIN, "rustc", "--edition=2024",
        "--crate-name=trueos_locale", "--crate-type=rlib", TRUEOS / "crates/trueos-locale/src/lib.rs",
        "-o", locale_lib])
    for opt in (0, 3):
        binary = output / f"native-timezone-O{opt}"
        run(f"O{opt}-compile", ["rustup", "run", TOOLCHAIN, "rustc", "--edition=2024", "--test",
            "-C", f"opt-level={opt}", "--extern", f"trueos_locale={locale_lib}", fixture, "-o", binary])
        run(f"O{opt}-behavior", [binary, "--nocapture"])
    paths = [platform, TRUEOS / "src/locale.rs", TRUEOS / "crates/trueos-locale/src/lib.rs",
             ENGINE / "src/native_time.rs", Path(__file__)]
    (output / "receipt.json").write_text(json.dumps(dict(scope=__doc__, records=records,
        sources=[dict(path=str(p), sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for p in paths]), indent=2) + "\n")
    print("PASS actual native kernel offset hook, locale policy, signed VM fixture, and engine bridge O0/O3")


if __name__ == "__main__":
    main()
