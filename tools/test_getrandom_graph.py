#!/usr/bin/env python3
"""Check that the kernel and all staged built-ins use one local getrandom."""
import argparse
import importlib.util
import json
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT.parent / "TRUEOS"
VERSION = "0.4.3"
CANONICAL = KERNEL / "vendor" / f"getrandom-{VERSION}"


def audit(output):
    errors = []
    sources = []
    for vendor in (KERNEL / "vendor", ROOT / "vendor"):
        for manifest in sorted(vendor.glob("*/Cargo.toml")):
            package = tomllib.loads(manifest.read_text()).get("package", {})
            if package.get("name") == "getrandom":
                sources.append((manifest.parent.resolve(), package.get("version")))
    if sources != [(CANONICAL.resolve(), VERSION)]:
        errors.append(f"expected only vendored {CANONICAL}; found {sources}")
    for version in ("0.2.17", "0.3.4", "0.4.2"):
        obsolete = ROOT / "vendor" / f"getrandom-{version}"
        if obsolete.exists():
            errors.append(f"obsolete source tree remains: {obsolete}")

    # Loading the driver only defines helpers; do not stage sources or run Cargo.
    spec = importlib.util.spec_from_file_location("getrandom_graph_build", ROOT / "tools/build.py")
    driver = importlib.util.module_from_spec(spec)
    previous = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        spec.loader.exec_module(driver)
    finally:
        sys.dont_write_bytecode = previous
    selected = {key: path.resolve() for key, path in driver.patches(KERNEL).items()
                if key[0] == "getrandom"}
    if selected != {("getrandom", VERSION): CANONICAL.resolve()}:
        errors.append(f"build driver must select only the canonical getrandom: {selected}")

    apps = json.loads((ROOT / "buildins.json").read_text())["buildins"]
    locks = [("kernel", KERNEL / "Cargo.lock")]
    locks += [(app, output / "sources" / app / "Cargo.lock") for app in apps]
    consumers = []
    for name, lock in locks:
        if not lock.is_file():
            errors.append(f"missing {name} lockfile: {lock}")
            continue
        data = tomllib.loads(lock.read_text())
        if data.get("version") not in (3, 4):
            errors.append(f"unsupported lock format in {lock}: {data.get('version')}")
        packages = [package for package in data.get("package", [])
                    if package["name"] == "getrandom"]
        if len(packages) > 1:
            errors.append(f"{name} resolves multiple getrandom packages: {packages}")
        for package in packages:
            if package["version"] != VERSION or "source" in package:
                errors.append(f"{name} must resolve local getrandom {VERSION}: {package}")
        if packages:
            consumers.append(name)
    if errors:
        raise AssertionError("getrandom graph audit failed:\n" + "\n".join(f"- {error}" for error in errors))
    print(f"getrandom {VERSION}: one kernel-owned source; checked kernel and {len(apps)} built-in locks")
    print("Consumers: " + ", ".join(consumers))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=KERNEL / "tgt/buildins/dev",
                        help="built-in build output directory (default: %(default)s)")
    args = parser.parse_args()
    try:
        audit(args.output.resolve())
    except (AssertionError, OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
