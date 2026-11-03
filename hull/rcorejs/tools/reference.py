#!/usr/bin/env python3
"""Verify the complete vendored source tree against its pinned fingerprint."""
import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PIN = ROOT / "UPSTREAM.json"


def fingerprint(source):
    tree = hashlib.sha256()
    count = 0
    for path in sorted(source.rglob("*")):
        if path.is_file():
            relative = path.relative_to(source).as_posix()
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            tree.update(f"{relative}\0{digest}\n".encode())
            count += 1
    return tree.hexdigest(), count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", action="store_true", help="record an intentionally reviewed source baseline")
    args = parser.parse_args()
    pin = json.loads(PIN.read_text())
    source = ROOT / f"core-js-{pin['version']}"
    if (source / "LICENSE").read_bytes() != (ROOT / "LICENSE").read_bytes():
        raise SystemExit("Upstream MIT notice differs from crate license")
    if json.loads((source / "package.json").read_text())["version"] != pin["version"]:
        raise SystemExit("Source package version differs from pin")
    digest, count = fingerprint(source)
    if args.record:
        pin["tree_sha256"] = digest
        pin["files"] = count
        PIN.write_text(json.dumps(pin, indent=2) + "\n")
    elif digest != pin.get("tree_sha256") or count != pin["files"]:
        raise SystemExit("Source tree changed; inspect changes before recording a new baseline")
    print(f"Verified {count} upstream files (core-js {pin['version']})")


if __name__ == "__main__":
    main()
