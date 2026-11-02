#!/usr/bin/env python3
"""Compare ELF layout variants without changing or publishing the normal app.

Run after preparing the Trauma builtin. Candidates and a JSON receipt are written below
`target/layout-experiment`. Validation checks file contents and relocations;
it does not execute the TRUEOS application.
"""
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
BUILDINS = ROOT.parent
KERNEL = BUILDINS.parent / "TRUEOS"
# Use the shared builtins validator rather than carrying an app-specific copy.
import argparse
import sys
sys.path.insert(0, str(ROOT.parent / "tools"))
from build import compress_module
from elf_layout import Elf, freeze_merge_sections, validate


def pack(elf, output, header):
    compressed = compress_module(elf.data)
    entry = elf.entry()
    packed = struct.pack("<4sHHQII", header[0], header[1], header[2],
                         (entry.section << 32) | entry.value, len(compressed), len(elf.data)) + compressed
    output.write_bytes(packed)
    return len(packed)


def main():
    output = ROOT / "target/layout-experiment"
    output.mkdir(parents=True, exist_ok=True)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-output", type=Path, default=KERNEL / "tgt/buildins/release")
    args = parser.parse_args()
    work = args.build_output / "objects/trauma"
    source = work / "module.debug-stripped.o"
    if not source.is_file():
        source = work / "module.stripped.o"
    blueprint = args.build_output / "dist/trauma.bp"
    shutil.copyfile(source, output / "baseline.o")
    shutil.copyfile(blueprint, output / "baseline.bp")
    original_bp = (output / "baseline.bp").read_bytes()
    header = struct.unpack_from("<4sHHQII", original_bp)
    assert header[:3] == (b"TRBP", 1, 3), "expected ordinary tar/LZ4 Blueprint"
    baseline = Elf(output / "baseline.o")
    assert header[4] + 24 == len(original_bp)
    linker_script = BUILDINS / "tools/rel-coalesce.ld"
    toolchain = tomllib.loads((BUILDINS / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    frozen = output / "layout-input.o"
    freeze_merge_sections(baseline, frozen)
    sysroot = Path(subprocess.check_output(["rustc", f"+{toolchain}", "--print", "sysroot"], text=True).strip())
    linker = sysroot / "lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld/ld.lld"
    receipt = {"builtin_sha256": hashlib.sha256(original_bp).hexdigest(),
               "baseline_elf_sha256": hashlib.sha256(baseline.data).hexdigest(), "variants": {}}
    for name, coalesce, discard in [("baseline", False, False), ("locals", False, True),
                                      ("coalesce", True, False), ("compact", True, True)]:
        elf_path = output / f"{name}.o"
        if coalesce:
            command = [str(linker), "-r", "--no-relax", "-T", str(linker_script)]
            if discard:
                command.append("--discard-all")
            subprocess.run(command + ["-o", str(elf_path), str(frozen)], check=True)
        elif discard:
            subprocess.run(["objcopy", "--discard-all", str(output / "baseline.o"), str(elf_path)], check=True)
        elf = Elf(elf_path)
        checks = validate(baseline, elf, coalesce)
        candidate = output / f"trauma.{name}.bp"
        packed_bytes = pack(elf, candidate, header)
        stats = elf.stats() | checks | {"bp_bytes": packed_bytes, "sha256": hashlib.sha256(candidate.read_bytes()).hexdigest()}
        receipt["variants"][name] = stats
        print(f"{name:10} ELF {len(elf.data):>9,} B | BP {packed_bytes:>9,} B | sections {len(elf.sections):>6,}")
    (output / "results.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"Validated candidates in {output}; normal pack and publish settings unchanged.")


if __name__ == "__main__":
    main()
