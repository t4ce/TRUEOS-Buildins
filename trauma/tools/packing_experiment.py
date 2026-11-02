#!/usr/bin/env python3
"""Compare ELF layout variants without changing or publishing the normal app.

Run after `cargo bp trauma`. Candidates and a JSON receipt are written below
`target/layout-experiment`. Validation checks file contents and relocations;
it does not execute the TRUEOS application.
"""
import collections
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
SDK = ROOT.parent / "TRUEOS-Blueprints"
GROUPS = (
    ".text", ".rodata", ".data.rel.ro", ".data", ".bss", ".tdata", ".tbss",
    ".eh_frame", ".gcc_except_table", ".init_array", ".fini_array",
)
RELOCATION_WIDTHS = {0: 0, 1: 8, 2: 4, 4: 4, 9: 4, 11: 4, 41: 4, 42: 4}
GOT_RELOCATIONS = {9, 41, 42}


@dataclass
class Section:
    name: str
    kind: int
    flags: int
    offset: int
    size: int
    link: int
    info: int
    alignment: int
    entry_size: int


@dataclass
class Symbol:
    name: str
    info: int
    visibility: int
    section: int
    value: int
    size: int


def cstring(data, offset):
    return data[offset:data.index(b"\0", offset)].decode()


class Elf:
    def __init__(self, path):
        self.data = path.read_bytes()
        header = struct.unpack_from("<16sHHIQQQIHHHHHH", self.data)
        assert header[0][:6] == b"\x7fELF\x02\x01" and header[1:3] == (1, 62)
        assert header[11] == 64 and header[12] > 0
        raw = [struct.unpack_from("<IIQQQQIIQQ", self.data, header[6] + i * 64)
               for i in range(header[12])]
        names = raw[header[13]]
        strings = self.data[names[4]:names[4] + names[5]]
        self.sections = [Section(cstring(strings, s[0]), s[1], s[2], *s[4:]) for s in raw]
        self.symbols = []
        self.relocations = []
        for section in self.sections:
            if section.kind == 2:
                assert not self.symbols and section.entry_size == 24
                names = self.sections[section.link]
                strings = self.data[names.offset:names.offset + names.size]
                for offset in range(section.offset, section.offset + section.size, 24):
                    symbol = struct.unpack_from("<IBBHQQ", self.data, offset)
                    self.symbols.append(Symbol(cstring(strings, symbol[0]), *symbol[1:]))
            elif section.kind == 4:
                assert section.entry_size == 24
                for offset in range(section.offset, section.offset + section.size, 24):
                    place, info, addend = struct.unpack_from("<QQq", self.data, offset)
                    self.relocations.append((section.info, place, info & 0xffffffff, info >> 32, addend))
            else:
                assert section.kind != 9, "REL relocations unsupported by the platform loader"
        assert self.symbols

    def contents(self, section):
        return self.data[section.offset:section.offset + section.size]

    def entry(self):
        entry = next(s for s in self.symbols if s.name == "_start")
        section = self.sections[entry.section]
        assert section.flags & 4 and entry.value < section.size
        assert entry.section < 2**32 and entry.value < 2**32
        return entry

    def imports(self):
        return {(s.name, s.info, s.visibility) for s in self.symbols if s.section == 0 and s.name}

    def globals(self):
        return {(s.name, s.info, s.visibility, s.size) for s in self.symbols
                if s.section != 0 and s.info >> 4 in (1, 2)}

    def stats(self):
        return {
            "elf_bytes": len(self.data),
            "sections": len(self.sections),
            "executable_section_bytes": sum(s.size for s in self.sections if s.flags & 4),
            "allocated_section_bytes": sum(s.size for s in self.sections if s.flags & 2),
            "symbols": len(self.symbols),
            "local_symbols": sum(s.info >> 4 == 0 for s in self.symbols),
            "imports": len(self.imports()),
            "global_definitions": len(self.globals()),
            "relocation_counts": dict(collections.Counter(r[2] for r in self.relocations)),
            "entry_hint": (self.entry().section << 32) | self.entry().value,
        }


