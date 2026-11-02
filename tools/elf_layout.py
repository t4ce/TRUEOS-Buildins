"""Validate a packaging-only ELF layout change against Cargo's original object.

The second link must preserve allocated bytes, symbol destinations, relocations,
and the entry point. These checks run before any release BP is emitted.
"""
import collections
from dataclasses import dataclass
import struct


def require(condition, message):
    if not condition:
        raise ValueError("ELF layout validation: " + str(message))


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
        require(header[0][:6] == b"\x7fELF\x02\x01" and header[1:3] == (1, 62), "expected an ELF64 little-endian x86-64 relocatable object")
        require(header[11] == 64, "expected ELF64 section headers")
        self.section_table = header[6]
        first = struct.unpack_from("<IIQQQQIIQQ", self.data, self.section_table)
        count = header[12] or first[5]
        name_index = first[6] if header[13] == 0xffff else header[13]
        require(count > 0 and self.section_table + count * 64 <= len(self.data), "section table is truncated")
        raw = [struct.unpack_from("<IIQQQQIIQQ", self.data, self.section_table + i * 64)
               for i in range(count)]
        names = raw[name_index]
        strings = self.data[names[4]:names[4] + names[5]]
        self.sections = [Section(cstring(strings, s[0]), s[1], s[2], *s[4:]) for s in raw]
        self.symbols = []
        self.relocations = []
        for section_index, section in enumerate(self.sections):
            if section.kind == 2:
                require(not self.symbols and section.entry_size == 24, "expected one ELF64 symbol table")
                names = self.sections[section.link]
                strings = self.data[names.offset:names.offset + names.size]
                extended = next((s for s in self.sections if s.kind == 18 and s.link == section_index), None)
                for number, offset in enumerate(range(section.offset, section.offset + section.size, 24)):
                    name, info, visibility, index, value, size = struct.unpack_from("<IBBHQQ", self.data, offset)
                    if index == 0xffff:
                        require(extended is not None, "extended symbol section index is missing")
                        index = struct.unpack_from("<I", self.data, extended.offset + number * 4)[0]
                    self.symbols.append(Symbol(cstring(strings, name), info, visibility, index, value, size))
            elif section.kind == 4:
                require(section.entry_size == 24, "expected ELF64 RELA records")
                for offset in range(section.offset, section.offset + section.size, 24):
                    place, info, addend = struct.unpack_from("<QQq", self.data, offset)
                    self.relocations.append((section.info, place, info & 0xffffffff, info >> 32, addend))
            else:
                require(section.kind != 9, "REL relocations unsupported by the platform loader")
        require(self.symbols, "symbol table is missing")

    def contents(self, section):
        return self.data[section.offset:section.offset + section.size]

    def entry(self):
        entry = next((s for s in self.symbols if s.name == "_start"), None)
        require(entry is not None and 0 < entry.section < len(self.sections), "defined _start symbol is missing")
        section = self.sections[entry.section]
        require(section.flags & 4 and entry.value < section.size, "entry point is outside executable data")
        require(entry.section < 2**32 and entry.value < 2**32, "entry hint exceeds the Blueprint header format")
        return entry

    def imports(self):
        return {(s.name, s.info, s.visibility) for s in self.symbols if s.section == 0 and s.name}

    def globals(self):
        return {(s.name, s.info, s.visibility, s.size) for s in self.symbols
                if s.section != 0 and s.info >> 4 != 0}

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
        require(len(before.sections) == len(after.sections), "section count changed without coalescing")
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
            require(cursor == after.sections[output_index].size, f"coalesced size changed: {group}")
        # Allocated custom sections remain linker-script orphans. Match them
        # by name rather than dropping assets or weakening the validation.
        pending = {s.name for i, s in enumerate(before.sections) if s.flags & 2 and i not in mapping}
        for name in pending:
            targets = [i for i, s in enumerate(after.sections) if s.name == name and s.flags & 2]
            require(len(targets) == 1, f"ambiguous custom section {name}")
            cursor = 0
            for i, section in enumerate(before.sections):
                if section.flags & 2 and section.name == name and i not in mapping:
                    alignment = max(1, section.alignment)
                    cursor = (cursor + alignment - 1) // alignment * alignment
                    mapping[i] = targets[0], cursor
                    cursor += section.size
            require(cursor == after.sections[targets[0]].size, f"custom section size changed: {name}")
    require(set(mapping) == {i for i, s in enumerate(before.sections) if s.flags & 2}, "allocated sections were lost")
    require({i for i, s in enumerate(after.sections) if s.flags & 2} == {
        index for index, _ in mapping.values()
    }, "unexpected allocated output sections")
    checked = 0
    for i, (new_index, offset) in mapping.items():
        old, new = before.sections[i], after.sections[new_index]
        require(old.kind == new.kind, "allocated section type changed")
        require(old.flags & 7 == new.flags & 7, "allocated section permissions changed")
        require(new.alignment % max(1, old.alignment) == 0, "section alignment was weakened")
        require(offset + old.size <= new.size, "allocated section was truncated")
        if old.kind != 8:
            # Slice only this original fragment, rather than copying the whole
            # coalesced output section for every function or data object.
            start = new.offset + offset
            require(before.contents(old) == after.data[start:start + old.size], old.name)
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
        require(kind in RELOCATION_WIDTHS, f"unsupported loader relocation type {kind}")
        require(place + RELOCATION_WIDTHS[kind] <= section.size, "relocation patch exceeds its section")
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
    require(before.imports() == after.imports(), "undefined imports changed")
    require(before.globals() == after.globals(), "global symbol definitions changed")
    mapping, checked = section_mapping(before, after, coalesced)
    expected_globals = {
        (s.name, s.info, s.visibility, s.size, canonical_symbol(s, 0, mapping))
        for s in before.symbols if s.section != 0 and s.info >> 4 != 0
    }
    actual_globals = {
        (s.name, s.info, s.visibility, s.size, canonical_symbol(s, 0))
        for s in after.symbols if s.section != 0 and s.info >> 4 != 0
    }
    require(expected_globals == actual_globals, "global symbol destinations changed")
    expected = canonical_relocations(before, mapping)
    actual = canonical_relocations(after)
    require(expected == actual, "relocation destinations changed")
    old_entry, new_entry = before.entry(), after.entry()
    index, offset = mapping[old_entry.section]
    require((new_entry.section, new_entry.value) == (index, offset + old_entry.value), "entry point destination changed")
    return {"original_allocated_bytes_verified": checked, "relocations_verified": sum(actual.values())}


def freeze_merge_sections(elf, destination):
    """Keep a second link from re-pooling already-linked strings/constants.

    Clearing optional linker merge hints changes only section headers. Data,
    symbol offsets, and relocations stay identical to Cargo's combined object.
    """
    data = bytearray(elf.data)
    for index, section in enumerate(elf.sections):
        if section.flags & 0x10:  # SHF_MERGE; SHF_STRINGS is its optional companion.
            struct.pack_into("<Q", data, elf.section_table + index * 64 + 8, section.flags & ~0x30)
            struct.pack_into("<Q", data, elf.section_table + index * 64 + 56, 0)
    destination.write_bytes(data)
