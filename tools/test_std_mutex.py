#!/usr/bin/env python3
"""Compile the patched PAL mutex with and without the TRUEOS attribute API."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

import build


HARNESS = r'''
#![feature(negative_impls)]
#![allow(dead_code, non_camel_case_types)]
pub use std::{cell, io, mem, pin, ptr};
use std::sync::atomic::{AtomicUsize, Ordering};

mod libc {
    use super::*;
    pub static INITS: AtomicUsize = AtomicUsize::new(0);
    pub static ATTRIBUTES: AtomicUsize = AtomicUsize::new(0);
    pub static DESTROYS: AtomicUsize = AtomicUsize::new(0);
    pub const EINVAL: i32 = 22;
    pub struct pthread_mutex_t { locked: bool }
    pub const PTHREAD_MUTEX_INITIALIZER: pthread_mutex_t = pthread_mutex_t { locked: false };

    // Deliberately omit the entire attribute API on TRUEOS: its compiled PAL
    // must only depend on the mutex operation itself.
    #[cfg(not(target_os = "trueos"))]
    mod attributes {
        use super::*;
        pub type pthread_mutexattr_t = i32;
        pub const PTHREAD_MUTEX_NORMAL: i32 = 1;
        pub unsafe fn pthread_mutexattr_init(attr: *mut i32) -> i32 {
            ATTRIBUTES.fetch_add(1, Ordering::Relaxed);
            unsafe { attr.write(0) };
            0
        }
        pub unsafe fn pthread_mutexattr_settype(attr: *mut i32, kind: i32) -> i32 {
            assert_eq!(unsafe { *attr }, 0);
            assert_eq!(kind, PTHREAD_MUTEX_NORMAL);
            ATTRIBUTES.fetch_add(1, Ordering::Relaxed);
            unsafe { attr.write(kind) };
            0
        }
        pub unsafe fn pthread_mutexattr_destroy(attr: *mut i32) -> i32 {
            assert_eq!(unsafe { *attr }, PTHREAD_MUTEX_NORMAL);
            ATTRIBUTES.fetch_add(1, Ordering::Relaxed);
            0
        }
    }
    #[cfg(not(target_os = "trueos"))]
    pub use attributes::*;

    pub unsafe fn pthread_mutex_init(mutex: *mut pthread_mutex_t, attr: *const i32) -> i32 {
        #[cfg(target_os = "trueos")]
        assert!(attr.is_null());
        #[cfg(not(target_os = "trueos"))]
        {
            assert!(!attr.is_null());
            assert_eq!(unsafe { *attr }, PTHREAD_MUTEX_NORMAL);
        }
        INITS.fetch_add(1, Ordering::Relaxed);
        unsafe { mutex.write(PTHREAD_MUTEX_INITIALIZER) };
        0
    }
    pub unsafe fn pthread_mutex_lock(mutex: *mut pthread_mutex_t) -> i32 {
        assert!(!unsafe { (*mutex).locked });
        unsafe { (*mutex).locked = true };
        0
    }
    pub unsafe fn pthread_mutex_trylock(mutex: *mut pthread_mutex_t) -> i32 {
        if unsafe { (*mutex).locked } { 16 } else {
            unsafe { (*mutex).locked = true };
            0
        }
    }
    pub unsafe fn pthread_mutex_unlock(mutex: *mut pthread_mutex_t) -> i32 {
        assert!(unsafe { (*mutex).locked });
        unsafe { (*mutex).locked = false };
        0
    }
    pub unsafe fn pthread_mutex_destroy(mutex: *mut pthread_mutex_t) -> i32 {
        assert!(!unsafe { (*mutex).locked });
        DESTROYS.fetch_add(1, Ordering::Relaxed);
        0
    }
}

mod backend {
    pub fn cvt_nz(code: i32) -> Result<(), i32> {
        if code == 0 { Ok(()) } else { Err(code) }
    }
    pub mod sync {
        pub mod mutex {
            use crate::libc;
            include!(SOURCE);
        }
    }
}

fn main() {
    let mut mutex = Box::pin(backend::sync::mutex::Mutex::new());
    unsafe {
        mutex.as_mut().init();
        assert!(mutex.as_ref().try_lock());
        assert!(!mutex.as_ref().try_lock());
        mutex.as_ref().unlock();
        mutex.as_ref().lock();
        mutex.as_ref().unlock();
    }
    drop(mutex);
    assert_eq!(libc::INITS.load(Ordering::Relaxed), 1);
    assert_eq!(libc::DESTROYS.load(Ordering::Relaxed), 1);
    assert_eq!(libc::ATTRIBUTES.load(Ordering::Relaxed),
               if cfg!(target_os = "trueos") { 0 } else { 3 });
}
'''


@unittest.skipUnless(shutil.which("patch") and shutil.which("rustup"),
                     "PAL compilation requires patch and the pinned Rust toolchain")
class StdMutexBackend(unittest.TestCase):
    def test_trueos_needs_no_attributes_and_other_unix_keeps_explicit_normal(self):
        result = subprocess.run(["rustup", "run", build.TOOLCHAIN, "rustc", "--print", "sysroot"],
                                text=True, capture_output=True)
        if result.returncode:
            self.skipTest("the pinned Rust toolchain is not installed")
        installed = Path(result.stdout.strip())
        relative = Path("std/src/sys/pal/unix/sync/mutex.rs")
        original = installed / "lib/rustlib/src/rust/library" / relative
        if not original.is_file():
            self.skipTest("the pinned rust-src component is not installed")
        sections = (build.ROOT / "tools/trueos-std.patch").read_text().split("--- a/")
        mutex_patch = next(section for section in sections if section.startswith(str(relative) + "\n"))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / relative
            source.parent.mkdir(parents=True)
            source.write_bytes(original.read_bytes())
            patch = root / "mutex.patch"
            patch.write_text("--- a/" + mutex_patch)
            build.apply_std_patch(root, patch)
            harness = root / "harness.rs"
            harness.write_text(HARNESS.replace("SOURCE", json.dumps(str(source))))
            env = dict(os.environ)
            env["LD_LIBRARY_PATH"] = str(installed / "lib") + ":" + env.get("LD_LIBRARY_PATH", "")
            for trueos in (False, True):
                with self.subTest(trueos=trueos):
                    binary = root / ("trueos" if trueos else "unix")
                    command = [installed / "bin/rustc", "--edition=2024", harness, "-o", binary]
                    if trueos:
                        command += ["--cfg", 'target_os="trueos"', "-Aexplicit_builtin_cfgs_in_flags"]
                    subprocess.run(list(map(str, command)), env=env, check=True,
                                   text=True, capture_output=True)
                    subprocess.run([str(binary)], check=True, text=True, capture_output=True)


if __name__ == "__main__":
    unittest.main()
