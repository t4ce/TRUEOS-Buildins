#!/usr/bin/env python3
"""Exercise the actual native console with a mocked SDK lease boundary.

The app source is copied with only its target selector changed. Every generated
file and executable stays in the supplied host cache, outside app source.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

APP = Path(__file__).resolve().parents[1]
HOST_TARGET = "x86_64-unknown-linux-gnu"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--toolchain", default="nightly-2026-07-10")
    args = parser.parse_args()
    cache = args.target_dir.resolve()
    command = ["cargo", "+" + args.toolchain, "build", "--offline",
               "--manifest-path", str(APP / "host/Cargo.toml"),
               "--target", HOST_TARGET, "--target-dir", str(cache),
               "--message-format=json"]
    built = subprocess.run(command, cwd=APP, text=True, stdout=subprocess.PIPE, check=True)
    libraries = {}
    for line in built.stdout.splitlines():
        try:
            artifact = json.loads(line)
        except ValueError:
            continue
        name = artifact.get("target", {}).get("name")
        if artifact.get("reason") != "compiler-artifact" or name not in ("tokio", "zeroize"):
            continue
        for filename in artifact.get("filenames", []):
            if filename.endswith(".rlib"):
                libraries[name] = filename
    if set(libraries) != {"tokio", "zeroize"}:
        raise RuntimeError("Host build did not report required actual dependency artifacts")
    fixture_root = cache / "native-console-fixtures"
    fixture_root.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="run-", dir=fixture_root) as temporary:
        work = Path(temporary)
        source = (APP / "src/terminal.rs").read_text()
        (work / "generated_native_terminal.rs").write_text(
            source.replace('target_os = "trueos"', 'feature = "native-fixture"'))
        (work / "console.rs").write_text((APP / "native-tests/console.rs").read_text())
        executable = work / "native-console-tests"
        command = ["rustc", "+" + args.toolchain, "--edition=2024", "--test",
                   "--target", HOST_TARGET, "--cfg", 'feature="native-fixture"',
                   str(work / "console.rs"), "-o", str(executable)]
        for name, filename in sorted(libraries.items()):
            command.extend(["--extern", name + "=" + filename])
        for directory in (cache / HOST_TARGET / "debug/deps", cache / "debug/deps"):
            command.extend(["-L", "dependency=" + str(directory)])
        subprocess.run(command, cwd=APP, check=True)
        subprocess.run([str(executable)], cwd=APP, check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
