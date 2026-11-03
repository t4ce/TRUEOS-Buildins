#!/usr/bin/env python3
"""Capture once, then replay production Hull entirely on the host."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = Path(__file__).resolve().parent
HULL = HERE.parents[1]
TARGET = HULL.parents[1] / "TRUEOS/tgt/hull-sandbox-host"
TOOLCHAIN = "nightly-2026-07-10"
HOST = "x86_64-unknown-linux-gnu"


def binary() -> Path:
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(TARGET)
    result = subprocess.run(["cargo", f"+{TOOLCHAIN}", "build", "--offline", "--quiet", "--manifest-path",
                    str(HERE / "Cargo.toml"), "--target", HOST],
                   cwd=tempfile.gettempdir(), env=env, capture_output=True, text=True)
    if result.returncode:
        sys.stderr.write(result.stdout + result.stderr)
        raise subprocess.CalledProcessError(result.returncode, result.args)
    return TARGET / HOST / "debug/hull-sandbox"


def child_limits() -> None:
    import resource
    resource.setrlimit(resource.RLIMIT_CPU, (15, 15))
    resource.setrlimit(resource.RLIMIT_AS, (1024**3, 1024**3))
    resource.setrlimit(resource.RLIMIT_FSIZE, (64 * 1024**2, 64 * 1024**2))


def execute(executable: Path, capture: Path, out: Path, arguments: list[str]) -> dict:
    if capture.resolve().is_relative_to(out.resolve()):
        raise ValueError("output must not be the capture directory or its parent")
    subprocess.run([str(executable), "--input", str(capture.resolve()), "--out", str(out.resolve()),
                    *arguments], cwd=tempfile.gettempdir(), check=True, timeout=30,
                   preexec_fn=child_limits if os.name == "posix" else None)
    return json.loads((out / "report.json").read_text())


def store(path: Path, value: dict) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


class WebOnlyRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        if urllib.parse.urlsplit(newurl).scheme not in {"http", "https"}:
            raise ValueError("capture redirect must remain HTTP(S)")
        return super().redirect_request(request, response, code, message, headers, newurl)


def fetch(url: str, limit: int, *, accept: str = "*/*") -> tuple[bytes, str, str]:
    if urllib.parse.urlsplit(url).scheme not in {"http", "https"}:
        raise ValueError("capture only accepts HTTP(S)")
    request = urllib.request.Request(url, headers={"User-Agent": "HullSandbox/0.1", "Accept-Encoding": "identity", "Accept": accept})
    with urllib.request.build_opener(WebOnlyRedirects()).open(request, timeout=15) as response:
        body = response.read(limit + 1)
        if len(body) > limit:
            raise ValueError(f"response exceeds {limit} bytes")
        return body, response.geturl(), response.headers.get("Content-Type", "")


def capture_page(args, executable: Path) -> None:
    capture = args.directory.resolve()
    capture.mkdir(parents=True, exist_ok=True)
    if (capture / "capture.json").exists() and not args.refresh:
        raise ValueError("capture already exists; use --refresh to replace its page/resource manifest")
    if args.html:
        body = args.html.read_bytes()
        if len(body) > 4 * 1024**2:
            raise ValueError("HTML exceeds 4 MiB")
        final_url, mime = args.url, "text/html (imported)"
    else:
        body, final_url, mime = fetch(args.url, 4 * 1024**2,
                                     accept="text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
    (capture / "page.html").write_bytes(body)
    manifest = {"version": 1, "url": args.url, "final_url": final_url,
                "content_type": mime, "resources": {}}
    store(capture / "capture.json", manifest)
    total = 0

    def resource(url: str, kind: str) -> None:
        nonlocal total
        if url in manifest["resources"]:
            return
        if len(manifest["resources"]) >= 64:
            return
        entry = {"kind": kind, "status": "failed"}
        try:
            cap = min(4 * 1024**2, 16 * 1024**2 - total)
            if cap <= 0:
                raise ValueError("capture resource budget exceeded")
            data, resolved, content_type = fetch(url, cap)
            name = "resources/" + hashlib.sha256(url.encode()).hexdigest() + "." + kind
            (capture / "resources").mkdir(exist_ok=True)
            (capture / name).write_bytes(data)
            total += len(data)
            entry.update(status="ok", path=name, bytes=len(data), final_url=resolved, content_type=content_type)
            print(f"captured {kind}: {url}")
        except (OSError, ValueError, urllib.error.URLError) as error:
            entry["error"] = str(error)
            print(f"capture failed {kind}: {url}: {error}", file=sys.stderr)
        manifest["resources"][url] = entry
        store(capture / "capture.json", manifest)

    # Production rp5/script extraction and CSS import discovery, not a second
    # HTML parser. Inspect never executes JavaScript, even when files are cached.
    for _ in range(17):
        report = execute(executable, capture, capture / "inspection", ["--inspect"])
        missing = [url for url in report["css_requests"] if url not in manifest["resources"]]
        if args.no_css or not missing:
            break
        for url in missing[:16]:
            resource(url, "css")
    if args.scripts:
        for source in report["scripts"]:
            if source["file"] is None:
                resource(source["origin"], "js")
    # Optional module graph capture uses QuickJS's parser and the production URL
    # policy. No regex import scanner and no downloaded JavaScript execution.
    def module_tool(mode, value, base):
        process = subprocess.run([str(executable), mode, str(value), base, str(capture)],
                                 cwd=tempfile.gettempdir(), capture_output=True,
                                 text=True, timeout=10)
        if process.returncode:
            raise ValueError(process.stderr.strip() or f"module helper failed: {process.returncode}")
        return json.loads(process.stdout)

    seen_modules = set()

    def module_graph(candidates, depth=0):
        if depth > 32:
            return False
        for url in candidates:
            if url in seen_modules:
                if manifest["resources"].get(url, {}).get("module_status") in {"ready", "visiting"}:
                    return True
                continue
            seen_modules.add(url)
            resource(url, "module")
            entry = manifest["resources"].get(url, {})
            if entry.get("status") != "ok":
                continue
            entry["module_status"] = "visiting"
            try:
                inspected = module_tool("--module-inspect", capture / entry["path"], url)
                dependencies = [module_graph(dep["candidates"], depth + 1)
                                for dep in inspected["dependencies"]]
                if all(dependencies):
                    entry["module_status"] = "ready"
                    store(capture / "capture.json", manifest)
                    return True
            except (ValueError, OSError, subprocess.SubprocessError) as error:
                entry["module_error"] = str(error)
                store(capture / "capture.json", manifest)
                print(f"module capture failed: {url}: {error}", file=sys.stderr)
            entry["module_status"] = "failed"
            store(capture / "capture.json", manifest)
        return False

    for specifier in args.module:
        plan = module_tool("--module-resolve", specifier, args.url)
        module_graph(plan["candidates"])
    if args.module:
        # Include static imports from inline module entries and captured external
        # modules; computed/dynamic roots can be added with another --module.
        for source in report["scripts"]:
            if source["type"].split(";")[0].strip().lower() == "module":
                if source["file"] is None:
                    plan = module_tool("--module-resolve", source["origin"], args.url)
                    module_graph(plan["candidates"])
                else:
                    try:
                        inspected = module_tool("--module-inspect", capture / "inspection" / source["file"], source["origin"])
                        for dep in inspected["dependencies"]:
                            module_graph(dep["candidates"])
                    except (ValueError, OSError, subprocess.SubprocessError) as error:
                        print(f"inline module capture failed: {error}", file=sys.stderr)
    print(f"Capture ready: {capture}; replay is offline. No JavaScript ran during capture.")


def replay_arguments(args) -> list[str]:
    result = ["--width", str(args.width), "--height", str(args.height), "--turn-ms", str(args.turn_ms)]
    for name in ("no_js", "no_svg", "external", "no_window", "modules"):
        if getattr(args, name):
            result.append("--" + name.replace("_", "-"))
    if args.only_node is not None:
        result += ["--only-node", str(args.only_node)]
    if args.prelude:
        result += ["--prelude", str(args.prelude.resolve())]
    for replacement in args.replace:
        node, filename = replacement.split("=", 1)
        int(node)
        result += ["--replace", node + "=" + str(Path(filename).resolve())]
    return result


def summary(report: dict) -> None:
    for source in report["scripts"]:
        detail = source.get("error") or source.get("reason") or ""
        label = detail.splitlines()[0] if detail else source['origin']
        if len(label) > 180:
            label = label[:180] + "… (full text in report.json)"
        print(plain(f"  {source['index']:3} nodes={source['nodes']} {source['status']}: {label}"))
    for experiment in report["experiments"]:
        print("  experiment:", experiment)
    for event in report.get("resources", []):
        if event["status"] in {"failed", "fallback", "import-failed", "configuration-failed"}:
            print(plain(f"  resource {event['status']}: {event['url']} {event['detail']}"))
    for error in report.get("job_errors", []):
        print(plain("  JavaScript job failed: " + error.splitlines()[0]))


def plain(text: str) -> str:
    return "".join(character if character.isprintable() or character == "\t" else f"\\u{ord(character):04x}"
                   for character in text)


def watched(args, out: Path) -> tuple:
    paths = [args.directory / "page.html", args.directory / "capture.json"]
    if (args.directory / "resources").exists():
        paths.extend((args.directory / "resources").rglob("*"))
    paths.extend(HULL.joinpath("src").glob("*.rs"))
    paths.extend(HULL.joinpath("src").glob("*.js"))
    paths.extend(HULL.joinpath("rjqry/src").glob("*.rs"))
    paths.extend([HULL / "rjqry/Cargo.toml", HERE / "Cargo.toml"])
    paths.extend(HULL.joinpath("crates/quickjs-engine/src").glob("*"))
    paths.extend([HULL / "crates/quickjs-engine/build.rs", HULL / "crates/quickjs-engine/vendor/quickjs/quickjs.h"])
    if args.prelude:
        paths.append(args.prelude)
    paths.extend(Path(value.split("=", 1)[1]) for value in args.replace)
    result = []
    for path in paths:
        if path.is_file() and not path.resolve().is_relative_to(out.resolve()):
            stat = path.stat()
            result.append((str(path.resolve()), stat.st_mtime_ns, stat.st_size))
    return tuple(sorted(result))


def replay(args, executable: Path) -> None:
    out = args.out or args.directory / "run"
    arguments = replay_arguments(args)
    def once():
        summary(execute(executable, args.directory, out, arguments))
    once()
    if args.command != "watch":
        return
    previous = watched(args, out)
    print("Watching captured sources, replacements and Hull source; Ctrl-C stops.", flush=True)
    while True:
        time.sleep(0.5)
        current = watched(args, out)
        if current == previous:
            continue
        previous = current
        try:
            executable = binary()
            once()
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            print(f"replay failed: {error}", file=sys.stderr)


def show(args, executable: Path) -> None:
    out = args.out or args.directory / "inspection"
    report = execute(executable, args.directory, out, ["--inspect"])
    sources = [source for source in report["scripts"] if args.node in source["nodes"]]
    if not sources:
        raise ValueError(f"node {args.node} is absent in this capture")
    source = sources[0]
    print(f"nodes={source['nodes']} type={source['type']} origin={source['origin']}")
    if source["file"] is None:
        raise ValueError("external source has no inline text; capture it with --scripts and use the resource path in capture.json")
    lines = (out / source["file"]).read_text(errors="replace").splitlines()
    start, end = max(1, args.line - args.context), min(len(lines), args.line + args.context)
    for number in range(start, end + 1):
        print(plain(f"{'>' if number == args.line else ' '} {number:4}: {lines[number - 1]}"))


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    capture = commands.add_parser("capture", help="fetch HTML/CSS once, or import a saved HTML file; never execute scripts")
    capture.add_argument("url")
    capture.add_argument("directory", type=Path)
    capture.add_argument("--html", type=Path, help="import exact saved HTML instead of fetching it")
    capture.add_argument("--scripts", action="store_true", help="also save external JS; does not enable execution")
    capture.add_argument("--module", action="append", default=[], metavar="SPECIFIER", help="capture an ESM entry and static dependency graph; repeat for dynamic roots")
    capture.add_argument("--no-css", action="store_true", help="do not fetch stylesheet references")
    capture.add_argument("--refresh", action="store_true")
    for name in ("replay", "watch"):
        replay = commands.add_parser(name, help="offline production actor and optional SVG pass")
        replay.add_argument("directory", type=Path)
        replay.add_argument("--out", type=Path)
        replay.add_argument("--no-js", action="store_true", help="static baseline")
        replay.add_argument("--no-svg", action="store_true", help="script diagnostics only")
        replay.add_argument("--external", action="store_true", help="legacy experiment: admit captured external bodies without graph loading")
        replay.add_argument("--no-window", action="store_true", help="experiment: omit window globals")
        replay.add_argument("--modules", action="store_true", help="enable production source loading from captured resources only")
        replay.add_argument("--only-node", type=int)
        replay.add_argument("--replace", action="append", default=[], metavar="NODE=FILE")
        replay.add_argument("--prelude", type=Path, help="experiment: local JS source before page scripts")
        replay.add_argument("--turn-ms", type=int, default=20, help="20 matches Hull's idle delay; 0 isolates fast headless scheduling")
        replay.add_argument("--width", type=int, default=1280)
        replay.add_argument("--height", type=int, default=720)
    show = commands.add_parser("show", help="print the exact extracted source around a reported line")
    show.add_argument("directory", type=Path)
    show.add_argument("--node", type=int, required=True)
    show.add_argument("--line", type=int, default=1)
    show.add_argument("--context", type=int, default=6)
    show.add_argument("--out", type=Path)
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        executable = binary()
        if args.command == "capture":
            capture_page(args, executable)
        elif args.command == "show":
            show(args, executable)
        else:
            replay(args, executable)
    except KeyboardInterrupt:
        return 130
    except (OSError, ValueError, subprocess.SubprocessError, urllib.error.URLError) as error:
        print(f"hull-sandbox: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
