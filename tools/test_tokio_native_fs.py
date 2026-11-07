#!/usr/bin/env python3
"""Run real Tokio native filesystem operations against a host broker boundary.

The host process supplies kernel ABI operations, not a replacement filesystem
implementation inside Tokio. Separate native-target linked objects check that
the complete exercised implementation needs core+alloc and native imports only.
"""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

BUILDINS = Path(__file__).resolve().parents[1]
ROOT = BUILDINS.parent / "TRUEOS"
TOKIO = BUILDINS / "vendor/tokio-1.54.1"
FEATURES = ["native", "rt", "rt-multi-thread", "sync", "time", "io-util", "fs", "io-std"]


def environment(target):
    env = dict(os.environ)
    for key in list(env):
        if key.startswith(("CARGO_ENCODED_", "CARGO_TARGET_", "CARGO_PROFILE_", "CARGO_CFG_", "CARGO_FEATURE_")) or key in {
            "RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET",
            "CARGO_BUILD_RUSTFLAGS", "CARGO_MAKEFLAGS", "MAKEFLAGS", "OUT_DIR", "CARGO_MANIFEST_DIR", "CARGO_MANIFEST_PATH",
        }:
            env.pop(key, None)
    env.update(CARGO_TARGET_DIR=str(target), RUSTC_BOOTSTRAP_SYNTHETIC_TARGET="1",
               CARGO_ENCODED_RUSTFLAGS='--check-cfg=cfg(target_os,values("trueos","zkvm"))')
    return env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="trueos-tokio-fs-") as temporary:
        work = Path(temporary)
        output = args.output.resolve() if args.output else work / "logs"
        output.mkdir(parents=True, exist_ok=True)
        checks = []

        def run(label, command, target, *, child=False):
            log = output / (label + ".log")
            env = environment(target)
            env.pop("TRUEOS_MCP_STDIO", None)
            if child:
                env["TRUEOS_MCP_STDIO"] = "1"
            with log.open("w") as stream:
                result = subprocess.run(command, cwd=work, env=env, stdout=stream,
                                        stderr=subprocess.STDOUT, timeout=240)
            checks.append({"label": label, "command": command, "exit": result.returncode, "log": str(log)})
            if result.returncode:
                raise RuntimeError(f"{label} failed\n{log.read_text()[-18000:]}")

        cargo = ["rustup", "run", "nightly-2026-07-10", "cargo"]
        host = work / "host"
        host.mkdir()
        (host / "Cargo.toml").write_text(f'''[package]
name="tokio-native-fs-behavior"
version="0.0.0"
edition="2024"
[workspace]
[[bin]]
name="tokio-native-fs-behavior"
path={json.dumps(str(BUILDINS / 'tools/tokio_native_fs/behavior.rs'))}
[dependencies]
tokio={{path={json.dumps(str(TOKIO))},default-features=false,features={json.dumps(FEATURES)}}}
[profile.dev]
panic="abort"
[profile.release]
panic="abort"
''')
        for label, flags in [("behavior-dev", []), ("behavior-release", ["--release"])]:
            run(label, cargo + ["run", "--offline", "--manifest-path", str(host / "Cargo.toml"),
                               "--target", "x86_64-unknown-linux-gnu", *flags], work / "host-target")
            assert "16 native filesystem boundary cases passed" in (output / (label + ".log")).read_text()
        (host / "Cargo.toml").write_text((host / "Cargo.toml").read_text().replace(
            str(BUILDINS / 'tools/tokio_native_fs/behavior.rs'),
            str(BUILDINS / 'tools/tokio_native_fs/stdio_behavior.rs')))
        for profile, flags in [("dev", []), ("release", ["--release"])]:
            for mode, child, count in [("attached", False, 5), ("child", True, 6)]:
                label = f"stdio-{mode}-{profile}"
                run(label, cargo + ["run", "--offline", "--manifest-path", str(host / "Cargo.toml"),
                                   "--target", "x86_64-unknown-linux-gnu", *flags], work / "host-target", child=child)
                assert f"{count} native {mode} stdio boundary cases passed" in (output / (label + ".log")).read_text()

        probe = work / "probe"
        probe.mkdir()
        (probe / "Cargo.toml").write_text(f'''[package]
name="tokio-native-fs-probe"
version="0.0.0"
edition="2024"
[workspace]
[lib]
path={json.dumps(str(BUILDINS / 'tools/tokio_native_fs/native_probe.rs'))}
[[bin]]
name="tokio-native-fs-linked"
path={json.dumps(str(BUILDINS / 'tools/tokio_native_fs/native_entry.rs'))}
[dependencies]
tokio={{path={json.dumps(str(TOKIO))},default-features=false,features={json.dumps(FEATURES)}}}
trueos={{path={json.dumps(str(BUILDINS / 'vendor/trueos'))},default-features=false,features=["default-global-allocator","default-panic-handler","native-runtime"]}}
[patch.crates-io]
tokio={{path={json.dumps(str(TOKIO))}}}
mio={{path={json.dumps(str(ROOT / 'vendor/mio-1.2.0'))}}}
[profile.dev]
panic="abort"
''')
        target = BUILDINS / "tools/trueos-object-native.json"
        target_flags = ["--target", str(target), "-Zbuild-std=core,compiler_builtins,alloc",
                        "-Zbuild-std-features=compiler-builtins-mem", "-Zjson-target-spec"]
        native_target = work / "native-target"
        for profile, flags in [("dev", []), ("release", ["--release"])]:
            run(f"native-object-{profile}", cargo + ["build", "--offline", "--manifest-path", str(probe / "Cargo.toml"),
                "--bin", "tokio-native-fs-linked", *flags, *target_flags], native_target)
        minimal = work / "minimal"
        minimal.mkdir()
        (minimal / "Cargo.toml").write_text(f'''[package]
name="tokio-native-fs-feature-check"
version="0.0.0"
edition="2024"
[workspace]
[lib]
path="lib.rs"
[dependencies]
tokio={{path={json.dumps(str(TOKIO))},default-features=false}}
[profile.dev]
panic="abort"
''')
        (minimal / "lib.rs").write_text('#![no_std]\npub use tokio::io::{AsyncRead,AsyncWrite};\n')
        for label, selected in [("minimal-native-fs", ["native", "fs"]),
                                ("minimal-native-stdio", ["native", "io-std"])]:
            run(label, cargo + ["check", "--offline", "--manifest-path", str(minimal / "Cargo.toml"),
                "--features", ",".join("tokio/" + feature for feature in selected), *target_flags], native_target)
        module = native_target / target.stem / "release/tokio-native-fs-linked.o"
        symbols = sorted({line.split()[-1] for line in subprocess.check_output(["nm", "-u", str(module)], text=True).splitlines() if line.strip()})
        assert not list((native_target / target.stem).rglob("libstd-*.rlib"))
        assert not list((native_target / target.stem).rglob("liblibc-*.rlib"))
        assert not list((native_target / target.stem).rglob("libsocket2-*.rlib"))
        assert {"trueos_cabi_async_fs_read_start", "trueos_cabi_shell_attached_read_v1"} <= set(symbols)
        sys.path.insert(0, str(ROOT / "tools/testpy"))
        import audit_platform_contracts as contracts
        legacy = set().union(*(values for family, values in contracts.FAMILIES.items() if family not in {
            "rust_system_layout_allocation", "native_execution_tls", "native_generation_wait", "native_named_thread_join",
        }))
        assert not set(symbols) & legacy, sorted(set(symbols) & legacy)
        direct = {}
        for source in [ROOT / "src/unix_compat.rs", ROOT / "src/hv/blueprint/blueprint.rs"]:
            direct.update(contracts.resolver_arms(source))
        abi = (ROOT / "crates/trueos-v/src/bp_abi.rs").read_text()
        declared = set(re.findall(r"\bfn\s+(\w+)\(", abi))
        definitions = set()
        for source in (ROOT / "src").rglob("*.rs"):
            definitions.update(re.findall(r'pub(?:\([^)]*\))?\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+(\w+)\s*\(', source.read_text()))
        unknown = sorted(set(symbols) - direct.keys() - (declared & definitions))
        assert not unknown, unknown
        saved_module = output / "native-fs-stdio.release.o"
        shutil.copy2(module, saved_module)
        cfg = subprocess.check_output(["rustup", "run", "nightly-2026-07-10", "rustc", "-Zunstable-options", "--print", "cfg", "--target", str(target)], cwd=work, text=True).splitlines()
        assert "unix" not in cfg and 'target_env=""' in cfg and not any(x.startswith("target_family=") for x in cfg)
        (output / "receipt.json").write_text(json.dumps({"checks": checks, "native_fs_cases": 16,
            "native_stdio_cases": 11, "native_features": FEATURES, "build_std": ["core", "compiler_builtins", "alloc"],
            "target_cfg": cfg, "imports": symbols, "unknown_source_contracts": unknown,
            "object": {"path": str(saved_module), "bytes": saved_module.stat().st_size,
                       "sha256": hashlib.sha256(saved_module.read_bytes()).hexdigest()},
            "fixtures": [{"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                         for path in sorted((BUILDINS / "tools/tokio_native_fs").glob("*.rs"))],
            "scope": "Actual native Tokio FS+stdio, host kernel-boundary behavior and no-Unix/core+alloc linked objects. No physical broker/console execution claim."}, indent=2) + "\n")
        print(f"PASS: 16 FS + 11 stdio boundary cases/dev+release; 2 no-Unix linked objects; {len(symbols)} native imports")


if __name__ == "__main__":
    main()
