#!/usr/bin/env python3
"""Offline source, differential, Rust and optional no_std target checks."""
import argparse
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="optional installed no_std target, e.g. x86_64-unknown-uefi")
    parser.add_argument("--target-dir", help="optional external Cargo build directory")
    arguments = parser.parse_args()
    commands = [[sys.executable, "tools/source_manifest.py"]]
    commands += [["node", f"tools/{name}.mjs"] for name in ("oracle", "event_oracle", "style_oracle", "ajax_oracle")]
    commands += [["cargo", "fmt", "--check"], ["cargo", "test", "--offline"], ["cargo", "tree", "--offline"]]
    if arguments.target:
        commands += [["cargo", "check", "--offline", "--lib", "--target", arguments.target]]
    for command in commands:
        if arguments.target_dir and command[:2] in (["cargo", "test"], ["cargo", "check"]):
            command += ["--target-dir", arguments.target_dir]
        print("+ " + " ".join(command), flush=True)
        subprocess.run(command, cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
