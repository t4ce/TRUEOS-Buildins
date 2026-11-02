#!/usr/bin/env python3
"""Check portable Trauma sources with a host HTTP backend, not the VMX runtime.

The TRUEOS Reqwest/Hyper forks have platform-only dependencies and incomplete
host branches. This temporary harness uses upstream Reqwest of the same version
without TLS. It retains vendored Tokio/Mio and compiles the real Trauma sources,
examples, and tests. Use the shared buildins driver separately for platform validation.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]


def inline_table(values):
    return "{ " + ", ".join(f"{key} = {json.dumps(value)}" for key, value in values.items()) + " }"


def host_manifest():
    source = tomllib.loads((ROOT / "Cargo.toml").read_text())
    lines = [
        "[package]",
        'name = "trauma"',
        f'version = {json.dumps(source["package"]["version"])}',
        f'edition = {json.dumps(source["package"]["edition"])}',
        "autoexamples = false",
        "[workspace]",
        "[lib]",
        f'path = {json.dumps(str(ROOT / "src/lib.rs"))}',
    ]
    for section in ["dependencies", "dev-dependencies"]:
        lines.append(f"[{section}]")
        for name, spec in source.get(section, {}).items():
            if isinstance(spec, str):
                lines.append(f"{name} = {json.dumps(spec)}")
                continue
            spec = dict(spec)
            if name == "reqwest":
                spec.pop("path", None)
            if "path" in spec:
                spec["path"] = str((ROOT / spec["path"]).resolve())
            lines.append(f"{name} = {inline_table(spec)}")
    lines.append("[features]")
    for name, features in source["features"].items():
        lines.append(f"{name} = {json.dumps(features)}")
    lines += [
        "[patch.crates-io]",
        "tokio = " + inline_table({"path": str(ROOT.parent / "vendor/tokio-1.54.1")}),
        "mio = " + inline_table({"path": str(ROOT.parent.parent / "TRUEOS/vendor/mio-1.2.0")}),
    ]
    for path in sorted((ROOT / "examples").glob("*.rs")):
        lines += ["[[example]]", f"name = {json.dumps(path.stem)}", f"path = {json.dumps(str(path))}"]
        if path.stem == "with-style":
            lines.append('required-features = ["terminal-progress"]')
    for path in sorted((ROOT / "tests").glob("*.rs")):
        lines += ["[[test]]", f"name = {json.dumps(path.stem)}", f"path = {json.dumps(str(path))}"]
    return "\n".join(lines) + "\n"


def main():
    toolchain = tomllib.loads((ROOT.parent / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(ROOT / "target/host-smoke")
    env["RUSTFLAGS"] = env.get("RUSTFLAGS", "") + ' --check-cfg=cfg(target_os,values("trueos","zkvm"))'
    with tempfile.TemporaryDirectory(prefix="trauma-host-") as scratch:
        manifest = Path(scratch) / "Cargo.toml"
        manifest.write_text(host_manifest())
        base = ["cargo", f"+{toolchain}", "test", "--manifest-path", str(manifest), "--no-default-features"]
        for flags in [
            ["--all-targets"],
            ["--all-targets", "--features", "terminal-progress,retry-tracing,socks,system-proxy"],
            ["--doc", "--features", "terminal-progress"],
        ]:
            subprocess.run(base + flags, cwd=ROOT, env=env, check=True)
        subprocess.run(["cargo", f"+{toolchain}", "run", "--manifest-path", str(manifest),
                        "--no-default-features", "--example", "with-resume"],
                       cwd=ROOT, env=env, check=True)
    print("Host checks passed; TRUEOS runtime and TLS behavior require separate validation.")


if __name__ == "__main__":
    main()
