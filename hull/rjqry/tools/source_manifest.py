#!/usr/bin/env python3
"""Pin the supplied jQuery archive by content; no unverified commit is inferred."""
import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UPSTREAM = ROOT / "jquery-main"
DESTINATION = ROOT / "UPSTREAM.json"


def manifest():
    if (ROOT / "LICENSE").read_bytes() != (UPSTREAM / "LICENSE.txt").read_bytes():
        raise SystemExit("Native license differs from supplied upstream MIT notice")
    paths = [UPSTREAM / "LICENSE.txt", UPSTREAM / "package.json"]
    paths += sorted((UPSTREAM / "src").rglob("*.js"))
    hashes = {str(path.relative_to(UPSTREAM)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}
    content_hash = hashlib.sha256("".join(f"{name}\0{digest}\n" for name, digest in hashes.items()).encode()).hexdigest()
    return {
        "source": "User-supplied jquery-main archive",
        "repository": "https://github.com/jquery/jquery",
        "package_version": json.loads((UPSTREAM / "package.json").read_text())["version"],
        "revision": None,
        "revision_note": "No commit identity accompanies this archive; version is its package label, not an inferred release revision.",
        "fingerprint_sha256": content_hash,
        "files_sha256": hashes,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="record an intentionally selected source baseline")
    arguments = parser.parse_args()
    content = json.dumps(manifest(), indent=2) + "\n"
    if arguments.write:
        DESTINATION.write_text(content)
    elif not DESTINATION.exists() or DESTINATION.read_text() != content:
        raise SystemExit("Upstream source fingerprint changed; inspect before recording a new baseline")
    print(f"Verified {len(manifest()['files_sha256'])} source/license/package files")


if __name__ == "__main__":
    main()
