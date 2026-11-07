#!/usr/bin/env python3
"""Run the real Termdir model, native I/O, and canonical path tests on the host.

An isolated Cargo working directory avoids inheriting the kernel's build-std
configuration. No kernel, complete builtin set, image, or deployment is built.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib

app = Path(__file__).resolve().parents[1]
kernel = app.parent.parent / "TRUEOS"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output", type=Path, default=kernel / "bld/termdir-native-boundary")
args = parser.parse_args()
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=True)
channel = tomllib.loads((kernel / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
with tempfile.TemporaryDirectory(prefix="termdir-boundary-") as isolated:
    directory = Path(isolated)
    (directory / "Cargo.toml").write_text(f'''[package]
name="trueos-path-host-tests"
version="0.0.0"
edition="2024"
[workspace]
[lib]
path={json.dumps(str(kernel / "crates/trueos-path/src/lib.rs"))}
[dependencies]
typed-path={{version="=0.13.0",default-features=false}}
''')
    for name, manifest in [("model", app / "Cargo.toml"), ("native-io", app / "tests/native-io/Cargo.toml"), ("paths", directory / "Cargo.toml")]:
        log = output / f"{name}.log"
        command = ["cargo", f"+{channel}", "test", "--offline", "--manifest-path", str(manifest), "--target", "x86_64-unknown-linux-gnu", "--target-dir", str(output / name / "target")]
        print(f"termdir: {name} tests -> {log}", flush=True)
        with log.open("w") as handle:
            result = subprocess.run(command, cwd=directory, stdout=handle, stderr=subprocess.STDOUT)
        if result.returncode:
            print(log.read_text())
            raise SystemExit(result.returncode)
print("termdir: all model, native I/O, and UTF-8 path cases passed")
