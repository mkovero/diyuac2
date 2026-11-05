#!/usr/bin/env python3
"""Make an Intel hex of the diyuac2 app plus its info sector, for a debug
probe (pyocd load). The bootloader starts the app only if the info sector
(magic, length, CRC-32) matches the image, which a DFU download writes
itself; a probe flash needs this.

  mkimage.py <app.bin> <out.hex> [<boot.bin>]

app.bin:  objcopy -O binary of the app (linked for flash offset 0x40000)
boot.bin: optional, objcopy -O binary of the bootloader (flash offset 0);
          include it to flash everything in one go.
Layout: rust/common/src/layout.rs.
"""

import struct
import sys
import zlib

FLASH_BASE = 0x6000_0000
INFO_OFFSET = 0x3_F000
APP_OFFSET = 0x4_0000
APP_MAX = 0x10_0000
BOOT_SIZE = 0x3_F000
INFO_MAGIC = 0x4941_3244  # "D2AI"


def info_sector(app):
    words = [INFO_MAGIC, len(app), zlib.crc32(app)]
    words.append(zlib.crc32(struct.pack("<3I", *words)))
    return struct.pack("<4I", *words)


def ihex(chunks):
    """chunks: [(address, bytes)] -> Intel hex text."""
    out = []

    def rec(typ, addr, data):
        b = bytes([len(data), addr >> 8 & 0xFF, addr & 0xFF, typ]) + data
        out.append(":" + (b + bytes([-sum(b) & 0xFF])).hex().upper())

    for addr, data in chunks:
        upper = None
        for off in range(0, len(data), 16):
            a = addr + off
            if a >> 16 != upper:
                upper = a >> 16
                rec(4, 0, struct.pack(">H", upper))
            rec(0, a & 0xFFFF, data[off:off + 16])
    rec(1, 0, b"")
    return "\n".join(out) + "\n"


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    app = open(sys.argv[1], "rb").read()
    if not 8 <= len(app) <= APP_MAX:
        sys.exit(f"app size {len(app)} out of range")
    _sp, pc = struct.unpack("<2I", app[:8])
    if not FLASH_BASE + APP_OFFSET <= pc < FLASH_BASE + APP_OFFSET + len(app):
        sys.exit(f"reset vector 0x{pc:08x} not in the app partition (wrong link address?)")
    chunks = []
    if len(sys.argv) == 4:
        boot = open(sys.argv[3], "rb").read()
        if len(boot) > BOOT_SIZE:
            sys.exit(f"bootloader size {len(boot)} > {BOOT_SIZE}")
        chunks.append((FLASH_BASE, boot))
    chunks.append((FLASH_BASE + INFO_OFFSET, info_sector(app)))
    chunks.append((FLASH_BASE + APP_OFFSET, app))
    open(sys.argv[2], "w").write(ihex(chunks))


if __name__ == "__main__":
    main()
