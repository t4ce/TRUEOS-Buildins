#!/usr/bin/env python3
"""Exercise the actual patched std error and panic-output implementations.

Apply the canonical patch to pristine pinned rust-src, extract only the native
items unchanged, and compile them against host std. The harness alone receives
the TRUEOS cfg; its attached-console ABI boundary records actual writes.
"""
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
pub use std::io;
use std::io::Write;
use std::sync::Mutex;

mod native {
    use crate::io;
    ERROR_STRING
    PANIC_STRUCT
    PANIC_IMPL
    PANIC_OUTPUT
}

struct Boundary {
    limit: usize,
    corrupt: bool,
    calls: Vec<Vec<u8>>,
    output: Vec<u8>,
}
static BOUNDARY: Mutex<Boundary> = Mutex::new(Boundary {
    limit: usize::MAX, corrupt: false, calls: Vec::new(), output: Vec::new(),
});

#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_shell_attached_write(bytes: *const u8, len: usize) -> usize {
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    let mut boundary = BOUNDARY.lock().unwrap();
    boundary.calls.push(bytes.to_vec());
    if boundary.corrupt {
        return if boundary.limit == usize::MAX { usize::MAX } else { len + 1 };
    }
    let count = len.min(boundary.limit);
    boundary.output.extend_from_slice(&bytes[..count]);
    count
}

fn signed_error_strings() {
    for (code, expected) in [
        (0, "trueos errno 0"),
        (2, "trueos errno 2"),
        (-2, "trueos errno -2"),
        (i32::MIN, "trueos errno -2147483648"),
        (i32::MAX, "trueos errno 2147483647"),
    ] {
        assert_eq!(native::error_string(code), expected);
    }
}

fn forwards_binary_bytes_and_count() {
    let bytes = [0, b'p', 0xff, 0x1b, b'[', b'0', b'm', b'\n', 0x80];
    let mut writer = native::panic_output().expect("native panic diagnostics stay enabled");
    BOUNDARY.lock().unwrap().limit = 5;
    assert_eq!(writer.write(&bytes).unwrap(), 5);
    writer.flush().unwrap();
    let boundary = BOUNDARY.lock().unwrap();
    assert_eq!(boundary.calls, [bytes.to_vec()]);
    assert_eq!(boundary.output, bytes[..5]);
}

fn complete_panic_message_survives_partial_writes() {
    let bytes = b"panic: all bytes must reach the attached console\n";
    let mut writer = native::panic_output().unwrap();
    BOUNDARY.lock().unwrap().limit = 3;
    writer.write_all(bytes).unwrap();
    writer.flush().unwrap();
    let boundary = BOUNDARY.lock().unwrap();
    assert_eq!(boundary.output, bytes);
    assert_eq!(boundary.calls.len(), bytes.len().div_ceil(3));
    for (index, call) in boundary.calls.iter().enumerate() {
        assert_eq!(call, &bytes[index * 3..], "retry must start after the reported count");
    }
}

fn no_route_is_write_zero() {
    let mut writer = native::panic_output().expect("failed transport must not disable diagnostics");
    BOUNDARY.lock().unwrap().limit = 0;
    assert_eq!(writer.write_all(b"panic details").unwrap_err().kind(), io::ErrorKind::WriteZero);
    writer.flush().unwrap();
    let boundary = BOUNDARY.lock().unwrap();
    assert_eq!(boundary.calls, [b"panic details".to_vec()]);
    assert!(boundary.output.is_empty());
}

fn corrupt_count_is_invalid_data(extreme: bool) {
    let mut writer = native::panic_output().unwrap();
    {
        let mut boundary = BOUNDARY.lock().unwrap();
        boundary.corrupt = true;
        boundary.limit = if extreme { usize::MAX } else { 1 };
    }
    let error = writer.write(b"panic").unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "invalid terminal write count");
    let boundary = BOUNDARY.lock().unwrap();
    assert_eq!(boundary.calls, [b"panic".to_vec()]);
    assert!(boundary.output.is_empty());
}

fn empty_write_and_flush() {
    let mut writer = native::panic_output().unwrap();
    assert_eq!(writer.write(&[]).unwrap(), 0);
    writer.flush().unwrap();
    let boundary = BOUNDARY.lock().unwrap();
    assert_eq!(boundary.calls, [Vec::<u8>::new()]);
    assert!(boundary.output.is_empty());
}

