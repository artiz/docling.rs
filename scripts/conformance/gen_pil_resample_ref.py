#!/usr/bin/env python3
"""Reference hashes for the PIL-exact resampler port (resample.rs pil_tests).

Builds the same LCG image as the Rust test, resizes it with genuine Pillow,
and prints the FNV-1a-64 of the raw RGB bytes for each case. Paste the values
into `resample.rs::pil_tests`. Requires Pillow (any version implementing the
classic Resample.c fixed-point path; verified with 12.3.0).
"""
from PIL import Image

def lcg_image(w, h):
    img = Image.new("RGB", (w, h))
    px = img.load()
    state = 0x2545F491
    def nxt():
        nonlocal state
        state = (state * 6364136223846793005 + 1442695040888963407) % (1 << 64)
        return (state >> 33) & 0xFF
    for y in range(h):
        for x in range(w):
            px[x, y] = (nxt(), nxt(), nxt())
    return img

def fnv1a(data):
    h = 0xCBF29CE484222325
    for b in data:
        h = ((h ^ b) * 0x100000001B3) % (1 << 64)
    return h

img = lcg_image(61, 47)
for name, size, resample in [
    ("PIL_HASH_BILINEAR_DOWN", (40, 30), Image.Resampling.BILINEAR),
    ("PIL_HASH_BILINEAR_UP", (97, 83), Image.Resampling.BILINEAR),
    ("PIL_HASH_BICUBIC_DOWN", (40, 30), Image.Resampling.BICUBIC),
    ("PIL_HASH_BICUBIC_UP", (97, 83), Image.Resampling.BICUBIC),
    ("PIL_HASH_BILINEAR_640", (640, 640), Image.Resampling.BILINEAR),
]:
    out = img.resize(size, resample=resample)
    print(f"    const {name}: u64 = 0x{fnv1a(out.tobytes()):016x};")
