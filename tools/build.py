#!/usr/bin/env python3
"""Build the kernel-owned app.db inputs. No publishing or independent app releases.

Cargo owns Rust dependency freshness; this driver stages only target adaptations,
links rustc's selected archive closure, and writes the existing TRBP containers.
All generated sources, Cargo locks, caches and packages live below --output.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
TOOLCHAIN = "nightly-2026-07-10"
COMMIT = "af3d95584dbddcae597890340995509a7fb47a50"
IGNORED = {".git", "target", "tgt", "__pycache__", "Cargo.lock"}
KERNEL_OVERLAYS = {"crc32fast", "futures-util", "http", "http-body", "httpdate", "hyper", "ring", "want"}


def run(args, **kwargs):
    return subprocess.run(list(map(str, args)), check=True, **kwargs)


def capture(args, **kwargs):
    return subprocess.check_output(list(map(str, args)), text=True, **kwargs).strip()


def write_changed(path: Path, data: bytes):
    if path.is_file() and path.read_bytes() == data:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".new")
    temporary.write_bytes(data)
    temporary.replace(path)
    return True


def sync_tree(source: Path, target: Path, transformed=None):
    """Keep unchanged mtimes and remove deleted sources, so Cargo stays incremental."""
    target.mkdir(parents=True, exist_ok=True)
    for old in target.iterdir():
        if old.name not in IGNORED and not (source / old.name).exists():
            shutil.rmtree(old) if old.is_dir() else old.unlink()
    for path in source.iterdir():
        if path.name in IGNORED:
            continue
        dest = target / path.name
        if path.is_dir():
            sync_tree(path, dest, transformed)
        else:
            write_changed(dest, (transformed or {}).get(path, path.read_bytes()))


def private_sysroot(output: Path, kernel: Path, rustc: Path, env: dict) -> Path:
    installed = Path(capture([rustc, "--print", "sysroot"], env=env))
    patch = ROOT / "tools/trueos-std.patch"
    backend = kernel / "tools/testpy/apply_trueos_rust_std_thread_backend.py"
    reference = kernel / "tools/rust-std/trueos_thread.rs"
    key = hashlib.sha256()
    for path in (patch, backend, reference):
        key.update(path.read_bytes())
    key.update(str(installed).encode())
    destination = output / "sysroots" / key.hexdigest()[:16]
    if (destination / ".ready").is_file():
        return destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
        stage = Path(temporary) / "sysroot"
        # Keep the compiler/runtime binaries in the installed, pinned toolchain.
        # Only rust-src is private and writable.
        for path in installed.rglob("*"):
            relative = path.relative_to(installed)
            if relative.parts[:3] == ("lib", "rustlib", "src"):
                continue
            dest = stage / relative
            if path.is_dir():
                dest.mkdir(parents=True, exist_ok=True)
            elif path.is_file():
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.symlink_to(path)
        sources = stage / "lib/rustlib/src/rust"
        shutil.copytree(installed / "lib/rustlib/src/rust/library", sources / "library")
        run(["patch", "--batch", "--forward", "-p1", "-i", patch],
            cwd=sources / "library", stdout=subprocess.DEVNULL)
        run([sys.executable, backend, sources])
        (stage / ".ready").write_text(COMMIT + "\n")
        if destination.exists():
            shutil.rmtree(destination)
        stage.rename(destination)
    return destination


def tool_environment(output: Path, kernel: Path, needs_std: bool):
    env = dict(os.environ)
    # Never inherit the kernel's target, linker script, features or jobserver.
    for key in list(env):
        if key.startswith(("CARGO_ENCODED_", "CARGO_TARGET_", "CARGO_PROFILE_", "CARGO_CFG_", "CARGO_FEATURE_")) or key in {
            "RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTFLAGS", "CARGO_MAKEFLAGS", "MAKEFLAGS",
            "OUT_DIR", "CARGO_MANIFEST_DIR", "CARGO_MANIFEST_PATH",
        }:
            env.pop(key, None)
    installed = Path(capture(["rustup", "run", TOOLCHAIN, "rustc", "--print", "sysroot"], env=env))
    env["LD_LIBRARY_PATH"] = str(installed / "lib") + (":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else "")
    rustc = installed / "bin/rustc"
    if f"commit-hash: {COMMIT}" not in capture([rustc, "-vV"], env=env):
        raise ValueError("the TRUEOS app build requires the pinned rustc commit " + COMMIT)
    sysroot = private_sysroot(output, kernel, rustc, env) if needs_std else installed
    wrapper = output / "rustc-app"
    write_changed(wrapper, ("#!/usr/bin/env python3\nimport os, sys\n"
                           f"os.execv({str(rustc)!r}, [{str(rustc)!r}, '--sysroot', {str(sysroot)!r}, *sys.argv[1:]])\n").encode())
    wrapper.chmod(0o755)
    env["RUSTC"] = str(wrapper)
    env["RUSTC_BOOTSTRAP_SYNTHETIC_TARGET"] = "1"
    env["CARGO_TARGET_DIR"] = str(output / "cargo")
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(["-C", "panic=abort", "--check-cfg=cfg(target_os,values(\"trueos\",\"zkvm\"))"])
    env["CARGO_PROFILE_RELEASE_LTO"] = "false"
    env["CARGO_PROFILE_DEV_PANIC"] = "abort"
    env["CARGO_PROFILE_RELEASE_PANIC"] = "abort"
    env["CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO"] = "off"
    env["CARGO_PROFILE_RELEASE_STRIP"] = "none"
    # Ring's cc build needs a freestanding target and no host stack protector.
    env["CFLAGS"] = env.get("CFLAGS", "") + " -fno-stack-protector"
    binaries = installed / "lib/rustlib/x86_64-unknown-linux-gnu/bin"
    return installed / "bin/cargo", binaries, env


def patches(kernel: Path):
    result = {}
    # These two vendor directories are the complete local platform overlay set.
    # Distinct versions use Cargo patch aliases (not replacements across versions).
    for vendor in (kernel / "vendor", ROOT / "vendor"):
        for manifest in sorted(vendor.glob("*/Cargo.toml")):
            package = tomllib.loads(manifest.read_text()).get("package", {})
            if isinstance(package.get("name"), str) and isinstance(package.get("version"), str):
                if vendor == kernel / "vendor" and package["name"] not in KERNEL_OVERLAYS:
                    continue
                result[(package["name"], package["version"])] = manifest.parent
    return result


def staged_manifest(path: Path, original: Path):
    source = original.read_text()
    table = ""
    lines = []
    skip = False
    for line in source.splitlines():
        if line.strip().startswith("["):
            table = line.strip().strip("[]")
            skip = table.startswith("patch.")
        if skip:
            continue
        # img/paper inherited their SDK dependency from the old Blueprint workspace.
        if re.match(r"\s*trueos\s*=.*workspace\s*=\s*true", line):
            line = re.sub(r"workspace\s*=\s*true", f"path = {json.dumps(str(ROOT / 'vendor/trueos'))}", line)
        dependency = table.endswith("dependencies") or ".dependencies." in table
        if dependency and (match := re.search(r'\bpath\s*=\s*"([^"]+)"', line)):
            relative = match[1]
            if relative.endswith("/api"):
                dest = ROOT / "vendor/trueos"
            elif "/vendor/" in relative:
                dest = ROOT / "vendor" / relative.split("/vendor/", 1)[1]
                if not dest.exists():
                    dest = ROOT.parent / "TRUEOS/vendor" / relative.split("/vendor/", 1)[1]
            else:
                # Keep app-internal dependencies inside the stable staged tree.
                dest = (path.parent / relative).resolve()
            line = line[:match.start(1)] + str(dest) + line[match.end(1):]
        lines.append(line)
    if "[workspace]" not in source and original.parent == ROOT / original.parent.name:
        lines += ["", "[workspace]"]
    return ("\n".join(lines) + "\n").encode()


def settings(name: str):
    manifest = ROOT / name / ("crates/edit/Cargo.toml" if name == "edit" else "Cargo.toml")
    doc = tomllib.loads(manifest.read_text())
    binary = doc.get("bin", [{}])[0]
    source = manifest.parent / binary.get("path", "src/main.rs")
    text = source.read_text()
    features = set()
    for match in re.finditer(r'trueos-blueprint:\s*features\s*=\s*(\[[^\]]*\])', text):
        features.update("trueos/" + feature for feature in json.loads(match[1]))
    uses_runtime = any(token in text for token in ("runtime", "worker", "task::", "tokio::"))
    needs_std = "#![no_std]" not in text or uses_runtime
    if uses_runtime:
        features.add("trueos/tokio-runtime")
    if any(token in text for token in ("trueos::net", "tokio::net", "current_thread_net")):
        features.add("trueos/tokio-net-probe")
    if "#[global_allocator]" not in text:
        features.add("trueos/default-global-allocator")
    if not needs_std and "#[panic_handler]" not in text:
        features.add("trueos/default-panic-handler")
    return manifest, source, needs_std, sorted(features), doc


def stage_app(name: str, output: Path, manifest: Path, source: Path):
    stage = output / "sources" / name
    transformed = {}
    for original in (ROOT / name).rglob("Cargo.toml"):
        if not any(part in IGNORED for part in original.relative_to(ROOT / name).parts):
            transformed[original] = staged_manifest(stage / original.relative_to(ROOT / name), original)
    text = source.read_text()
    if "#![no_main]" not in text and 'cfg_attr(target_os = "trueos", no_main)' not in text:
        text = '#![no_main]\n' + text + '''
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = main();
    unsafe extern "C" { fn exit(code: core::ffi::c_int) -> !; }
    unsafe { exit(0) }
}
'''
        transformed[source] = text.encode()
    sync_tree(ROOT / name, stage, transformed)
    return stage / manifest.relative_to(ROOT / name)


def selected_inputs(rlink: Path):
    """Read the pinned rustc .rlink's ordered paths, never glob cached archives."""
    archives, objects = [], []
    for raw in re.findall(rb"[!-~]+", rlink.read_bytes()):
        token = raw.decode()
        if ".rlib" in token:
            token = token[:token.index(".rlib") + 5]
            path = Path(token[token.index("/"):]) if "/" in token else rlink.parent / token[token.rfind("lib"):]
            if path.is_file() and path not in archives:
                archives.append(path)
        elif token.endswith(".o") and "/" in token:
            path = Path(token[token.index("/"):])
            if path.name.startswith(rlink.stem) and path.is_file() and path not in objects:
                objects.append(path)
    if not archives or not objects:
        raise ValueError(f"cannot recover the selected objects and archives from {rlink}")
    if any(path.name.startswith("libpanic_unwind-") for path in archives):
        raise ValueError("app selected panic_unwind despite panic=abort")
    return objects, archives


