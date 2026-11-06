#!/usr/bin/env python3
"""Compile the actual native System backend with a guarded host ABI boundary.

The production realloc_fallback is taken from pinned rust-src unchanged. Only
std's stability annotation and module docs are removed for this host harness;
the allocator implementation and native ABI declarations are used verbatim.
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


HARNESS = r'''
#![forbid(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]
pub use std::ptr;
mod alloc {
    pub use std::alloc::{GlobalAlloc, Layout};
    pub struct System;
}
use alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

mod backend {
    use crate::alloc::{GlobalAlloc, Layout, System};
    use crate::ptr;
    REALLOC_FALLBACK
    mod native {
        include!(SOURCE);
    }
}

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
        let allocation = ledger.live.remove(&(pointer as usize)).expect("free must use a live exact pointer");
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
fn check_live(pointer: *mut u8, layout: Layout) {
    assert!(!pointer.is_null());
    assert_eq!(pointer as usize % layout.align(), 0);
    let allocation = *LEDGER.lock().unwrap().live.get(&(pointer as usize)).unwrap();
    assert_eq!(allocation.requested, layout);
    check_guards(pointer, allocation);
}

fn small_and_overaligned(overaligned: bool) {
    let cases: &[(usize, usize)] = if overaligned {
        &[(1, 128), (17, 256), (257, 4096), (8193, 8192)]
    } else { &[(1, 1), (3, 2), (7, 8), (15, 16), (63, 64), (4096, 8)] };
    for &(size, align) in cases {
        let layout = Layout::from_size_align(size, align).unwrap();
        let pointer = unsafe { System.alloc(layout) };
        check_live(pointer, layout);
        assert!(block(pointer, size).iter().all(|byte| *byte == FRESH));
        fill(pointer, size);
        check_pattern(pointer, size);
        unsafe { System.dealloc(pointer, layout) };
    }
    let ledger = LEDGER.lock().unwrap();
    assert_eq!(ledger.attempts, cases);
    assert_eq!(ledger.freed.len(), cases.len());
}

fn zeroed() {
    for &(size, align) in &[(1,1), (13,64), (257,4096)] {
        let layout = Layout::from_size_align(size, align).unwrap();
        let pointer = unsafe { System.alloc_zeroed(layout) };
        check_live(pointer, layout);
        assert!(block(pointer,size).iter().all(|byte| *byte == 0));
        unsafe { System.dealloc(pointer,layout) };
    }
    FAIL_NEXT.store(true,Ordering::Relaxed);
    assert!(unsafe { System.alloc_zeroed(Layout::from_size_align(23,128).unwrap()) }.is_null());
    assert_eq!(LEDGER.lock().unwrap().freed.len(),3);
}

fn realloc_case(old_size: usize, new_size: usize, align: usize) {
    let old_layout = Layout::from_size_align(old_size,align).unwrap();
    let old = unsafe { System.alloc(old_layout) };
    fill(old,old_size);
    let new = unsafe { System.realloc(old,old_layout,new_size) };
    let new_layout = Layout::from_size_align(new_size,align).unwrap();
    check_live(new,new_layout);
    assert_ne!(old,new,"fallback allocates before freeing old storage");
    check_pattern(new,old_size.min(new_size));
    if new_size > old_size { assert!(block(new,new_size)[old_size..].iter().all(|byte| *byte == FRESH)); }
    {
        let ledger = LEDGER.lock().unwrap();
        assert_eq!(ledger.attempts,[(old_size,align),(new_size,align)]);
        assert_eq!(ledger.freed,[old as usize]);
        assert!(!ledger.live.contains_key(&(old as usize)));
    }
    unsafe { System.dealloc(new,new_layout) };
    assert_eq!(LEDGER.lock().unwrap().freed,[old as usize,new as usize]);
}

fn failed_realloc() {
    let layout = Layout::from_size_align(71,256).unwrap();
    let old = unsafe { System.alloc(layout) };
    fill(old,layout.size());
    FAIL_NEXT.store(true,Ordering::Relaxed);
    let replacement = unsafe { System.realloc(old,layout,4097) };
    assert!(replacement.is_null());
    check_live(old,layout);
    check_pattern(old,layout.size());
    {
        let ledger = LEDGER.lock().unwrap();
        assert_eq!(ledger.live.len(),1);
        assert!(ledger.freed.is_empty());
        assert_eq!(ledger.attempts,[(71,256),(4097,256)]);
    }
    unsafe { System.dealloc(old,layout) };
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("small") => small_and_overaligned(false),
        Some("overaligned") => small_and_overaligned(true),
        Some("zeroed") => zeroed(),
        Some("grow") => realloc_case(13,257,64),
        Some("shrink") => realloc_case(257,13,256),
        Some("same_size") => realloc_case(73,73,128),
        Some("failed_realloc") => failed_realloc(),
        _ => panic!("unknown allocator case"),
    }
    assert!(LEDGER.lock().unwrap().live.is_empty(),"every successful allocation must be released");
}
'''


def function_item(source, name):
    """Extract a production function including its body, without duplicating it."""
    start = re.search(r"(?:unsafe\s+)?fn\s+" + re.escape(name) + r"\s*\(", source).start()
    opening = source.index("{", start)
    depth = 1
    cursor = opening + 1
    while depth:
        depth += (source[cursor] == "{") - (source[cursor] == "}")
        cursor += 1
    return source[start:cursor]


@unittest.skipUnless(shutil.which("patch") and shutil.which("rustup"),
                     "allocator compilation requires patch and pinned rust-src")
class StdNativeAllocator(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run(["rustup", "run", build.TOOLCHAIN, "rustc", "--print", "sysroot"],
                                text=True, capture_output=True)
        if result.returncode:
            raise unittest.SkipTest("the pinned Rust toolchain is not installed")
        cls.installed = Path(result.stdout.strip())
        cls.library = cls.installed / "lib/rustlib/src/rust/library"
        relative = Path("std/src/sys/alloc/mod.rs")
        original = cls.library / relative
        if not original.is_file():
            raise unittest.SkipTest("the pinned rust-src component is not installed")
        cls.temporary = tempfile.TemporaryDirectory(prefix="trueos-std-allocator-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        destination = cls.root / relative
        destination.parent.mkdir(parents=True)
        destination.write_bytes(original.read_bytes())

        # Keep both allocator sections together, including the /dev/null new file.
        full_patch = (build.ROOT / "tools/trueos-std.patch").read_text()
        sections = re.findall(r"(?ms)^--- .*?(?=^--- |\Z)", full_patch)
        selected = [section for section in sections
                    if re.search(r"^\+\+\+ b/std/src/sys/alloc/(mod|trueos)\.rs$", section, re.M)]
        if len(selected) != 2:
            raise AssertionError("native allocator must patch selection and supply its backend")
        patch = cls.root / "allocator.patch"
        patch.write_text("".join(selected))
        build.apply_std_patch(cls.root, patch)
        native = destination.with_name("trueos.rs")
        host_source = "\n".join(line for line in native.read_text().splitlines()
                                if not line.startswith("//!") and not line.startswith("#[stable(")) + "\n"
        host_native = cls.root / "native-host.rs"
        host_native.write_text(host_source)
        fallback = function_item(original.read_text(), "realloc_fallback")
        harness = cls.root / "harness.rs"
        harness.write_text(HARNESS.replace("REALLOC_FALLBACK", fallback)
                          .replace("SOURCE", json.dumps(str(host_native))))
        cls.env = dict(os.environ)
        cls.env["LD_LIBRARY_PATH"] = str(cls.installed / "lib") + ":" + cls.env.get("LD_LIBRARY_PATH", "")
        cls.binaries = []
        for optimization in (0, 3):
            binary = cls.root / f"allocator-o{optimization}"
            command = [cls.installed / "bin/rustc", "--edition=2024", "-C", f"opt-level={optimization}",
                       harness, "-o", binary]
            subprocess.run(list(map(str, command)), env=cls.env, check=True,
                           text=True, capture_output=True)
            cls.binaries.append(binary)

    def test_full_patch_applies_to_pinned_pristine_rust_src(self):
        result = subprocess.run(["patch", "--batch", "--forward", "--dry-run", "-p1", "-i",
                                 str(build.ROOT / "tools/trueos-std.patch")], cwd=self.library,
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.assertEqual(result.returncode, 0, result.stdout)

    def test_native_layout_allocation_and_reallocation_contracts(self):
        for binary in self.binaries:
            for case in ("small", "overaligned", "zeroed", "grow", "shrink", "same_size", "failed_realloc"):
                with self.subTest(optimization=binary.name, case=case):
                    subprocess.run([str(binary), case], env=self.env, check=True,
                                   text=True, capture_output=True)


if __name__ == "__main__":
    unittest.main()