def section_mapping(before, after, coalesced):
    """Locate every original allocated section inside the new ELF."""
    if not coalesced:
        assert len(before.sections) == len(after.sections)
        mapping = {i: (i, 0) for i, s in enumerate(before.sections) if s.flags & 2}
    else:
        mapping = {}
        for group in GROUPS:
            indices = [i for i, s in enumerate(before.sections) if s.name == group]
            indices += [i for i, s in enumerate(before.sections) if s.name.startswith(group + ".")]
            indices = [i for i in indices if before.sections[i].flags & 2 and i not in mapping]
            if not indices:
                continue
            output_index = next(i for i, s in enumerate(after.sections) if s.name == group)
            cursor = 0
            for i in indices:
                section = before.sections[i]
                alignment = max(1, section.alignment)
                cursor = (cursor + alignment - 1) // alignment * alignment
                mapping[i] = output_index, cursor
                cursor += section.size
            assert cursor == after.sections[output_index].size, group
    assert set(mapping) == {i for i, s in enumerate(before.sections) if s.flags & 2}
    assert {i for i, s in enumerate(after.sections) if s.flags & 2} == {
        index for index, _ in mapping.values()
    }
    checked = 0
    for i, (new_index, offset) in mapping.items():
        old, new = before.sections[i], after.sections[new_index]
        assert old.kind == new.kind
        assert old.flags & 7 == new.flags & 7
        assert new.alignment % max(1, old.alignment) == 0
        assert offset + old.size <= new.size
        if old.kind != 8:
            assert before.contents(old) == after.contents(new)[offset:offset + old.size], old.name
            checked += old.size
    return mapping, checked


def canonical_symbol(symbol, addend, mapping=None):
    index, value = symbol.section, symbol.value
    if index == 0:
        return "undefined", symbol.name, symbol.info >> 4, value + addend
    if index == 0xfff1:
        return "absolute", value + addend
    if mapping is not None:
        index, offset = mapping[index]
        value += offset
    return "section", index, value + addend


def canonical_relocations(elf, mapping=None):
    result = collections.Counter()
    for index, place, kind, symbol, addend in elf.relocations:
        section = elf.sections[index]
        if not section.flags & 2:
            continue
        assert kind in RELOCATION_WIDTHS
        assert place + RELOCATION_WIDTHS[kind] <= section.size
        target = elf.symbols[symbol]
        # A GOT reference addresses a slot holding S, so its addend cannot be
        # folded into S as with ordinary S+A relocations.
        if kind in GOT_RELOCATIONS:
            destination = canonical_symbol(target, 0, mapping), addend
        else:
            destination = canonical_symbol(target, addend, mapping)
        if mapping is not None:
            index, offset = mapping[index]
            place += offset
        result[index, place, kind, destination] += 1
    return result


def validate(before, after, coalesced):
    assert before.imports() == after.imports()
    assert before.globals() == after.globals()
    mapping, checked = section_mapping(before, after, coalesced)
    expected_globals = {
        (s.name, s.info, s.visibility, s.size, canonical_symbol(s, 0, mapping))
        for s in before.symbols if s.section != 0 and s.info >> 4 in (1, 2)
    }
    actual_globals = {
        (s.name, s.info, s.visibility, s.size, canonical_symbol(s, 0))
        for s in after.symbols if s.section != 0 and s.info >> 4 in (1, 2)
    }
    assert expected_globals == actual_globals, "global symbol destinations changed"
    expected = canonical_relocations(before, mapping)
    actual = canonical_relocations(after)
    assert expected == actual, "relocation destinations changed"
    old_entry, new_entry = before.entry(), after.entry()
    index, offset = mapping[old_entry.section]
    assert (new_entry.section, new_entry.value) == (index, offset + old_entry.value)
    return {"original_allocated_bytes_verified": checked, "relocations_verified": sum(actual.values())}


