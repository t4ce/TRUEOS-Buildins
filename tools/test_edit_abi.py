#!/usr/bin/env python3
"""Check the editor's private FFI layout and flags against the kernel ABI."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT.parent / "TRUEOS"


def constant(source, name):
    match = re.search(rf"const {name}: [^=]+ = ([0-9a-f_xob]+);", source)
    if match is None:
        raise AssertionError(f"missing kernel ABI constant {name}")
    value = match[1].replace("_", "")
    return int(value, 0) if value.startswith(("0x", "0o", "0b")) else int(value)


class EditorAbi(unittest.TestCase):
    def test_layout_and_constants_match_kernel(self):
        terminal = (KERNEL / "src/unix_abi_shim.rs").read_text()
        descriptors = (KERNEL / "src/std_abi_shim.rs").read_text()
        bindings = ROOT / "edit/crates/edit/src/sys/trueos_abi.rs"
        checks = [
            f'assert_eq!(size_of::<abi::termios>(), {constant(terminal, "TRUEOS_TERMIOS_BYTES")});',
            'assert_eq!(size_of::<abi::pollfd>(), 8);',
            'assert_eq!(align_of::<abi::pollfd>(), 4);',
            'assert_eq!(size_of::<abi::winsize>(), 8);',
            'assert_eq!(size_of::<abi::Dl_info>(), 4 * size_of::<usize>());',
        ]
        for field in ("iflag", "oflag", "cflag", "lflag"):
            offset = constant(terminal, f"TRUEOS_TERMIOS_{field.upper()}_OFFSET")
            checks.append(f'assert_eq!(offset_of!(abi::termios, c_{field}), {offset});')
        for name in ("IGNBRK", "BRKINT", "PARMRK", "ISTRIP", "INLCR", "IGNCR", "ICRNL",
                     "IXON", "OPOST", "ISIG", "ICANON", "ECHO", "ECHONL", "IEXTEN",
                     "CSIZE", "CS8", "PARENB", "POLLIN", "TIOCGWINSZ"):
            checks.append(f'assert_eq!(abi::{name}, {constant(terminal, "TRUEOS_" + name)});')
        for name in ("O_NONBLOCK", "EAGAIN", "ENOENT"):
            checks.append(f'assert_eq!(abi::{name}, {constant(descriptors, "TRUEOS_" + name)});')
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            source = directory / "check.rs"
            source.write_text(f'#[path = "{bindings}"] mod abi;\n'
                              'use std::mem::{size_of, align_of, offset_of};\n'
                              'fn main() {\n' + '\n'.join(checks) + '\n}\n')
            binary = directory / "check"
            subprocess.run(["rustup", "run", "nightly-2026-07-10", "rustc", "--edition=2024",
                            "--cfg", 'target_env="musl"', "--cfg", "edit_icu_renaming_auto_detect",
                            "-Aexplicit_builtin_cfgs_in_flags", "-Adead_code",
                            str(source), "-o", str(binary)], check=True, cwd=directory)
            subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    unittest.main()
