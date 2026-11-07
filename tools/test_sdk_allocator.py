#!/usr/bin/env python3
"""Exercise the SDK's actual global allocator at a guarded native ABI boundary.

The production allocator and its native ABI declarations are extracted verbatim.
The host harness supplies only the native allocation hooks, so a return to the
C allocation adapter fails compilation. A separate no_std object enables the
real default-global-allocator feature and checks its imported allocation hooks.
"""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

import build


SDK = build.ROOT / "vendor/trueos/src/lib.rs"
ABI = build.ROOT / "vendor/trueos-v/src/bp_abi.rs"
NATIVE_SYMBOLS = {"sys_alloc_aligned", "sys_dealloc"}

HARNESS = r'''
#![forbid(unsafe_op_in_unsafe_fn)]
#![allow(dead_code, unexpected_cfgs)]
mod sdk {
    use core::alloc::{GlobalAlloc, Layout};
    use core::ptr::null_mut;
    mod v { pub mod vcabi { unsafe extern "C" { DECLARATIONS } } }
    include!(SOURCE);
}
use std::alloc::{GlobalAlloc, Layout};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
static ALLOCATOR: sdk::TrueosAllocator = sdk::TrueosAllocator;

#[derive(Clone, Copy)]
struct Allocation { base: usize, whole: Layout, requested: Layout, guard: usize }
#[derive(Default)]
struct Ledger {
    live: HashMap<usize, Allocation>,
    attempts: Vec<(usize, usize)>,
    freed: Vec<usize>,
}
static LEDGER: LazyLock<Mutex<Ledger>> = LazyLock::new(|| Mutex::new(Ledger::default()));
static FAIL_NEXT: AtomicBool = AtomicBool::new(false);
const GUARD: u8 = 0xD3;
const FRESH: u8 = 0xA5;

#[unsafe(no_mangle)]
unsafe extern "C" fn sys_alloc_aligned(size: usize, align: usize) -> *mut u8 {
    LEDGER.lock().unwrap().attempts.push((size, align));
    if FAIL_NEXT.swap(false, Ordering::Relaxed) { return std::ptr::null_mut(); }
    let requested = Layout::from_size_align(size, align).unwrap();
    assert_ne!(size, 0, "the SDK promises at least one allocated byte");
    let guard = align.max(64);
    let whole = Layout::from_size_align(size.checked_add(2 * guard).unwrap(), align).unwrap();
    let base = unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, whole) };
    assert!(!base.is_null());
    unsafe { std::ptr::write_bytes(base, GUARD, whole.size()) };
    let pointer = unsafe { base.add(guard) };
    unsafe { std::ptr::write_bytes(pointer, FRESH, size) };
    let previous = LEDGER.lock().unwrap().live.insert(pointer as usize,
        Allocation { base: base as usize, whole, requested, guard });
    assert!(previous.is_none());
    pointer
}

fn check_guards(pointer: *mut u8, allocation: Allocation) {
    let prefix = unsafe { std::slice::from_raw_parts(allocation.base as *const u8, allocation.guard) };
    let suffix = unsafe { std::slice::from_raw_parts(pointer.add(allocation.requested.size()), allocation.guard) };
    assert!(prefix.iter().all(|byte| *byte == GUARD), "prefix guard modified");
    assert!(suffix.iter().all(|byte| *byte == GUARD), "suffix guard modified");
}

#[unsafe(no_mangle)]
unsafe extern "C" fn sys_dealloc(pointer: *mut u8) {
    let allocation = {
        let mut ledger = LEDGER.lock().unwrap();
        let allocation = ledger.live.remove(&(pointer as usize)).expect("free requires a live exact pointer");
        ledger.freed.push(pointer as usize);
        allocation
    };
    check_guards(pointer, allocation);
    unsafe { std::alloc::GlobalAlloc::dealloc(&std::alloc::System, allocation.base as *mut u8, allocation.whole) };
}

fn block(pointer: *mut u8, size: usize) -> &'static [u8] {
    unsafe { std::slice::from_raw_parts(pointer, size) }
}
fn fill(pointer: *mut u8, size: usize) {
    for offset in 0..size { unsafe { pointer.add(offset).write((offset as u8).wrapping_mul(31).wrapping_add(7)) }; }
}
fn check_pattern(pointer: *mut u8, size: usize) {
    for (offset, byte) in block(pointer, size).iter().enumerate() {
        assert_eq!(*byte, (offset as u8).wrapping_mul(31).wrapping_add(7));
    }
}
fn check_live(pointer: *mut u8, size: usize, align: usize) {
    assert!(!pointer.is_null());
    assert_eq!(pointer as usize % align, 0);
    let allocation = *LEDGER.lock().unwrap().live.get(&(pointer as usize)).unwrap();
    assert_eq!(allocation.requested, Layout::from_size_align(size.max(1), align).unwrap());
    check_guards(pointer, allocation);
}

fn allocation_cases(overaligned: bool, zeroed: bool) {
    let cases: &[(usize, usize)] = if overaligned {
        &[(0, 128), (1, 256), (17, 4096), (8193, 8192)]
    } else { &[(0, 1), (1, 1), (3, 2), (7, 8), (15, 16), (4096, 8)] };
    for &(size, align) in cases {
        let layout = Layout::from_size_align(size, align).unwrap();
        let pointer = unsafe {
            if zeroed { ALLOCATOR.alloc_zeroed(layout) } else { ALLOCATOR.alloc(layout) }
        };
        check_live(pointer, size, align);
        assert!(block(pointer, size.max(1)).iter().all(|byte| *byte == if zeroed { 0 } else { FRESH }));
        fill(pointer, size.max(1));
        check_pattern(pointer, size.max(1));
        unsafe { ALLOCATOR.dealloc(pointer, layout) };
    }
    let ledger = LEDGER.lock().unwrap();
    let expected = cases.iter().map(|&(size, align)| (size.max(1), align)).collect::<Vec<_>>();
    assert_eq!(ledger.attempts, expected);
    assert_eq!(ledger.freed.len(), cases.len());
}

fn realloc_case(old_size: usize, new_size: usize, align: usize) {
    let old_layout = Layout::from_size_align(old_size, align).unwrap();
    let old = unsafe { ALLOCATOR.alloc(old_layout) };
    fill(old, old_size);
    let new = unsafe { ALLOCATOR.realloc(old, old_layout, new_size) };
    let new_layout = Layout::from_size_align(new_size.max(1), align).unwrap();
    check_live(new, new_size, align);
    assert_ne!(old, new, "replacement must be allocated before releasing the original");
    check_pattern(new, old_size.min(new_size));
    if new_size > old_size { assert!(block(new, new_size)[old_size..].iter().all(|byte| *byte == FRESH)); }
    {
        let ledger = LEDGER.lock().unwrap();
        assert_eq!(ledger.attempts, [(old_size.max(1), align), (new_size.max(1), align)]);
        assert_eq!(ledger.freed, [old as usize]);
        assert!(!ledger.live.contains_key(&(old as usize)));
    }
    unsafe { ALLOCATOR.dealloc(new, new_layout) };
    assert_eq!(LEDGER.lock().unwrap().freed, [old as usize, new as usize]);
}

fn failed_realloc() {
    for align in [8, 256] {
        let layout = Layout::from_size_align(71, align).unwrap();
        let old = unsafe { ALLOCATOR.alloc(layout) };
        fill(old, layout.size());
        FAIL_NEXT.store(true, Ordering::Relaxed);
        let replacement = unsafe { ALLOCATOR.realloc(old, layout, 4097) };
        assert!(replacement.is_null());
        check_live(old, layout.size(), align);
        check_pattern(old, layout.size());
        assert!(!LEDGER.lock().unwrap().freed.contains(&(old as usize)));
        unsafe { ALLOCATOR.dealloc(old, layout) };
    }
    assert_eq!(LEDGER.lock().unwrap().attempts, [(71,8), (4097,8), (71,256), (4097,256)]);
}

fn null_and_overflow() {
    let old_layout = Layout::from_size_align(19, 256).unwrap();
    // Null pointers and zero sizes are compatibility extensions retained by
    // this SDK; valid GlobalAlloc callers do not depend on either extension.
    let new = unsafe { ALLOCATOR.realloc(std::ptr::null_mut(), old_layout, 37) };
    check_live(new, 37, 256);
    unsafe { ALLOCATOR.dealloc(new, Layout::from_size_align(37, 256).unwrap()) };
    let old = unsafe { ALLOCATOR.alloc(old_layout) };
    fill(old, old_layout.size());
    let count = LEDGER.lock().unwrap().attempts.len();
    for invalid in [usize::MAX, isize::MAX as usize] {
        assert!(unsafe { ALLOCATOR.realloc(old, old_layout, invalid) }.is_null());
        check_pattern(old, old_layout.size());
        check_live(old, old_layout.size(), 256);
    }
    assert_eq!(LEDGER.lock().unwrap().attempts.len(), count);
    unsafe { ALLOCATOR.dealloc(old, old_layout) };
    unsafe { ALLOCATOR.dealloc(std::ptr::null_mut(), old_layout) };
    assert_eq!(LEDGER.lock().unwrap().freed.len(), 2);
}

fn failed_allocation() {
    for align in [1, 4096] {
        let layout = Layout::from_size_align(257, align).unwrap();
        FAIL_NEXT.store(true, Ordering::Relaxed);
        assert!(unsafe { ALLOCATOR.alloc(layout) }.is_null());
        FAIL_NEXT.store(true, Ordering::Relaxed);
        assert!(unsafe { ALLOCATOR.alloc_zeroed(layout) }.is_null());
    }
    assert!(LEDGER.lock().unwrap().freed.is_empty());
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("small") => allocation_cases(false, false),
        Some("overaligned") => allocation_cases(true, false),
        Some("zeroed") => allocation_cases(false, true),
        Some("zeroed_overaligned") => allocation_cases(true, true),
        Some("grow") => realloc_case(13, 257, 8),
        Some("grow_overaligned") => realloc_case(13, 257, 256),
        Some("shrink") => realloc_case(257, 13, 8),
        Some("shrink_overaligned") => realloc_case(257, 13, 256),
        Some("same_size") => realloc_case(73, 73, 128),
        Some("zero_size") => realloc_case(73, 0, 128),
        Some("zero_original") => realloc_case(0, 73, 128),
        Some("failed_realloc") => failed_realloc(),
        Some("null_and_overflow") => null_and_overflow(),
        Some("failed_allocation") => failed_allocation(),
        _ => panic!("unknown allocator case"),
    }
    assert!(LEDGER.lock().unwrap().live.is_empty(), "every successful allocation must be released");
}
'''


