#!/usr/bin/env python3
"""Check the reusable CLI without copying or modifying an installed toolchain."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


def load_module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


build = load_module("buildins_sysroot", "build.py")
with patch.dict(sys.modules, {"build": build}):
    prepare = load_module("prepare_sysroot", "prepare_sysroot.py")


class PrepareSysroot(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.output = self.root / "output with 'quotes'"
        self.kernel = self.root / "kernel"
        self.rustc = self.root / "selected compiler's toolchain/bin/rustc"
        self.sysroot = self.root / "private sysroot's sources"
        self.result = self.root / "results/prepared.json"
        self.args = ["--output", str(self.output), "--kernel", str(self.kernel),
                     "--rustc", str(self.rustc), "--result", str(self.result)]

    def test_missing_or_different_commit_is_rejected_before_preparation(self):
        for version in ("rustc 1.99.0-nightly", "commit-hash: wrong",
                        "commit-hash: " + build.COMMIT + "f"):
            with self.subTest(version=version), \
                 patch.object(build, "capture", return_value=version), \
                 patch.object(build, "private_sysroot") as private:
                with self.assertRaisesRegex(ValueError, build.COMMIT):
                    prepare.main(self.args)
                private.assert_not_called()
                self.assertFalse(self.output.exists())
                self.assertFalse(self.result.exists())

    def test_selected_compiler_and_shared_sysroot_are_used_for_all_wrapper_arguments(self):
        # Execute the wrapper to exercise Python escaping for real path/argument
        # strings, without needing a compiler or a real sysroot.
        self.rustc.parent.mkdir(parents=True)
        self.rustc.write_text(f"#!{sys.executable}\nimport json, os, sys\n"
                              "print(json.dumps({'args': sys.argv, 'libraries': os.environ.get('LD_LIBRARY_PATH')}))\n")
        self.rustc.chmod(0o755)
        libraries = self.rustc.parent.parent / "lib"
        libraries.mkdir()
        with patch.dict(os.environ, {"TRUEOS_PREPARE_TEST": "copied", "LD_LIBRARY_PATH": "/inherited rustup/lib"}), \
             patch.object(build, "capture", return_value="rustc 1.99.0-nightly\ncommit-hash: " + build.COMMIT) as capture, \
             patch.object(build, "private_sysroot", return_value=self.sysroot) as private:
            prepare.main(self.args)
        environment = capture.call_args.kwargs["env"]
        self.assertIsNot(environment, os.environ)
        self.assertEqual(environment["TRUEOS_PREPARE_TEST"], "copied")
        self.assertEqual(environment["LD_LIBRARY_PATH"], str(libraries) + ":/inherited rustup/lib")
        capture.assert_called_once_with([self.rustc, "-vV"], env=environment)
        private.assert_called_once_with(self.output, self.kernel, self.rustc, environment)
        result = json.loads(self.result.read_text())
        wrapper = self.output / "rustc-app"
        self.assertEqual(result, {"sysroot": str(self.sysroot), "rustc_wrapper": str(wrapper)})
        self.assertTrue(wrapper.stat().st_mode & 0o111)
        args = ["--print", "sysroot", "argument with 'quotes' and spaces"]
        # The wrapper must prepend its compiler's libraries to the current
        # runtime environment rather than capturing the caller's old libraries.
        runtime_env = dict(os.environ, LD_LIBRARY_PATH="/runtime toolchain/lib")
        actual = json.loads(subprocess.check_output([str(wrapper), *args], text=True, env=runtime_env))
        self.assertEqual(actual["args"], [str(self.rustc), "--sysroot", str(self.sysroot), *args])
        self.assertEqual(actual["libraries"], str(libraries) + ":/runtime toolchain/lib")
        self.assertFalse(self.sysroot.exists(), "the CLI must delegate source preparation")


if __name__ == "__main__":
    unittest.main()
