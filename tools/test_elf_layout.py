"""Exercise the release layout step using real assembler/linker artifacts."""
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest

import build
from elf_layout import Elf, freeze_merge_sections, validate


@unittest.skipUnless(shutil.which("cc"), "layout fixtures require a host assembler")
class ReleaseLayout(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        sysroot = subprocess.check_output(["rustup", "run", build.TOOLCHAIN, "rustc", "--print", "sysroot"], text=True).strip()
        cls.binaries = Path(sysroot) / "lib/rustlib/x86_64-unknown-linux-gnu/bin"

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        source = self.root / "fixture.S"
        source.write_text('''
.section .text._start,"ax",@progbits
.globl _start
.type _start,@function
_start:
    call imported_function@PLT
    lea local_data(%rip), %rax
    movabs $local_data, %r10
    mov imported_data@GOTPCREL(%rip), %r11
    ret
.size _start, .-_start
.section .rodata.str1.1,"aMS",@progbits,1
.Lmessage:
    .asciz "layout fixture"
.section .data.local_data,"aw",@progbits
local_data:
    .quad .Lmessage
.section .bss.cache,"aw",@nobits
    .balign 32
    .space 64
.section .trueos.fixture,"a",@progbits
    .byte 1, 2, 3, 4
.section .note.GNU-stack,"",@progbits
''')
        self.original = self.root / "original.o"
        self.compact = self.root / "module.stripped.o"
        subprocess.run(["cc", "-c", str(source), "-o", str(self.original)], check=True)
        build.compact_object(self.original, self.compact, self.binaries, {})

    def test_real_relink_preserves_imports_data_got_relocations_and_entry(self):
        before, after = Elf(self.original), Elf(self.compact)
        checks = validate(before, after, True)
        self.assertEqual(checks["relocations_verified"], 5)
        self.assertEqual(before.imports(), after.imports())
        self.assertIn(".trueos.fixture", [s.name for s in after.sections])
        self.assertIn(".bss", [s.name for s in after.sections])
        self.assertEqual(build.entry_hint(self.compact), after.stats()["entry_hint"])
        receipt = json.loads((self.root / "layout.json").read_text())
        self.assertEqual(receipt["relocations_verified"], 5)
        frozen = self.root / "frozen.o"
        freeze_merge_sections(before, frozen)
        self.assertTrue(any(s.flags & 0x10 for s in before.sections))
        self.assertFalse(any(s.flags & 0x10 for s in Elf(frozen).sections))
        validate(before, Elf(frozen), False)

    def test_altered_code_or_relocation_is_rejected(self):
        before, after = Elf(self.original), Elf(self.compact)
        code = next(s for s in after.sections if s.flags & 4)
        tampered = bytearray(after.data)
        tampered[code.offset] ^= 1
        path = self.root / "bad-code.o"
        path.write_bytes(tampered)
        with self.assertRaises(ValueError):
            validate(before, Elf(path), True)
        relocation = next(s for s in after.sections if s.kind == 4)
        tampered = bytearray(after.data)
        addend = struct.unpack_from("<q", tampered, relocation.offset + 16)[0]
        struct.pack_into("<q", tampered, relocation.offset + 16, addend + 1)
        path = self.root / "bad-relocation.o"
        path.write_bytes(tampered)
        with self.assertRaisesRegex(ValueError, "relocation destinations"):
            validate(before, Elf(path), True)


if __name__ == "__main__":
    unittest.main()
