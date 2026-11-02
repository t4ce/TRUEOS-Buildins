# Kernel-owned builtin builds

`cargo rig` and `cargo rigreal` in the kernel checkout both call
`tools/trueos-boot/prepare.sh`, which invokes this repository's `tools/build.py`.
`rig` selects `dev`; `rigreal` selects `release`. No command changes are needed.
The registry is `buildins.json`. Trauma is a normal source directory here and
replaces the former Hyper downloader application; the Hyper library stays.

Release Cargo compilation uses `release-profile.toml`: fat LTO, one codegen
unit, and the application's own optimization level. Packaging strips debug and
LLVM compiler sections, then coalesces live code/data sections and discards
unneeded local symbols. It disables linker relaxation and clears merge hints
on a temporary copy to prevent another string/constant pooling pass. Cargo's
original cached object keeps its debug information.

Before emitting a compact package, `elf_layout.py` compares all original
allocated bytes, flags/alignment, imports, global symbol destinations,
relocations (including GOT references), and `_start`. Failure stops packaging.
The kernel's CABI/import checks still run on the final object. Each release
app has `objects/NAME/layout.json` with before/after sizes and verification
counts. The package cache fingerprints the layout script and validator.
Development skips compaction. Both profiles keep the symbol table and
relocations required by the kernel; neither uses `--strip-all`.

Prepare a complete release set without publishing or resetting hardware:

```sh
python3 tools/build.py --kernel ../TRUEOS --profile release --output ../TRUEOS/tgt/buildins/release
python3 tools/build.py --kernel ../TRUEOS --profile release --output ../TRUEOS/tgt/buildins/release --check
```

For development, select `--profile dev` and `--output ../TRUEOS/tgt/buildins/dev`.
`--app trauma` builds only one app for diagnostics and does not mark a complete
AppDB set ready. Full builds remove retired `.bp` packages, regenerate AppDB,
and write a complete-set readiness marker only after all apps pass.

Run the driver and actual-link layout regression checks:

```sh
python3 -m unittest discover -s tools -p test_build.py
python3 -m unittest discover -s tools -p test_elf_layout.py
```

After preparing a complete set, test the production kernel's decompression and
AppDB loading code without booting or resetting:

```sh
python3 ../TRUEOS/tools/testpy/test_buildins_payloads.py --builtins ../TRUEOS/tgt/buildins/release
```

These checks do not execute the builtin applications in VMX.
