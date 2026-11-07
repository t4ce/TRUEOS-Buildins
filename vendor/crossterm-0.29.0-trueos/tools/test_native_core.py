#!/usr/bin/env python3
"""Test the actual Crossterm library in desktop and native-core host fixtures.

Only Crossterm receives a forced TRUEOS cfg in the native host fixture. Its
production core/alloc source and shared native runtime execute with test-only
kernel hooks. Real no-Unix target linkage is verified separately by the builtin
driver. Unused example/dev runtimes are omitted from this library-only fixture.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT.parents[1].parent / "TRUEOS"
TOOLCHAIN = "nightly-2026-07-10"


def manifest():
    table = ""
    lines = []
    for line in (ROOT / "Cargo.toml").read_text().splitlines():
        if line.startswith("["):
            table = line.strip("[]")
        if table.startswith(("example", "test")):
            continue
        if table.startswith("dev-dependencies.") and table not in {
            "dev-dependencies.serde_json", "dev-dependencies.temp-env", "dev-dependencies.serial_test"
        }:
            continue
        if table == "lib" and line.startswith("path ="):
            line = "path = " + json.dumps(str(ROOT / "src/lib.rs"))
        elif ("dependencies" in table) and (match := re.search(r'^path = "([^"]+)"$', line)):
            line = "path = " + json.dumps(str((ROOT / match[1]).resolve()))
        lines.append(line)
    lines += ["", "[dependencies.trueos-io]", "path = " + json.dumps(str(KERNEL / "crates/trueos-io")),
              "", "[dependencies.trueos-runtime]", "path = " + json.dumps(str(KERNEL / "crates/trueos-runtime")),
              "", "[workspace]", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mode", choices=("all", "host", "trueos-std", "native"), default="all")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    fixture = output / "fixture"
    fixture.mkdir(exist_ok=True)
    (fixture / "Cargo.toml").write_text(manifest())
    rustc = subprocess.check_output(["rustup", "run", TOOLCHAIN, "which", "rustc"], text=True).strip()
    wrapper = output / "native-rustc"
    wrapper.write_text("#!/usr/bin/env python3\nimport os, sys\n"
                       "args=sys.argv[1:]\n"
                       "if '--crate-name' in args and args[args.index('--crate-name')+1]=='crossterm':\n"
                       "    args+=['--cfg','target_os=\"trueos\"','-Aexplicit_builtin_cfgs_in_flags']\n"
                       f"os.execv({rustc!r},[{rustc!r},*args])\n")
    wrapper.chmod(0o755)
    features = "events,bracketed-paste,event-stream,derive-more,serde,osc52,use-dev-tty,libc"
    runs = []
    for mode in ("host", "trueos-std", "native"):
        if args.mode not in ("all", mode):
            continue
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("CARGO_ENCODED_", "CARGO_BUILD_", "CARGO_TARGET_"))
               and key not in {"RUSTC", "RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"}}
        env["CARGO_TARGET_DIR"] = str(output / ("cargo-" + mode))
        env["RUST_MIN_STACK"] = str(16 * 1024 * 1024)
        if mode != "host":
            env["RUSTC"] = str(wrapper)
        command = ["rustup", "run", TOOLCHAIN, "cargo", "test", "--offline", "--lib",
                   "--target", "x86_64-unknown-linux-gnu", "--manifest-path", str(fixture / "Cargo.toml"),
                   "--config", str(KERNEL / "tgt/buildins/release/overlays.toml"),
                   "--no-default-features", "--features", features + (",native-core" if mode == "native" else ""),
                   "--", "--test-threads=1"]
        log_path = output / (mode + ".log")
        with log_path.open("w") as log:
            log.write("COMMAND " + json.dumps(command) + "\n")
            log.flush()
            result = subprocess.run(command, cwd="/tmp", env=env, stdout=log, stderr=subprocess.STDOUT)
        if result.returncode:
            raise SystemExit(f"{mode} fixture failed ({result.returncode}); {log_path}")
        text = log_path.read_text()
        match = re.search(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", text)
        if match is None:
            raise SystemExit(f"{mode}: missing test completion; {log_path}")
        runs.append({"mode": mode, "passed": int(match[1]), "failed": int(match[2]),
                     "ignored": int(match[3]), "log": str(log_path), "command": command})
    receipt = {"scope": __doc__, "runs": runs,
               "source_sha256": {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in sorted((ROOT / "src").rglob("*.rs"))},
               "native_target_proof": "Separate builtin driver build required; forced host cfg is not target linkage proof."}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"runs": runs, "receipt": str(output / "receipt.json")}, indent=2))


if __name__ == "__main__":
    main()
