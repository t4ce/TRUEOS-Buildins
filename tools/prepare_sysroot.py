#!/usr/bin/env python3
"""Expose the shared TRUEOS private sysroot recipe to other app builders."""
from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys

import build


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--kernel", type=Path, required=True)
    parser.add_argument("--rustc", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args(argv)
    output = args.output.expanduser().resolve()
    kernel = args.kernel.expanduser().resolve()
    # Preserve the executable name if the caller supplied a rustup proxy symlink.
    rustc = args.rustc.expanduser().absolute()
    result = args.result.expanduser().resolve()
    env = dict(os.environ)
    libraries = rustc.parent.parent / "lib"
    if libraries.is_dir():
        # Cargo may have injected another toolchain's rustc driver directory.
        env["LD_LIBRARY_PATH"] = str(libraries) + (":" + env["LD_LIBRARY_PATH"]
                                                     if env.get("LD_LIBRARY_PATH") else "")
    version = build.capture([rustc, "-vV"], env=env)
    commit = next((line.removeprefix("commit-hash: ") for line in version.splitlines()
                   if line.startswith("commit-hash: ")), None)
    if commit != build.COMMIT:
        raise ValueError("the TRUEOS app build requires the pinned rustc commit " + build.COMMIT)

    output.mkdir(parents=True, exist_ok=True)
    with (output / "prepare.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        sysroot = build.private_sysroot(output, kernel, rustc, env).resolve()
        wrapper = output / "rustc-app"
        wrapper_source = "#!/usr/bin/env python3\nimport os, sys\n"
        if libraries.is_dir():
            wrapper_source += (f"os.environ['LD_LIBRARY_PATH'] = {str(libraries)!r} + "
                               "(':' + os.environ['LD_LIBRARY_PATH'] if os.environ.get('LD_LIBRARY_PATH') else '')\n")
        wrapper_source += f"os.execv({str(rustc)!r}, [{str(rustc)!r}, '--sysroot', {str(sysroot)!r}, *sys.argv[1:]])\n"
        build.write_changed(wrapper, wrapper_source.encode())
        wrapper.chmod(0o755)
        build.write_changed(result, (json.dumps({"sysroot": str(sysroot),
                                                 "rustc_wrapper": str(wrapper)}, indent=2) + "\n").encode())


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"prepare-sysroot: {error}")
