"""Packs Feline's PNG icons into one .ico (PNG-compressed entries, Vista+)."""
import struct, sys, os

icons_dir, out = sys.argv[1], sys.argv[2]
sizes = [16, 32, 48, 64, 128, 256]
images = [open(os.path.join(icons_dir, f"{n}x{n}", "apps", "feline.png"), "rb").read() for n in sizes]

header = struct.pack("<HHH", 0, 1, len(images))
offset = 6 + 16 * len(images)
entries = b""
for n, data in zip(sizes, images):
    dim = 0 if n >= 256 else n  # 0 means 256
    entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
with open(out, "wb") as f:
    f.write(header + entries + b"".join(images))
print("wrote", out, os.path.getsize(out), "bytes")
