#!/usr/bin/env python3
"""Compile actual pinned QuickJS backends and Rust C adapter, then run JS races.

The host supplies only the kernel wait/clock and carrier fixtures. No TRUEOS
boot, syscall implementation, app promotion or dependency-closure claim occurs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess

ENGINE = Path(__file__).resolve().parents[1]
SOURCE = ENGINE / "vendor/quickjs"
TOOLCHAIN = "nightly-2026-07-10"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    records = []

    def run(label, command, *, timeout=180):
        log = output / (label + ".log")
        print(label, flush=True)
        with log.open("w") as stream:
            stream.write(shlex.join(map(str, command)) + "\n")
            stream.flush()
            result = subprocess.run(list(map(str, command)), cwd="/tmp", stdout=stream,
                                    stderr=subprocess.STDOUT, timeout=timeout)
        records.append(dict(label=label, command=list(map(str, command)),
                            log=str(log), exit=result.returncode))
        if result.returncode:
            raise RuntimeError(log.read_text()[-6000:])
        return log

    # Compile the actual Rust engine's TRUEOS branch, including raw API, with
    # its no_std attribute active. This is a host-ABI cfg check; the root's
    # integration build separately checks the real TRUEOS target and resolver.
    run("nostd-engine-raw", ["rustup", "run", TOOLCHAIN, "rustc", "--edition=2024",
        "--crate-name=hull_quickjs", "--crate-type=rlib", "--emit=metadata", "-C", "panic=abort",
        "--cfg", 'target_os="trueos"', "--cfg", 'feature="raw"',
        "-Aexplicit_builtin_cfgs_in_flags", ENGINE / "src/lib.rs", "-o", output / "native-engine.rmeta"])

    for opt in (0, 3):
        work = output / f"O{opt}"
        work.mkdir(exist_ok=True)
        bridge = work / "libnative_wait_fixture.a"
        run(f"O{opt}-rust-bridge", ["rustup", "run", TOOLCHAIN, "rustc", "--edition=2024",
            "--crate-type=staticlib", "-C", f"opt-level={opt}", ENGINE / "tests/fixtures/native_wait_bridge.rs",
            "-o", bridge])
        wrapper = work / "native_wait_object.rs"
        wrapper.write_text('#![no_std]\n#[path=' + json.dumps(str(ENGINE / "src/native_wait.rs")) + ']\nmod production;\n')
        native_object = work / "native_wait.o"
        run(f"O{opt}-nostd-bridge", ["rustup", "run", TOOLCHAIN, "rustc", "--edition=2024",
            "--crate-type=lib", "--emit=obj", "-C", "panic=abort", "-C", f"opt-level={opt}", wrapper, "-o", native_object])
        imports = sorted(line.split()[-1] for line in subprocess.check_output(
            ["nm", "-u", str(native_object)], text=True).splitlines() if line.strip())
        assert imports == sorted(["trueos_tokio_platform_wait_observe", "trueos_tokio_platform_wait_after",
                                  "trueos_tokio_platform_wake_one", "trueos_platform_monotonic_nanos"]), imports
        for backend in ("host", "native"):
            objects = []
            flags = ["cc", *(["-std=gnu11"] if opt == 0 else []), f"-O{opt}", "-D_GNU_SOURCE", "-DHULL_QUICKJS_FULL_POINTERS",
                     '-DCONFIG_VERSION="native-sync-test"', "-I", SOURCE, "-fno-stack-protector"]
            if backend == "native":
                flags.extend(["-DHULL_QUICKJS_NATIVE_WAIT", "-DHULL_QUICKJS_NATIVE_CLOCK",
                    "-DHULL_QUICKJS_NATIVE_ALLOC", "-Dmalloc=hull_quickjs_native_alloc",
                    "-Dcalloc=hull_quickjs_native_calloc", "-Drealloc=hull_quickjs_native_realloc",
                    "-Dfree=hull_quickjs_native_free", "-Dmalloc_usable_size=hull_quickjs_native_alloc_size"])
            for file in ("quickjs.c", "libregexp.c", "libunicode.c", "cutils.c", "dtoa.c"):
                obj = work / (backend + "-" + file + ".o")
                run(f"O{opt}-{backend}-{file}", [*flags, "-c", SOURCE / file, "-o", obj])
                objects.append(obj)
            engine_imports = sorted(set(line.split()[-1] for line in subprocess.check_output(
                ["nm", "-u", str(objects[0])], text=True).splitlines() if line.strip()))
            if backend == "native":
                assert not any(name.startswith("pthread_") for name in engine_imports), engine_imports
                assert "clock_gettime" not in engine_imports, engine_imports
                assert not set(engine_imports).intersection({"malloc", "calloc", "realloc", "free",
                    "malloc_usable_size", "gettimeofday", "localtime_r"}), engine_imports
            binary = work / (backend + "-atomics")
            run(f"O{opt}-{backend}-link", [*flags, ENGINE / "tests/native_atomics.c", *objects,
                bridge, "-lm", "-lpthread", "-ldl", "-o", binary])
            log = run(f"O{opt}-{backend}-behavior", [binary], timeout=45)
            records.append(dict(label=f"O{opt}-{backend}-actual-engine-imports", imports=engine_imports,
                                passed_cases=log.read_text().count("PASS ")))
    sources = [ENGINE / "build.rs", *sorted((ENGINE / "src").glob("*.rs")), SOURCE / "quickjs.c",
               SOURCE / "quickjs-thread.h", *sorted((ENGINE / "tests").rglob("*"))]
    receipt = dict(scope=__doc__, toolchain=TOOLCHAIN, records=records,
        sources=[dict(path=str(p), sha256=hashlib.sha256(p.read_bytes()).hexdigest()) for p in sources if p.is_file()])
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print("PASS actual QuickJS host/native O0/O3 and production no_std bridge", flush=True)


if __name__ == "__main__":
    main()