def entry_hint(path: Path):
    for line in capture(["readelf", "-Ws", path]).splitlines():
        columns = line.split()
        if len(columns) >= 8 and columns[3] == "FUNC" and columns[-1] == "_start" and columns[6].isdigit():
            return (int(columns[6]) << 32) | int(columns[1], 16)
    raise ValueError(f"missing _start in {path}")


def build_app(name: str, output: Path, profile: str, cargo: Path, binaries: Path, env: dict, overlays):
    manifest, source, needs_std, features, doc = settings(name)
    staged = stage_app(name, output, manifest, source)
    args = [cargo, "rustc", "--message-format=json-render-diagnostics",
            "-Zbuild-std=" + ("core,compiler_builtins,alloc,std,panic_abort" if needs_std else "core,compiler_builtins,alloc"),
            "-Zjson-target-spec", "--target", ROOT / "tools/trueos-app.json",
            "--manifest-path", staged, "--bin", name]
    config = output / "overlays.toml"
    lines = ["[patch.crates-io]"]
    for index, ((package, version), path) in enumerate(sorted(overlays.items())):
        lines.append(f'overlay_{index} = {{package={json.dumps(package)},path={json.dumps(str(path))}}}')
    write_changed(config, ("\n".join(lines) + "\n").encode())
    args += ["--config", config]
    if profile == "release":
        args.append("--release")
    if features:
        args += ["--features", ",".join(features)]
    args += ["--", "-Zno-link", "--emit=obj"]
    log = output / f"{name}.log"
    print(f"buildins: checking {name} ({profile})", flush=True)
    artifacts = []
    errors = []
    with log.open("w") as stream:
        process = subprocess.Popen(list(map(str, args)), cwd=staged.parent, env=env,
                                   stdout=subprocess.PIPE, stderr=stream, text=True)
        for line in process.stdout:
            stream.write(line)
            try:
                item = json.loads(line)
            except ValueError:
                continue
            if item.get("reason") == "compiler-message" and item["message"]["level"] == "error":
                errors.append(item["message"].get("rendered", item["message"]["message"]))
            if item.get("reason") == "compiler-artifact" and item["target"]["name"] == name and "bin" in item["target"]["kind"]:
                artifacts.extend(item["filenames"])
        if process.wait():
            details = "\n".join(errors)[:20000] if errors else log.read_text()[-16000:]
            raise ValueError(f"{name} compilation failed; {log}\n{details}")
    # rustc's no-link artifact is a .rlink; Cargo may report the executable stem.
    candidates = [Path(filename).with_suffix(".rlink") for filename in artifacts]
    deps = output / "cargo/trueos-app" / ("debug" if profile == "dev" else "release") / "deps"
    candidates += sorted(deps.glob(name.replace("-", "_") + "-*.rlink"), key=lambda p: p.stat().st_mtime_ns, reverse=True)
    rlink = next((path for path in candidates if path.is_file()), None)
    if rlink is None:
        raise ValueError(f"{name}: Cargo produced no selected .rlink")
    objects, archives = selected_inputs(rlink)
    state = [[str(path), path.stat().st_size, path.stat().st_mtime_ns] for path in [Path(__file__), ROOT / "tools/trueos-app.json", *objects, *archives]]
    state_bytes = json.dumps(state).encode()
    work = output / "objects" / name
    work.mkdir(parents=True, exist_ok=True)
    stamp = work / "inputs.json"
    package = output / "dist" / f"{name}.bp"
    metadata = doc.get("package", {}).get("metadata", {}).get("trueos-blueprint", {})
    flags = sum(bit for key, value, bit in [("replicatable", True, 1 << 8), ("entry-abi", "argv-v1", 1 << 9),
                ("filesystem-independent", True, 1 << 10), ("fs-scope", "trueosfs", 1 << 11)] if metadata.get(key) == value)
    state_bytes += json.dumps(flags).encode()
    if package.is_file() and stamp.is_file() and stamp.read_bytes() == state_bytes:
        print(f"buildins: fresh {name}", flush=True)
        return package
    module = work / "module.o"
    run([binaries / "rust-lld", "-flavor", "gnu", "-r", "--gc-sections", "--undefined=_start", "-o", module,
         *objects, "--start-group", *archives, "--end-group"], env=env)
    stripped = work / "module.stripped.o"
    run([binaries / "rust-objcopy", "--strip-debug", "--set-section-flags", ".trueos.shaders=readonly", module, stripped], env=env)
    payload = stripped.read_bytes()
    # TRBP's existing raw-ELF payload (kind 1), consumed by the normal loader.
    # No catalog, publisher, alternate launcher, or package identity is needed.
    data = struct.pack("<4sHHQII", b"TRBP", 1, 1 | flags, entry_hint(stripped), len(payload), len(payload)) + payload
    write_changed(package, data)
    write_changed(stamp, state_bytes)
    print(f"buildins: linked {name} ({len(data):,} bytes)", flush=True)
    return package


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, default=ROOT.parent / "TRUEOS")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profile", choices=("dev", "release"), default="dev")
    parser.add_argument("--app", action="append", help="compile selected apps for diagnostics; no complete-set marker")
    args = parser.parse_args()
    output, kernel = args.output.resolve(), args.kernel.resolve()
    output.mkdir(parents=True, exist_ok=True)
    names = json.loads((ROOT / "buildins.json").read_text())["buildins"]
    if not names or len(set(names)) != len(names) or any(not re.fullmatch(r"[A-Za-z0-9_-]+", name) for name in names):
        raise ValueError("buildins.json must contain unique, nonempty app names")
    selected = args.app or names
    if any(name not in names for name in selected):
        raise ValueError("unknown built-in app")
    needs_std = any(settings(name)[2] for name in selected)
    cargo, binaries, env = tool_environment(output, kernel, needs_std)
    overlays = patches(kernel)
    for name in selected:
        build_app(name, output, args.profile, cargo, binaries, env, overlays)
    if not args.app:
        write_changed(output / "buildins.json", (ROOT / "buildins.json").read_bytes())
        print(f"buildins: complete set of {len(names)} apps ready for the kernel", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"buildins: {error}")
