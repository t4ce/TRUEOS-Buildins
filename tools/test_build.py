#!/usr/bin/env python3
"""Exercise source staging and Cargo-owned object packaging without compiling."""
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

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


class ToolEnvironment(unittest.TestCase):
    def test_dev_build_stays_unoptimized_and_does_not_inherit_kernel_linking(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            installed = root / "installed"
            output = root / "output"
            inherited = {
                "CARGO_PROFILE_DEV_OPT_LEVEL": "3",
                "CARGO_PROFILE_DEV_LTO": "fat",
                "CARGO_PROFILE_DEV_CODEGEN_UNITS": "1",
                "RUSTFLAGS": "-C link-arg=-Tkernel.ld",
                "CARGO_ENCODED_RUSTFLAGS": "-C\x1flink-arg=-Tkernel.ld",
                "CARGO_TARGET_TRUEOS_APP_RUSTFLAGS": "-C link-arg=-Tkernel.ld",
                "CARGO_BUILD_TARGET": "kernel.json",
            }
            with patch.dict(os.environ, inherited, clear=True), \
                 patch.object(build, "capture", side_effect=[str(installed), "commit-hash: " + build.COMMIT]), \
                 patch.object(build, "private_sysroot") as private:
                cargo, binaries, env = build.tool_environment(output, root / "kernel", False)
            self.assertEqual(cargo, installed / "bin/cargo")
            self.assertEqual(binaries, installed / "lib/rustlib/x86_64-unknown-linux-gnu/bin")
            self.assertEqual(env["CARGO_PROFILE_DEV_OPT_LEVEL"], "0")
            self.assertEqual(env["CARGO_PROFILE_DEV_LTO"], "false")
            self.assertEqual(env["CARGO_PROFILE_DEV_CODEGEN_UNITS"], "256")
            self.assertEqual(env["CARGO_PROFILE_DEV_SPLIT_DEBUGINFO"], "off")
            self.assertEqual(env["CARGO_PROFILE_DEV_STRIP"], "none")
            self.assertNotIn("RUSTFLAGS", env)
            self.assertNotIn("CARGO_BUILD_TARGET", env)
            self.assertNotIn("CARGO_TARGET_TRUEOS_APP_RUSTFLAGS", env)
            self.assertNotIn("kernel.ld", env["CARGO_ENCODED_RUSTFLAGS"])
            private.assert_not_called()
            for name in ("libc.a", "libgcc_s.a"):
                self.assertEqual((output / "system-libraries" / name).read_bytes(), b"!<arch>\n")
            self.assertFalse(installed.exists(), "tool preparation must not modify the installed toolchain")


class CargoObjects(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.output = self.root / "output"
        self.output.mkdir()
        self.manifest = self.root / "paper/Cargo.toml"
        self.manifest.parent.mkdir()
        self.manifest.write_text('[package]\nname="paper"\nversion="0.1.0"\n')
        self.source = self.manifest.parent / "src/main.rs"
        self.source.parent.mkdir()
        self.source.write_text("fn main() {}\n")
        (self.root / "tools").mkdir()
        (self.root / "tools/trueos-object.json").write_text('{"exe-suffix":".o"}\n')
        self.current = self.output / "cargo/trueos-object/debug/paper.o"
        self.current.parent.mkdir(parents=True)
        self.current.write_bytes(b"current Cargo object")
        self.stale = self.current.parent / "deps/paper-newer-stale.o"
        self.stale.parent.mkdir()
        self.stale.write_bytes(b"stale object must never be selected")
        self.artifacts = [self.artifact([self.current])]
        self.status = 0
        self.cargo = self.root / "toolchain/bin/cargo"
        self.binaries = self.root / "toolchain/bin"
        self.hint = (3 << 32) | 16
        self.doc = {}
        for name, value in (
            ("ROOT", self.root),
            ("settings", lambda _: (self.manifest, self.source, False, [], self.doc)),
            ("stage_app", lambda *_: self.manifest),
            ("checkpoint", lambda *_: None),
            ("entry_hint", lambda _: self.hint),
        ):
            replacement = patch.object(build, name, value)
            replacement.start()
            self.addCleanup(replacement.stop)
        replacement = patch.object(build.subprocess, "Popen", side_effect=self.cargo_process)
        self.popen = replacement.start()
        self.addCleanup(replacement.stop)
        replacement = patch.object(build, "run", side_effect=self.strip)
        self.run = replacement.start()
        self.addCleanup(replacement.stop)
        replacement = patch.object(build, "compress_module", side_effect=lambda data: b"packed:" + data)
        self.compress = replacement.start()
        self.addCleanup(replacement.stop)

    def artifact(self, paths, *, name="paper", kind="bin", fresh=False):
        return {"reason": "compiler-artifact", "manifest_path": str(self.manifest),
                "target": {"name": name, "kind": [kind]},
                "filenames": list(map(str, paths)),
                "executable": str(paths[0]) if paths and kind == "bin" else None,
                "fresh": fresh}

    def cargo_process(self, *args, **kwargs):
        messages = [*self.artifacts, {"reason": "build-finished", "success": self.status == 0}]
        return SimpleNamespace(stdout=io.StringIO("".join(json.dumps(item) + "\n" for item in messages)),
                               wait=lambda: self.status)

    def strip(self, command, **kwargs):
        # The driver must package Cargo's combined object, not invoke another
        # linker or rediscover old objects and dependency archives.
        command = list(map(str, command))
        self.assertEqual(Path(command[0]).name, "rust-objcopy")
        self.assertIn("--strip-debug", command)
        self.assertEqual(Path(command[-2]), self.current)
        Path(command[-1]).write_bytes(b"stripped:" + self.current.read_bytes())

    def package(self):
        return build.build_app("paper", self.output, "dev", self.cargo, self.binaries,
                               {}, {})

    def test_packages_only_the_selected_cargo_binary_object(self):
        self.artifacts.insert(0, self.artifact([self.stale], kind="lib"))
        self.artifacts.insert(0, self.artifact([self.stale], name="another-app"))
        other_package = self.artifact([self.stale])
        other_package["manifest_path"] = str(self.root / "dependency/Cargo.toml")
        self.artifacts.insert(0, other_package)
        # Cargo's executable field is authoritative even when filenames also
        # names another object. Filesystem ordering must never select it.
        self.artifacts[-1]["filenames"].append(str(self.stale))
        package = self.package()
        self.assertEqual(package.read_bytes()[:4], b"TRBP")
        self.assertTrue(package.read_bytes().endswith(b"packed:stripped:" + self.current.read_bytes()))
        command = self.popen.call_args.args[0]
        self.assertEqual(command[1], "build")
        self.assertIn(str(self.root / "tools/trueos-object.json"), command)
        self.assertNotIn("rustc", command)
        self.assertNotIn("-Zno-link", command)
        self.assertNotIn("--emit=obj", command)
        self.run.assert_called_once()

    def test_fresh_cargo_object_skips_repeated_stripping_and_packaging(self):
        package = self.package()
        previous = (package.read_bytes(), package.stat().st_mtime_ns)
        self.artifacts = [self.artifact([self.current], fresh=True)]
        self.assertEqual(self.package(), package)
        self.assertEqual(self.popen.call_count, 2, "Cargo must decide dependency freshness on every build")
        self.assertEqual(self.run.call_count, 1)
        self.assertEqual(self.compress.call_count, 1)
        self.assertEqual((package.read_bytes(), package.stat().st_mtime_ns), previous)

    def test_changed_cargo_object_repackages_even_when_the_path_is_unchanged(self):
        package = self.package()
        previous = package.read_bytes()
        self.current.write_bytes(b"updated Cargo object with changed code")
        self.package()
        self.assertEqual(self.run.call_count, 2)
        self.assertEqual(self.compress.call_count, 2)
        self.assertNotEqual(package.read_bytes(), previous)

    def test_unchanged_object_bytes_skip_packaging_after_its_mtime_changes(self):
        self.package()
        metadata = self.current.stat()
        os.utime(self.current, ns=(metadata.st_atime_ns, metadata.st_mtime_ns + 1_000_000_000))
        self.package()
        self.assertEqual(self.popen.call_count, 2)
        self.assertEqual(self.run.call_count, 1)
        self.assertEqual(self.compress.call_count, 1)

    def test_missing_selected_object_never_uses_a_stale_glob_match(self):
        self.artifacts = [self.artifact([self.current.with_name("paper-missing.o")])]
        with self.assertRaises(ValueError):
            self.package()
        self.run.assert_not_called()
        self.compress.assert_not_called()

    def test_missing_binary_artifact_never_uses_a_cached_object(self):
        self.artifacts = [self.artifact([self.stale], kind="lib")]
        with self.assertRaises(ValueError):
            self.package()
        self.run.assert_not_called()

    def test_conflicting_root_executable_artifacts_are_rejected(self):
        self.artifacts.append(self.artifact([self.stale]))
        with self.assertRaises(ValueError):
            self.package()
        self.run.assert_not_called()

    def test_nonobject_executable_is_rejected(self):
        executable = self.current.with_suffix("")
        executable.write_bytes(b"not a combined object")
        self.artifacts = [self.artifact([executable])]
        with self.assertRaises(ValueError):
            self.package()
        self.run.assert_not_called()

    def test_missing_executable_never_uses_a_filename_or_glob_match(self):
        self.artifacts[0]["executable"] = None
        with self.assertRaises(ValueError):
            self.package()
        self.run.assert_not_called()

    def test_corrupted_package_is_rebuilt_from_the_cargo_object(self):
        package = self.package()
        expected = package.read_bytes()
        package.write_bytes(b"damaged package")
        self.package()
        self.assertEqual(self.run.call_count, 2)
        self.assertEqual(self.compress.call_count, 2)
        self.assertEqual(package.read_bytes(), expected)

    def test_corrupted_stripped_object_is_rebuilt_from_the_cargo_object(self):
        self.package()
        stripped = Path(self.run.call_args.args[0][-1])
        stripped.write_bytes(b"damaged stripped object")
        self.package()
        self.assertEqual(self.run.call_count, 2)
        self.assertEqual(self.compress.call_count, 2)
        self.assertEqual(stripped.read_bytes(), b"stripped:" + self.current.read_bytes())

    def test_legacy_or_malformed_stamp_is_replaced_then_fresh_builds_skip_packaging(self):
        package = self.package()
        expected = package.read_bytes()
        stamp = self.output / "objects/paper/inputs.json"
        self.artifacts = [self.artifact([self.current], fresh=True)]
        legacy = json.dumps([[str(self.current), 20, 123456]]) + json.dumps(0)
        for contents in (legacy, '{"inputs":', "[]"):
            with self.subTest(stamp=contents):
                stamp.write_text(contents)
                cargo_calls, strip_calls, compress_calls = (
                    self.popen.call_count, self.run.call_count, self.compress.call_count)
                self.assertEqual(self.package(), package)
                self.assertEqual(self.run.call_count, strip_calls + 1)
                self.assertEqual(self.compress.call_count, compress_calls + 1)
                self.assertEqual(package.read_bytes(), expected)
                self.assertIsInstance(json.loads(stamp.read_text()), dict)
                self.assertEqual(self.package(), package)
                self.assertEqual(self.popen.call_count, cargo_calls + 2)
                self.assertEqual(self.run.call_count, strip_calls + 1)
                self.assertEqual(self.compress.call_count, compress_calls + 1)

    def test_changed_package_flags_repackages_an_unchanged_cargo_object(self):
        package = self.package()
        previous = package.read_bytes()
        self.doc["package"] = {"metadata": {"trueos-blueprint": {"replicatable": True}}}
        self.package()
        self.assertEqual(self.run.call_count, 2)
        self.assertNotEqual(package.read_bytes(), previous)

    def test_failed_cargo_does_not_package_a_preexisting_object(self):
        self.status = 1
        with self.assertRaises(ValueError):
            self.package()
        self.run.assert_not_called()
        self.compress.assert_not_called()


if __name__ == "__main__":
    unittest.main()
