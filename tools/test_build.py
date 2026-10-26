#!/usr/bin/env python3
"""Regression checks for source staging and authoritative link input selection."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("buildins", Path(__file__).with_name("build.py"))
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class BuildInputs(unittest.TestCase):
    def test_staging_keeps_unchanged_mtimes_and_tracks_edits_and_deletions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / "paper"
            (app / "src").mkdir(parents=True)
            manifest = app / "Cargo.toml"
            manifest.write_text('[package]\nname="paper"\nversion="0.1.0"\n[dependencies]\ntrueos={workspace=true}\n')
            source = app / "src/main.rs"
            # A comment mentioning no_main must never suppress the real entry shim.
            source.write_text('// consider #![no_main]\nfn main() {}\n')
            removed = app / "src/old.rs"
            removed.write_text("// old\n")
            original_root, build.ROOT = build.ROOT, root
            self.addCleanup(setattr, build, "ROOT", original_root)
            output = root / "output"
            staged = build.stage_app("paper", output, manifest, source)
            entry = staged.parent / "src/main.rs"
            self.assertTrue(entry.read_text().startswith("#![no_main]\n"))
            self.assertIn('fn _start()', entry.read_text())
            before = (staged.stat().st_mtime_ns, entry.stat().st_mtime_ns)
            build.stage_app("paper", output, manifest, source)
            self.assertEqual(before, (staged.stat().st_mtime_ns, entry.stat().st_mtime_ns))
            source.write_text(source.read_text() + "// changed\n")
            removed.unlink()
            build.stage_app("paper", output, manifest, source)
            self.assertEqual(before[0], staged.stat().st_mtime_ns)
            self.assertIn("// changed", entry.read_text())
            self.assertFalse((entry.parent / "old.rs").exists())

    def test_rlink_selects_only_current_archives_and_all_codegen_objects(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            selected = root / "libselected-123.rlib"
            stale = root / "libstale-456.rlib"
            first = root / "paper-123.first.rcgu.o"
            second = root / "paper-123.second.rcgu.o"
            for path in (selected, stale, first, second):
                path.touch()
            rlink = root / "paper-123.rlink"
            # Printable bincode length bytes can be adjacent to the path.
            rlink.write_bytes(b"~" + str(first).encode() + b"\0" + str(second).encode()
                              + b"\0" + str(selected).encode() + b"\0")
            objects, archives = build.selected_inputs(rlink)
            self.assertEqual(objects, [first, second])
            self.assertEqual(archives, [selected])


if __name__ == "__main__":
    unittest.main()
