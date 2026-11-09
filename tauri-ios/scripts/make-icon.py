"""Writes a plain 1024x1024 app-icon.png (blue square with a lighter inner square). Stdlib only."""
import struct, zlib, sys

N = 1024
rows = []
for y in range(N):
    row = bytearray([0])
    for x in range(N):
        inner = 256 <= x < 768 and 256 <= y < 768
        row += bytes((0x4F, 0xC1, 0xFF, 255)) if inner else bytes((0x00, 0x7A, 0xCC, 255))
    rows.append(bytes(row))

def chunk(tag, data):
    c = struct.pack(">I", len(data)) + tag + data
    return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0)) \
    + chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
open(sys.argv[1] if len(sys.argv) > 1 else "app-icon.png", "wb").write(png)
