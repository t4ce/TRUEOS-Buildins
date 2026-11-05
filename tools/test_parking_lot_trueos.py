#!/usr/bin/env python3
"""Run the production TRUEOS parker against a host model of native wait queues.

No Cargo dependencies or network access are needed. The parker and its traits
come from the vendored production source; only the native platform boundary is
modeled. The model preserves generation changes, realm scoping, spurious wakes,
and the platform's zero=probe / MAX=infinite timeout convention.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
PARKER = ROOT / "vendor/parking_lot_core-0.9.12/src/thread_parker"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--repeat", type=int, default=1)
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be at least 1")

    # Compile the real trait definitions too, rather than maintaining a second
    # copy of the contract. Only the cfg_if backend selection is replaced.
    traits = (PARKER / "mod.rs").read_text().split("cfg_if! {", 1)[0]
    traits = traits.replace("use cfg_if::cfg_if;\n", "")
    module = (
        "mod thread_parker {\n" + traits
        + "\nmod imp {\ninclude!(" + json.dumps(str(PARKER / "trueos.rs")) + ");\n"
        + "pub(super) fn timeout_for_test(duration: std::time::Duration) -> u64 {\n"
        + "    timeout_millis(duration)\n}\n}\n"
        + "pub use imp::{thread_yield, ThreadParker};\n"
        + "pub fn timeout_for_test(duration: std::time::Duration) -> u64 {\n"
        + "    imp::timeout_for_test(duration)\n}\n}\n"
    )
    source = (ROOT / "tools/parking_lot_trueos_tests.rs").read_text()
    source = source.replace("// @PRODUCTION_PARKER_MODULE@", module)
    with tempfile.TemporaryDirectory(prefix="trueos-parker-tests-") as temporary:
        temporary = Path(temporary)
        harness = temporary / "parker_tests.rs"
        binary = temporary / "parker_tests"
        harness.write_text(source)
        for optimization in ("0", "3"):
            subprocess.run([
                args.rustc, "--edition=2021", "--test", str(harness),
                "-C", f"opt-level={optimization}", "-o", str(binary),
            ], check=True, cwd=ROOT, timeout=60)
            for iteration in range(args.repeat):
                print(f"TRUEOS parker: opt-level={optimization}, run {iteration + 1}", flush=True)
                subprocess.run([str(binary), "--test-threads=1"], check=True, timeout=60)


if __name__ == "__main__":
    main()