@unittest.skipUnless(shutil.which("rustup") and shutil.which("nm"),
                     "allocator compilation requires the pinned Rust toolchain and nm")
class SdkNativeAllocator(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run(["rustup", "run", build.TOOLCHAIN, "rustc", "--print", "sysroot"],
                                text=True, capture_output=True)
        if result.returncode:
            raise unittest.SkipTest("the pinned Rust toolchain is not installed")
        installed = Path(result.stdout.strip())
        cls.rustc = installed / "bin/rustc"
        cls.env = dict(os.environ)
        cls.env["LD_LIBRARY_PATH"] = str(installed / "lib") + ":" + cls.env.get("LD_LIBRARY_PATH", "")
        cls.temporary = tempfile.TemporaryDirectory(prefix="trueos-sdk-allocator-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        source = SDK.read_text()
        start = source.index("pub struct TrueosAllocator;")
        end = source.index("\npub fn panic_abort(", start)
        cls.allocator = cls.root / "allocator.rs"
        cls.allocator.write_text(source[start:end])
        declarations = []
        for symbol in sorted(NATIVE_SYMBOLS):
            matches = re.findall(r"^\s*(pub fn " + symbol + r"\([^;]+;)", ABI.read_text(), re.M)
            if len(matches) != 1:
                raise AssertionError(f"expected one native ABI declaration for {symbol}")
            declarations.append(matches[0])
        cls.declarations = "\n".join(declarations)
        harness = cls.root / "harness.rs"
        harness.write_text(HARNESS.replace("DECLARATIONS", cls.declarations)
                          .replace("SOURCE", json.dumps(str(cls.allocator))))
        cls.binaries = []
        for optimization in (0, 3):
            binary = cls.root / f"allocator-o{optimization}"
            subprocess.run(list(map(str, [cls.rustc, "--edition=2024", "-C", f"opt-level={optimization}",
                                         harness, "-o", binary])), env=cls.env, check=True,
                           text=True, capture_output=True)
            cls.binaries.append(binary)

    def test_native_layout_allocation_and_reallocation_contracts(self):
        for binary in self.binaries:
            for case in ("small", "overaligned", "zeroed", "zeroed_overaligned", "grow", "grow_overaligned",
                         "shrink", "shrink_overaligned", "same_size", "zero_size", "zero_original",
                         "failed_realloc", "null_and_overflow", "failed_allocation"):
                with self.subTest(optimization=binary.name, case=case):
                    subprocess.run([str(binary), case], env=self.env, check=True,
                                   text=True, capture_output=True)

    def test_default_global_allocator_is_no_std_and_only_imports_native_allocation_hooks(self):
        probe = self.root / "global.rs"
        probe.write_text('''#![no_std]
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
mod v { pub mod vcabi { unsafe extern "C" { ''' + self.declarations + ''' } } }
include!(''' + json.dumps(str(self.allocator)) + ''');
''')
        for enabled in (False, True):
            with self.subTest(default_global_allocator=enabled):
                output = self.root / f"global-{enabled}.o"
                command = [self.rustc, "--edition=2024", "--crate-type=rlib", "--emit=obj",
                           "-C", "opt-level=3", probe, "-o", output]
                if enabled:
                    command += ["--cfg", 'feature="default-global-allocator"']
                subprocess.run(list(map(str, command)), env=self.env, check=True,
                               text=True, capture_output=True)
                symbols = subprocess.check_output(["nm", "-u", str(output)], text=True)
                imported = {line.split()[-1] for line in symbols.splitlines() if line.split()}
                self.assertTrue(NATIVE_SYMBOLS <= imported, symbols)
                self.assertFalse(any(name.startswith("trueos_cabi_") for name in imported), symbols)
                defined = subprocess.check_output(["nm", "--defined-only", str(output)], text=True)
                for wrapper in ("___rust_alloc", "___rust_dealloc", "___rust_realloc", "___rust_alloc_zeroed"):
                    self.assertEqual(wrapper in defined, enabled, defined)


if __name__ == "__main__":
    unittest.main()
