#!/usr/bin/env python3
"""Build the kernel-owned app.db inputs. No publishing or independent app releases.

Cargo builds and tracks complete relocatable objects; this driver stages target
adaptations, strips debug information, and writes the existing TRBP containers.
All generated sources, Cargo locks, caches and packages live below --output.
"""
from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
TOOLCHAIN = "nightly-2026-07-10"
COMMIT = "af3d95584dbddcae597890340995509a7fb47a50"
IGNORED = {".git", "target", "tgt", "__pycache__", "Cargo.lock"}
KERNEL_PLATFORM_SOURCES = {"hyper": "hyper-1.10.0", "mio": "mio-1.2.0", "httpdate": "httpdate-1.0.3",
                           "getrandom": "getrandom-0.4.3", "rand_core": "rand_core-0.6.4"}
KERNEL_OVERLAYS = {"crc32fast", "futures-util", "http", "http-body", "httpdate", "hyper", "mio", "getrandom", "rand_core", "ring", "want"}
# Bump when the stripping or container recipe changes, independently of Rust inputs.
PACKAGE_REVISION = 2


def run(args, **kwargs):
    return subprocess.run(list(map(str, args)), check=True, **kwargs)


def capture(args, **kwargs):
    return subprocess.check_output(list(map(str, args)), text=True, **kwargs).strip()


def write_changed(path: Path, data: bytes, *, durable=False):
    if path.is_file() and path.read_bytes() == data:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".new")
    with temporary.open("wb") as stream:
        stream.write(data)
        if durable:
            stream.flush()
            os.fsync(stream.fileno())
    temporary.replace(path)
    if durable:
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
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


