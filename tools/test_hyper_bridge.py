#!/usr/bin/env python3
"""Exercise the actual Hyper/Tokio adapters with their shared std contracts.

The host harness includes the kernel's runtime trait sources directly because
the full kernel Hyper fork currently selects its I/O/time exports only on
TrueOS/zkvm. Real target consumer builds validate the complete Cargo wiring.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT.parent / "TRUEOS"


class HyperBridge(unittest.TestCase):
    def test_adapters_preserve_shared_contracts(self):
        overlay = ROOT / "vendor/hyper-util-0.1.20"
        manifest = tomllib.loads((overlay / "Cargo.toml").read_text())
        self.assertIn("std-compat", manifest["dependencies"]["hyper"]["features"])
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        with tempfile.TemporaryDirectory(prefix="trueos-hyper-bridge-") as directory:
            directory = Path(directory)
            (directory / "src/rt").mkdir(parents=True)
            (directory / "hyper-rt/src").mkdir(parents=True)
            (directory / "Cargo.toml").write_text(
                '[package]\nname = "trueos-hyper-bridge-test"\nversion = "0.0.0"\nedition = "2021"\n'
                '[features]\ntracing = []\n'
                '[dependencies]\nhyper = { path = "hyper-rt" }\n'
                f'tokio = {{ path = {json.dumps(str(ROOT / "vendor/tokio-1.54.1"))}, '
                'default-features = false, features = ["rt", "time", "macros", "test-util"] }\n'
                'pin-project-lite = "0.2"\n'
            )
            (directory / "src/lib.rs").write_text('extern crate alloc;\npub mod rt;\n')
            (directory / "src/rt/mod.rs").write_text('pub mod tokio;\n')
            for child in ("tokio.rs", "tokio"):
                source = overlay / "src/rt" / child
                (directory / "src/rt" / child).symlink_to(source, target_is_directory=source.is_dir())
            (directory / "hyper-rt/Cargo.toml").write_text(
                '[package]\nname = "hyper"\nversion = "1.10.0"\nedition = "2021"\n'
            )
            (directory / "hyper-rt/src/lib.rs").write_text(
                'extern crate alloc;\npub use std::{io, time};\n'
                f'#[path = {json.dumps(str(KERNEL / "vendor/hyper-1.10.0/src/rt/mod.rs"))}]\n'
                'pub mod rt;\n'
            )
            env = os.environ.copy()
            env["CARGO_TARGET_DIR"] = str(directory / "target")
            env["RUSTFLAGS"] = env.get("RUSTFLAGS", "") + " -Aunexpected_cfgs"
            env.setdefault("RUST_MIN_STACK", str(16 * 1024 * 1024))
            for platform in (None, "trueos", "zkvm"):
                with self.subTest(platform=platform or "host"):
                    command = ["rustup", "run", toolchain, "cargo", "rustc", "--offline", "--tests",
                               "--message-format=json"]
                    # Apply the branch selection only to the adapter harness;
                    # Tokio and the runtime traits keep their real host target.
                    if platform:
                        command += ["--", "--cfg", f'target_os="{platform}"',
                                    "-Aexplicit_builtin_cfgs_in_flags"]
                    result = subprocess.run(command, check=False, cwd=directory, env=env,
                                            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    messages = [json.loads(line) for line in result.stdout.splitlines()]
                    diagnostics = "\n".join(item["message"].get("rendered", "") for item in messages
                                            if item.get("reason") == "compiler-message")
                    self.assertEqual(result.returncode, 0, result.stderr + diagnostics)
                    executables = [item["executable"] for item in messages
                                   if item.get("reason") == "compiler-artifact"
                                   and item.get("executable") and item["profile"].get("test")]
                    self.assertEqual(len(executables), 1)
                    subprocess.run([executables[0]], check=True, cwd=directory)


if __name__ == "__main__":
    unittest.main()
