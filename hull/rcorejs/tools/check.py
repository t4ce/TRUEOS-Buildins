#!/usr/bin/env python3
"""Run the reference check, upstream oracle, Rust tests and optional target check offline."""
import argparse
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="optional installed no_std target")
    parser.add_argument("--target-dir", help="optional Cargo output directory")
    args = parser.parse_args()
    commands = [[sys.executable, "tools/reference.py"], ["node", "tools/oracle.mjs"],
                ["cargo", "fmt", "--check"], ["cargo", "test", "--offline"], ["cargo", "tree", "--offline"]]
    if args.target:
        commands.append(["cargo", "check", "--offline", "--lib", "--target", args.target])
    for command in commands:
        if args.target_dir and command[:2] in (["cargo", "test"], ["cargo", "check"]):
            command += ["--target-dir", args.target_dir]
        print("+ " + " ".join(command), flush=True)
        subprocess.run(command, cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