fn formatted_diagnostic_preserves_error_details() {
    let mut writer = native::panic_output().unwrap();
    BOUNDARY.lock().unwrap().limit = 7;
    writeln!(writer, "panic: native operation failed: {}", native::error_string(i32::MIN)).unwrap();
    assert_eq!(BOUNDARY.lock().unwrap().output,
        b"panic: native operation failed: trueos errno -2147483648\n");
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("error_strings") => signed_error_strings(),
        Some("forward_count") => forwards_binary_bytes_and_count(),
        Some("partial_writes") => complete_panic_message_survives_partial_writes(),
        Some("write_zero") => no_route_is_write_zero(),
        Some("corrupt_count") => corrupt_count_is_invalid_data(false),
        Some("extreme_count") => corrupt_count_is_invalid_data(true),
        Some("empty_write") => empty_write_and_flush(),
        Some("formatted_diagnostic") => formatted_diagnostic_preserves_error_details(),
        _ => panic!("unknown diagnostic case"),
    }
}
'''


def native_item(source, declaration, *, unit=False):
    """Keep the production TRUEOS cfg and declaration, including its body."""
    marker = '#[cfg(target_os = "trueos")]\n' + declaration
    start = source.index(marker)
    if unit:
        return source[start:source.index(";", start) + 1]
    opening = source.index("{", start)
    depth, cursor = 1, opening + 1
    while depth:
        depth += (source[cursor] == "{") - (source[cursor] == "}")
        cursor += 1
    return source[start:cursor]


@unittest.skipUnless(shutil.which("patch") and shutil.which("rustup"),
                     "native diagnostics compilation requires patch and pinned rust-src")
class StdNativeDiagnostics(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        result = subprocess.run(["rustup", "run", build.TOOLCHAIN, "rustc", "--print", "sysroot"],
                                text=True, capture_output=True)
        if result.returncode:
            raise unittest.SkipTest("the pinned Rust toolchain is not installed")
        cls.installed = Path(result.stdout.strip())
        library = cls.installed / "lib/rustlib/src/rust/library"
        relatives = (Path("std/src/sys/io/error/unix.rs"), Path("std/src/sys/stdio/unix.rs"))
        if not all((library / relative).is_file() for relative in relatives):
            raise unittest.SkipTest("the pinned rust-src component is not installed")
        cls.temporary = tempfile.TemporaryDirectory(prefix="trueos-std-native-diagnostics-")
        cls.addClassCleanup(cls.temporary.cleanup)
        root = Path(cls.temporary.name)
        for relative in relatives:
            destination = root / relative
            destination.parent.mkdir(parents=True)
            destination.write_bytes((library / relative).read_bytes())

        full_patch = (build.ROOT / "tools/trueos-std.patch").read_text()
        sections = re.findall(r"(?ms)^--- .*?(?=^--- |\Z)", full_patch)
        selected = [section for section in sections if any(
            f"+++ b/{relative}\n" in section for relative in relatives)]
        if len(selected) != 2:
            raise AssertionError("native diagnostics must patch std error descriptions and panic output")
        patch = root / "diagnostics.patch"
        patch.write_text("".join(selected))
        build.apply_std_patch(root, patch)

        errors = (root / relatives[0]).read_text()
        stdio = (root / relatives[1]).read_text()
        replacements = {
            "ERROR_STRING": native_item(errors, "pub fn error_string("),
            "PANIC_STRUCT": native_item(stdio, "struct TrueosPanicOutput", unit=True),
            "PANIC_IMPL": native_item(stdio, "impl io::Write for TrueosPanicOutput"),
            "PANIC_OUTPUT": native_item(stdio, "pub fn panic_output("),
        }
        harness_text = HARNESS
        for marker, production in replacements.items():
            harness_text = harness_text.replace(marker, production)
        harness = root / "harness.rs"
        harness.write_text(harness_text)
        cls.env = dict(os.environ)
        cls.env["LD_LIBRARY_PATH"] = str(cls.installed / "lib") + ":" + cls.env.get("LD_LIBRARY_PATH", "")
        cls.binaries = []
        for optimization in (0, 3):
            binary = root / f"native-diagnostics-o{optimization}"
            command = [cls.installed / "bin/rustc", "--edition=2024", "--cfg", 'target_os="trueos"',
                       "-Aexplicit_builtin_cfgs_in_flags", "-C", f"opt-level={optimization}",
                       harness, "-o", binary]
            subprocess.run(list(map(str, command)), env=cls.env, check=True,
                           text=True, capture_output=True)
            cls.binaries.append(binary)

    def run_cases(self, cases):
        for binary in self.binaries:
            for case in cases:
                with self.subTest(optimization=binary.name, case=case):
                    result = subprocess.run([str(binary), case], env=self.env,
                                            text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_native_error_strings_preserve_signed_errno(self):
        self.run_cases(("error_strings",))

    def test_native_panic_output_preserves_bytes_counts_and_errors(self):
        self.run_cases(("forward_count", "partial_writes", "write_zero", "corrupt_count",
                        "extreme_count", "empty_write", "formatted_diagnostic"))


if __name__ == "__main__":
    unittest.main()