def apply_std_patch(library: Path, patch: Path):
    """Accept pristine or fully patched rust-src, without changing partial inputs."""
    command = ["patch", "--batch", "--forward", "-p1", "-i", str(patch.resolve())]
    forward = subprocess.run([*command, "--dry-run"], cwd=library,
                             text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if forward.returncode == 0:
        run(command, cwd=library, stdout=subprocess.DEVNULL)
        return
    # --forward also prevents patch from silently ignoring --reverse when a
    # source file is still pristine. Every hunk must already be present.
    reverse = subprocess.run([*command, "--dry-run", "--reverse"], cwd=library,
                             text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if reverse.returncode == 0:
        print("buildins: archived rust-src already includes the TRUEOS std patch", flush=True)
        return
    raise ValueError("rust-src does not match the pristine or fully patched TRUEOS std inputs; "
                     "refusing to modify it\nforward patch check:\n" + forward.stdout +
                     "\nreverse patch check:\n" + reverse.stdout)


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
        apply_std_patch(sources / "library", patch)
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
    # std declares native libc/unwinder libraries. TRUEOS supplies those symbols
    # at load time, just as it does for the kernel's freestanding link.
    libraries = output / "system-libraries"
    for name in ("c", "gcc_s"):
        write_changed(libraries / f"lib{name}.a", b"!<arch>\n")
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join([
        "-C", "panic=abort", "--check-cfg=cfg(target_os,values(\"trueos\",\"zkvm\"))",
        "-L", f"native={libraries}",
    ])
    # Some copied app manifests used release-like dev profiles. Rig development
    # uses ordinary incremental, unoptimized code and strips only after linking.
    env["CARGO_PROFILE_DEV_OPT_LEVEL"] = "0"
    env["CARGO_PROFILE_DEV_LTO"] = "false"
    env["CARGO_PROFILE_DEV_CODEGEN_UNITS"] = "256"
    env["CARGO_PROFILE_DEV_SPLIT_DEBUGINFO"] = "off"
    env["CARGO_PROFILE_DEV_STRIP"] = "none"
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
    for package, directory in KERNEL_PLATFORM_SOURCES.items():
        manifest = kernel / "vendor" / directory / "Cargo.toml"
        if not manifest.is_file():
            raise ValueError(f"missing shared kernel source for {package}: {manifest}")
    # These two vendor directories are the complete local platform overlay set.
    # Distinct versions use Cargo patch aliases (not replacements across versions).
    for vendor in (kernel / "vendor", ROOT / "vendor"):
        for manifest in sorted(vendor.glob("*/Cargo.toml")):
            package = tomllib.loads(manifest.read_text()).get("package", {})
            if isinstance(package.get("name"), str) and isinstance(package.get("version"), str):
                if vendor == kernel / "vendor" and package["name"] not in KERNEL_OVERLAYS:
                    continue
                key = (package["name"], package["version"])
                if vendor == ROOT / "vendor" and package["name"] in KERNEL_PLATFORM_SOURCES:
                    continue
                result[key] = manifest.parent
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
                suffix = relative.split("/vendor/", 1)[1]
                dest = ROOT / "vendor" / suffix
                if suffix.split("/", 1)[0] in KERNEL_PLATFORM_SOURCES.values():
                    dest = ROOT.parent / "TRUEOS/vendor" / suffix
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
    if not re.search(r'(?m)^\s*#!\[(?:no_main|cfg_attr\(target_os\s*=\s*"trueos",\s*no_main\))\]', text):
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


def entry_hint(path: Path):
    for line in capture(["readelf", "-Ws", path]).splitlines():
        columns = line.split()
        if len(columns) >= 8 and columns[3] == "FUNC" and columns[-1] == "_start" and columns[6].isdigit():
            return (int(columns[6]) << 32) | int(columns[1], 16)
    raise ValueError(f"missing _start in {path}")


def compress_module(payload: bytes):
    """The normal Blueprint payload: deterministic USTAR inside an LZ4 frame."""
    member = tarfile.TarInfo("module.elf")
    member.size, member.mode, member.mtime = len(payload), 0o644, 0
    archive = member.tobuf(format=tarfile.USTAR_FORMAT) + payload
    archive += b"\0" * ((-len(archive)) % 512 + 1024)
    return lz4_frame(archive)


def lz4_frame(archive: bytes):
    library = ctypes.util.find_library("lz4")
    if library is None:
        raise ValueError("LZ4 packaging requires the host liblz4 library")
    lz4 = ctypes.CDLL(library)
    lz4.LZ4F_compressFrameBound.argtypes = [ctypes.c_size_t, ctypes.c_void_p]
    lz4.LZ4F_compressFrameBound.restype = ctypes.c_size_t
    lz4.LZ4F_compressFrame.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
    lz4.LZ4F_compressFrame.restype = ctypes.c_size_t
    lz4.LZ4F_isError.argtypes = [ctypes.c_size_t]
    capacity = lz4.LZ4F_compressFrameBound(len(archive), None)
    buffer = ctypes.create_string_buffer(capacity)
    size = lz4.LZ4F_compressFrame(buffer, capacity, archive, len(archive), None)
    if lz4.LZ4F_isError(size):
        raise ValueError("LZ4 frame compression failed")
    return buffer.raw[:size]


def input_roots(kernel: Path):
    names = json.loads((ROOT / "buildins.json").read_text())["buildins"]
    return [ROOT / "buildins.json", ROOT / "tools", ROOT / "vendor", *(ROOT / name for name in names),
            *(kernel / "crates" / name for name in ("trueos-io", "trueos-fs", "trueos-crypto")),
            kernel / "vendor/infer", kernel / "crates/trueos-v/src/bp_abi.rs", kernel / "crates/trueos-v/src/worker_abi.rs",
            kernel / "src/hv/blueprint/blueprint.rs", kernel / "src/hv/blueprint/prebind.rs", kernel / "src/unix_compat.rs",
            kernel / "tools/appdb-seed",
            kernel / "Cargo.toml",
            kernel / "tools/testpy/apply_trueos_rust_std_thread_backend.py", kernel / "tools/rust-std/trueos_thread.rs"]


def source_digest(kernel: Path):
    digest = hashlib.sha256()
    for root in input_roots(kernel):
        for path in sorted(root.rglob("*") if root.is_dir() else [root]):
            if path.is_file() and not any(part in IGNORED for part in path.relative_to(root.parent).parts):
                digest.update(str(path).encode() + b"\0" + hashlib.sha256(path.read_bytes()).digest())
    for path in sorted(patches(kernel).values()):
        if path.is_relative_to(kernel):
            for source in sorted(path.rglob("*")):
                if source.is_file() and not any(part in IGNORED for part in source.relative_to(path).parts):
                    digest.update(str(source).encode() + b"\0" + hashlib.sha256(source.read_bytes()).digest())
    return digest.hexdigest()


def verify_abi(kernel: Path, output: Path, names):
    def contracts(path):
        text = re.sub(r"//[^\n]*", "", path.read_text())
        return {match[1]: re.sub(r"\s+", "", text[match.start():text.index(";", match.end())]).replace(",)", ")")
                for match in re.finditer(r"pub fn (trueos_cabi_\w+)\s*\(", text)}
    sdk = contracts(ROOT / "vendor/trueos-v/src/bp_abi.rs")
    host = contracts(kernel / "crates/trueos-v/src/bp_abi.rs")
    loader = (kernel / "src/hv/blueprint/blueprint.rs").read_text()
    unix = (kernel / "src/unix_compat.rs").read_text()
    prebind = (kernel / "src/hv/blueprint/prebind.rs").read_text()
    known = set(re.findall(r'"([^"\n]+)"\s*(?:\||=>)', loader + unix))
    rejected = set()
    for source, function in [(unix, "unsupported_unix_import_reason"), (prebind, "retired_import_error")]:
        body = source.split("fn " + function + "(", 1)[1].split("\n}", 1)[0]
        rejected.update(re.findall(r'"([^"\n]+)"', body))
    for name in names:
        for line in capture(["nm", "-u", output / "objects" / name / "module.stripped.o"]).splitlines():
            symbol = line.split()[-1]
            if symbol in rejected or symbol.startswith("ring_core_"):
                raise ValueError(f"{name} imports a retired or unsupported ABI: {symbol}")
            if symbol.startswith("trueos_cabi_"):
                if symbol not in host or sdk.get(symbol) != host[symbol]:
                    raise ValueError(f"{name}: SDK/kernel CABI declaration mismatch for {symbol}")
            elif symbol not in known:
                raise ValueError(f"{name}: kernel has no literal import binding for {symbol}")


def check_ready(output: Path, kernel: Path, profile: str):
    ready = json.loads((output / "ready.json").read_text())
    names = json.loads((ROOT / "buildins.json").read_text())["buildins"]
    if (output / ".building").exists() or ready["profile"] != profile or ready["sources"] != source_digest(kernel):
        raise ValueError("builtins are stale or the last build failed; prepare the kernel and builtins together")
    if list(ready["packages"]) != names or (output / "buildins.json").read_bytes() != (ROOT / "buildins.json").read_bytes():
        raise ValueError("the prepared builtins do not match buildins.json")
    for name, expected in ready["packages"].items():
        if hashlib.sha256((output / "dist" / f"{name}.bp").read_bytes()).hexdigest() != expected:
            raise ValueError(f"prepared package changed since the complete build: {name}")
    for name, expected in ready["database"].items():
        if hashlib.sha256((output / name).read_bytes()).hexdigest() != expected:
            raise ValueError(f"prepared database changed since the complete build: {name}")
    # Cargo watches source directories as well as existing files, catching additions/deletions.
    for path in [*input_roots(kernel), *patches(kernel).values(), output / "ready.json"]:
        print(f"cargo:rerun-if-changed={path}")


def build_database(output: Path, kernel: Path, cargo: Path, env: dict, names):
    packages = {name: hashlib.sha256((output / "dist" / f"{name}.bp").read_bytes()).hexdigest() for name in names}
    tool = kernel / "tools/appdb-seed"
    kernel_version = tomllib.loads((kernel / "Cargo.toml").read_text())["dependencies"]["redb"]["version"]
    host_version = tomllib.loads((tool / "Cargo.toml").read_text())["dependencies"]["redb"]["version"]
    if kernel_version != host_version or not host_version.startswith("="):
        raise ValueError("kernel and app.db builder must pin the same exact redb version")
    inputs = hashlib.sha256(json.dumps(packages).encode())
    for path in sorted(tool.rglob("*")):
        if path.is_file() and not any(part in IGNORED for part in path.relative_to(tool).parts):
            inputs.update(str(path).encode() + b"\0" + path.read_bytes())
    inputs.update(os.environ.get("SOURCE_DATE_EPOCH", "").encode())
    signature = inputs.hexdigest()
    state = output / "database.json"
    if state.is_file():
        previous = json.loads(state.read_text())
        if previous["inputs"] == signature and all((output / name).is_file() and hashlib.sha256((output / name).read_bytes()).hexdigest() == digest
                                                     for name, digest in previous["database"].items()):
            print("buildins: fresh app.db image", flush=True)
            return previous["database"]
    host_env = dict(env)
    host_env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    host_env.pop("RUSTC_BOOTSTRAP_SYNTHETIC_TARGET", None)
    host_env["RUSTC"] = str(cargo.parent / "rustc")
    host_env["CARGO_TARGET_DIR"] = str(output / "host")
    updated = time.strftime("%Y-%m-%d %H:%MZ", time.gmtime(int(os.environ.get("SOURCE_DATE_EPOCH", time.time()))))
    # An external working directory keeps the host helper out of kernel Cargo config.
    with tempfile.TemporaryDirectory(prefix="trueos-appdb-host-") as directory:
        run([cargo, "run", "--quiet", "--release", "--target", "x86_64-unknown-linux-gnu",
             "--manifest-path", tool / "Cargo.toml", "--", ROOT / "buildins.json", output / "dist", output / "app.db", updated],
            cwd=directory, env=host_env)
    write_changed(output / "app.db.lz4", lz4_frame((output / "app.db").read_bytes()), durable=True)
    database = {name: hashlib.sha256((output / name).read_bytes()).hexdigest() for name in ("app.db", "app.db.lz4")}
    write_changed(state, (json.dumps({"inputs": signature, "database": database}, indent=2) + "\n").encode(), durable=True)
    return database


def build_app(name: str, output: Path, profile: str, cargo: Path, binaries: Path, env: dict, overlays):
    manifest, source, needs_std, features, doc = settings(name)
    staged = stage_app(name, output, manifest, source)
    args = [cargo, "build", "--message-format=json-render-diagnostics",
            "-Zbuild-std=" + ("core,compiler_builtins,alloc,std,panic_abort" if needs_std else "core,compiler_builtins,alloc"),
            "-Zbuild-std-features=compiler-builtins-mem", "-Zjson-target-spec", "--target", ROOT / "tools/trueos-object.json",
            "--manifest-path", staged, "--bin", name]
    config = output / "overlays.toml"
    lines = ["[patch.crates-io]"]
    for index, ((package, version), path) in enumerate(sorted(overlays.items())):
        lines.append(f'overlay_{index} = {{package={json.dumps(package)},path={json.dumps(str(path))}}}')
    write_changed(config, ("\n".join(lines) + "\n").encode())
    args += ["--config", config]
    if profile == "release":
        args += ["--release", "--config", ROOT / "tools/release-profile.toml"]
        # Apply the shared Cargo profile only to builtin compiler processes.
        # The host AppDB helper retains its own release settings.
        env = {key: value for key, value in env.items() if not key.startswith("CARGO_PROFILE_RELEASE_")}
    if features:
        args += ["--features", ",".join(features)]
    work = output / "objects" / name
    package = output / "dist" / f"{name}.bp"
    log = output / f"{name}.log"
    print(f"buildins: checking {name} ({profile})", flush=True)
    checkpoint(output, name, "compile start")
    artifacts = set()
    errors = []
    with log.open("w") as stream:
        # Cargo discovers .cargo/config from its working directory, not --manifest-path.
        # Keep the kernel linker script and kernel build-std settings out of apps.
        process = subprocess.Popen(list(map(str, args)), cwd=tempfile.gettempdir(), env=env,
                                   stdout=subprocess.PIPE, stderr=stream, text=True)
        for line in process.stdout:
            stream.write(line)
            try:
                item = json.loads(line)
            except ValueError:
                continue
            if item.get("reason") == "compiler-message" and item["message"]["level"] == "error":
                errors.append(item["message"].get("rendered", item["message"]["message"]))
            if (item.get("reason") == "compiler-artifact" and item["target"]["name"] == name
                    and "bin" in item["target"]["kind"] and Path(item["manifest_path"]) == staged):
                if item.get("executable"):
                    artifacts.add(Path(item["executable"]))
        if process.wait():
            details = "\n".join(errors)[:20000] if errors else log.read_text()[-16000:]
            raise ValueError(f"{name} compilation failed; {log}\n{details}")
    if len(artifacts) != 1:
        raise ValueError(f"{name}: Cargo must report exactly one executable object, got {len(artifacts)}")
    module = artifacts.pop()
    if module.suffix != ".o" or not module.is_file():
        raise ValueError(f"{name}: Cargo reported an invalid executable object: {module}")
    checkpoint(output, name, "compile complete")
    work.mkdir(parents=True, exist_ok=True)
    stamp = work / "inputs.json"
    stripped = work / "module.stripped.o"
    metadata = doc.get("package", {}).get("metadata", {}).get("trueos-blueprint", {})
    flags = sum(bit for key, value, bit in [("replicatable", True, 1 << 8), ("entry-abi", "argv-v1", 1 << 9),
                ("filesystem-independent", True, 1 << 10), ("fs-scope", "trueosfs", 1 << 11)] if metadata.get(key) == value)
    inputs = {"module": hashlib.sha256(module.read_bytes()).hexdigest(), "flags": flags, "format": PACKAGE_REVISION}
    if package.is_file() and stripped.is_file() and stamp.is_file():
        try:
            previous = json.loads(stamp.read_text())
        except json.JSONDecodeError:
            # The previous driver stored concatenated JSON values here.
            previous = None
        if (isinstance(previous, dict) and previous.get("inputs") == inputs
                and previous.get("package") == hashlib.sha256(package.read_bytes()).hexdigest()
                and previous.get("stripped") == hashlib.sha256(stripped.read_bytes()).hexdigest()):
            print(f"buildins: fresh {name}", flush=True)
            return package
    checkpoint(output, name, "strip start")
    # Relocatable LTO links retain compiler_builtins' excluded LLVM bitcode.
    # Drop that compiler input while keeping native symbols and relocations.
    run([binaries / "rust-objcopy", "--strip-debug", "--remove-section=.llvmbc", "--remove-section=.llvmcmd",
         "--set-section-flags", ".trueos.shaders=readonly", module, stripped], env=env)
    payload = stripped.read_bytes()
    checkpoint(output, name, "compress start")
    compressed = compress_module(payload)
    data = struct.pack("<4sHHQII", b"TRBP", 1, 3 | flags, entry_hint(stripped), len(compressed), len(payload)) + compressed
    write_changed(package, data, durable=True)
    state = {"inputs": inputs, "package": hashlib.sha256(data).hexdigest(), "stripped": hashlib.sha256(payload).hexdigest()}
    write_changed(stamp, (json.dumps(state, indent=2) + "\n").encode(), durable=True)
    print(f"buildins: packaged {name} ({len(data):,} bytes)", flush=True)
    checkpoint(output, name, "package complete")
    return package


def checkpoint(output: Path, name: str, phase: str):
    with (output / "phases.log").open("a") as stream:
        stream.write(f"{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())} pid={os.getpid()} {name}: {phase}\n")
        stream.flush()
        os.fsync(stream.fileno())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=Path, default=ROOT.parent / "TRUEOS")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--profile", choices=("dev", "release"), default="dev")
    parser.add_argument("--app", action="append", help="compile selected apps for diagnostics; no complete-set marker")
    parser.add_argument("--check", action="store_true", help="verify the complete prepared set without building")
    args = parser.parse_args()
    output, kernel = args.output.resolve(), args.kernel.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if args.check:
        check_ready(output, kernel, args.profile)
        return
    lock = (output / "build.lock").open("w")
    fcntl.flock(lock, fcntl.LOCK_EX)
    names = json.loads((ROOT / "buildins.json").read_text())["buildins"]
    if not names or len(set(names)) != len(names) or any(not re.fullmatch(r"[A-Za-z0-9_-]+", name) for name in names):
        raise ValueError("buildins.json must contain unique, nonempty app names")
    selected = args.app or names
    if any(name not in names for name in selected):
        raise ValueError("unknown built-in app")
    needs_std = any(settings(name)[2] for name in selected)
    before = source_digest(kernel)
    (output / ".building").touch()
    cargo, binaries, env = tool_environment(output, kernel, needs_std)
    overlays = patches(kernel)
    for name in selected:
        build_app(name, output, args.profile, cargo, binaries, env, overlays)
    verify_abi(kernel, output, selected)
    if not args.app:
        checkpoint(output, "app.db", "image start")
        database = build_database(output, kernel, cargo, env, names)
        checkpoint(output, "app.db", "image complete")
    if source_digest(kernel) != before:
        raise ValueError("built-in inputs changed during the build; run the build again")
    if not args.app:
        write_changed(output / "buildins.json", (ROOT / "buildins.json").read_bytes())
        ready = {"profile": args.profile, "sources": before, "database": database, "packages": {
            name: hashlib.sha256((output / "dist" / f"{name}.bp").read_bytes()).hexdigest() for name in names}}
        write_changed(output / "ready.json", (json.dumps(ready, indent=2) + "\n").encode(), durable=True)
        (output / ".building").unlink()
        print(f"buildins: complete set of {len(names)} apps ready for the kernel", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"buildins: {error}")