def pack(elf, output, header):
    # Match the production deterministic USTAR header, padding, and LZ4 flags.
    tar_header = bytearray(512)
    tar_header[:10] = b"module.elf"
    for lo, hi, value in [(100, 108, 0o644), (108, 116, 0), (116, 124, 0),
                          (124, 136, len(elf.data)), (136, 148, 0)]:
        tar_header[lo:hi - 1] = f"{value:0{hi-lo-1}o}".encode()
    tar_header[148:156] = b" " * 8
    tar_header[156] = ord("0")
    tar_header[257:263], tar_header[263:265] = b"ustar\0", b"00"
    tar_header[148:154] = f"{sum(tar_header):06o}".encode()
    tar_header[154] = 0
    tar = tar_header + elf.data
    tar += bytes((-len(tar)) % 512 + 1024)
    tar_path, archive = output.with_suffix(".tar"), output.with_suffix(".tar.lz4")
    tar_path.write_bytes(tar)
    subprocess.run(["lz4", "-q", "-12", "-BI", "--content-size", "-f", str(tar_path), str(archive)], check=True)
    compressed = archive.read_bytes()
    entry = elf.entry()
    packed = struct.pack("<4sHHQII", header[0], header[1], header[2],
                         (entry.section << 32) | entry.value, len(compressed), len(elf.data)) + compressed
    output.write_bytes(packed)
    # Verify lossless archive roundtrip and the original allocated ELF bytes.
    decoded = subprocess.check_output(["lz4", "-q", "-d", "-c", str(archive)])
    assert decoded == tar and decoded[512:512 + len(elf.data)] == elf.data
    return len(packed)


def main():
    output = ROOT / "target/layout-experiment"
    output.mkdir(parents=True, exist_ok=True)
    source = ROOT / "target/trueos-blueprint/work/trauma/module.stripped.o"
    blueprint = SDK / "dist/trauma.bp"
    shutil.copyfile(source, output / "baseline.o")
    shutil.copyfile(blueprint, output / "baseline.bp")
    original_bp = (output / "baseline.bp").read_bytes()
    header = struct.unpack_from("<4sHHQII", original_bp)
    assert header[:3] == (b"TRBP", 1, 3), "expected ordinary tar/LZ4 Blueprint"
    baseline = Elf(output / "baseline.o")
    assert header[5] == len(baseline.data) and header[4] + 24 == len(original_bp)
    assert header[3] == baseline.stats()["entry_hint"]
    script = re.search(r'const REL_COALESCE_LINKER_SCRIPT: &str = r#"(.*?)"#;',
                       (SDK / "src/main.rs").read_text(), re.S)
    assert script, "production coalescing script not found"
    linker_script = output / "coalesce.ld"
    linker_script.write_text(script.group(1))
    toolchain = tomllib.loads((SDK / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    sysroot = Path(subprocess.check_output(["rustc", f"+{toolchain}", "--print", "sysroot"], text=True).strip())
    linker = sysroot / "lib/rustlib/x86_64-unknown-linux-gnu/bin/gcc-ld/ld.lld"
    receipt = {"baseline_sha256": hashlib.sha256(original_bp).hexdigest(), "variants": {}}
    for name, coalesce, discard in [("baseline", False, False), ("locals", False, True),
                                      ("coalesce", True, False), ("compact", True, True)]:
        elf_path = output / f"{name}.o"
        if coalesce:
            command = [str(linker), "-r", "--no-relax", "-T", str(linker_script)]
            if discard:
                command.append("--discard-all")
            subprocess.run(command + ["-o", str(elf_path), str(output / "baseline.o")], check=True)
        elif discard:
            subprocess.run(["objcopy", "--discard-all", str(output / "baseline.o"), str(elf_path)], check=True)
        elf = Elf(elf_path)
        checks = validate(baseline, elf, coalesce)
        candidate = output / f"trauma.{name}.bp"
        packed_bytes = pack(elf, candidate, header)
        if name == "baseline":
            assert candidate.read_bytes() == original_bp, "baseline repack differs from production"
        stats = elf.stats() | checks | {"bp_bytes": packed_bytes, "sha256": hashlib.sha256(candidate.read_bytes()).hexdigest()}
        receipt["variants"][name] = stats
        print(f"{name:10} ELF {len(elf.data):>9,} B | BP {packed_bytes:>9,} B | sections {len(elf.sections):>6,}")
    (output / "results.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"Validated candidates in {output}; normal pack and publish settings unchanged.")


if __name__ == "__main__":
    main()
