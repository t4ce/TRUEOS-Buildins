#!/usr/bin/env python3
"""Check native Blueprint completion ordering and its no_std object boundary."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

import build


ENTRY = build.ROOT / "vendor/trueos/src/entry.rs"
VSHELL = build.ROOT / "vendor/trueos-v/src/vshell.rs"
ABI = build.ROOT / "vendor/trueos-v/src/bp_abi.rs"
SYMBOLS = {"trueos_cabi_blueprint_return_to_cli", "trueos_cabi_blueprint_shutdown"}


def function_item(source, name):
    start = re.search(r"pub fn " + re.escape(name) + r"\(", source).start()
    opening = source.index("{", start)
    depth, cursor = 1, opening + 1
    while depth:
        depth += (source[cursor] == "{") - (source[cursor] == "}")
        cursor += 1
    return source[start:cursor]


HOST = r'''
#[path = SOURCE]
mod entry;
mod vshell {
    use std::sync::Mutex;
    pub static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    pub struct ShutdownTrap;
    pub fn leave_terminal_handoff() {
        EVENTS.lock().unwrap().push("terminal released".into());
        if std::env::args().nth(1).as_deref() == Some("fallback") { println!("terminal released"); }
    }
    pub fn shutdown_current_blueprint(reason: &str) -> bool {
        let mut events = EVENTS.lock().unwrap();
        assert_eq!(*events, ["terminal released"], "release must precede shutdown exactly once");
        assert_eq!(reason, "blueprint main returned");
        events.push(format!("shutdown: {reason}"));
        drop(events);
        if std::env::args().nth(1).as_deref() == Some("fallback") {
            use std::io::Write;
            println!("shutdown returned: {reason}");
            std::io::stdout().flush().unwrap();
            return false;
        }
        std::panic::resume_unwind(Box::new(ShutdownTrap))
    }
}
fn main() {
    if std::env::args().nth(1).as_deref() == Some("fallback") { entry::finish(); }
    let outcome: Result<(), _> = std::panic::catch_unwind(|| entry::finish());
    assert!(outcome.unwrap_err().is::<vshell::ShutdownTrap>());
    assert_eq!(*vshell::EVENTS.lock().unwrap(),
        ["terminal released", "shutdown: blueprint main returned"]);
}
'''


@unittest.skipUnless(shutil.which("rustup") and shutil.which("nm"),
                     "entry compilation requires the pinned Rust toolchain and nm")
class SdkNativeEntry(unittest.TestCase):
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
        cls.temporary = tempfile.TemporaryDirectory(prefix="trueos-sdk-entry-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        harness = cls.root / "entry-host.rs"
        harness.write_text(HOST.replace("SOURCE", json.dumps(str(ENTRY))))
        cls.binaries = []
        for optimization in (0, 3):
            binary = cls.root / f"entry-o{optimization}"
            subprocess.run(list(map(str, [cls.rustc, "--edition=2024", "-C", f"opt-level={optimization}",
                                         harness, "-o", binary])), env=cls.env, check=True,
                           text=True, capture_output=True)
            cls.binaries.append(binary)

    def test_terminal_is_released_before_nonreturning_shutdown(self):
        for binary in self.binaries:
            with self.subTest(optimization=binary.name):
                subprocess.run([str(binary)], env=self.env, check=True, text=True, capture_output=True)

    def test_shutdown_return_uses_nonreturning_spin_fallback(self):
        for binary in self.binaries:
            with self.subTest(optimization=binary.name):
                with subprocess.Popen([str(binary), "fallback"], env=self.env,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
                    try:
                        stdout, stderr = process.communicate(timeout=0.5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        stdout, stderr = process.communicate()
                    else:
                        self.fail(f"completion returned or terminated: {process.returncode}: {stdout} {stderr}")
                    self.assertEqual(stdout.splitlines(),
                                     ["terminal released", "shutdown returned: blueprint main returned"])
                    self.assertEqual(stderr, "")

    def test_no_std_object_uses_existing_native_lifecycle_imports(self):
        source = VSHELL.read_text()
        declarations = []
        for symbol in sorted(SYMBOLS):
            matches = re.findall(r"^\s*(pub fn " + symbol + r"\([^;]+;)", ABI.read_text(), re.M)
            self.assertEqual(len(matches), 1)
            declarations.append(matches[0])
        probe = self.root / "entry-native.rs"
        # Exercise the real facade's legacy-lease fallback; normal typed lease
        # semantics are independently tested by the terminal lease suite.
        probe.write_text('''#![no_std]
#![allow(dead_code)]
#[path = ''' + json.dumps(str(ENTRY)) + ''']
mod entry;
mod vshell {
    mod vcabi { unsafe extern "C" { ''' + "\n".join(declarations) + ''' } }
    struct TerminalLease;
    impl TerminalLease { fn release_to_shell(&self) -> Result<(), ()> { Err(()) } }
    fn terminal_initial_lease() -> Result<TerminalLease, ()> { Err(()) }
''' + function_item(source, "leave_terminal_handoff") + "\n" +
            function_item(source, "shutdown_current_blueprint") + '''
}
#[unsafe(no_mangle)]
pub fn native_entry_probe() -> ! { entry::finish() }
''')
        for optimization in (0, 3):
            with self.subTest(optimization=optimization):
                output = self.root / f"entry-native-o{optimization}.o"
                subprocess.run(list(map(str, [self.rustc, "--edition=2024", "--crate-type=rlib",
                                              "--emit=obj", "-C", f"opt-level={optimization}",
                                              probe, "-o", output])), env=self.env, check=True,
                               text=True, capture_output=True)
                symbols = subprocess.check_output(["nm", "-u", str(output)], text=True)
                imported = {line.split()[-1] for line in symbols.splitlines() if line.split()}
                self.assertEqual(imported, SYMBOLS, symbols)


if __name__ == "__main__":
    unittest.main()
