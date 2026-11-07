#!/usr/bin/env python3
"""Check Edit's native attached-terminal declarations against the kernel contract."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT.parent / "TRUEOS"


def signature(source, name):
    """The selected terminal declarations contain only scalar/pointer arguments."""
    found = re.search(r"\bfn\s+" + re.escape(name) + r"\s*\(", source)
    if found is None:
        raise AssertionError(f"missing native ABI declaration {name}")
    opening = source.index("(", found.start())
    closing = source.index(")", opening)
    arguments = source[opening + 1:closing]
    types = [item.split(":", 1)[1].strip() for item in arguments.split(",") if item.strip()]
    tail = source[closing + 1:source.index(";", closing)].strip()
    result = tail[2:].strip() if tail.startswith("->") else "()"
    return tuple(re.sub(r"\s+", "", item) for item in types), re.sub(r"\s+", "", result)


class EditorAbi(unittest.TestCase):
    def test_native_terminal_declarations_match_kernel(self):
        contract = (KERNEL / "crates/trueos-v/src/bp_abi.rs").read_text()
        declared = set()
        for filename in ("trueos_terminal.rs", "trueos_output.rs"):
            source = (ROOT / "edit/crates/edit/src/sys" / filename).read_text()
            names = set(re.findall(r"\bfn\s+(trueos_cabi_\w+)\s*\(", source))
            for name in names:
                with self.subTest(symbol=name):
                    self.assertEqual(signature(source, name), signature(contract, name))
                declared.add(name)
        self.assertGreaterEqual(len(declared), 6)


if __name__ == "__main__":
    unittest.main()
