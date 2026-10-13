#!/usr/bin/env python3
"""Word recall of the picture OCR on rendered UI pictures (#666).

Generates a seeded synthetic set — dialogs and forms in UI-sized type,
charts with bottom-to-top axis titles, tables with vertical headers,
sideways and upside-down document photos, a capture over RapidOCR's
2000 px cap — with the drawn words as ground truth, then reads it with

  * docling.rs' picture OCR (the `picture_ocr` example of docling-pdf) at
    the default resolution and at `picture_ocr_scale` 1.0, with
    `ocr_textline_orientation` off and on;
  * RapidOCR 3.9.2 with its defaults (PP-OCRv6 det/rec, the textline
    classifier on), when the `rapidocr` package is importable.

Recall = matched ground-truth words / ground-truth words (case-folded,
punctuation stripped, multiset). Pillow + DejaVu Sans; run from the repo
root with the OCR models in `.models/`:

    python3 scripts/conformance/picture_ocr_recall.py [OUTDIR]
"""
import collections
import os
import random
import re
import subprocess
import sys

from PIL import Image, ImageDraw, ImageFont

FONT_DIR = "/usr/share/fonts/truetype/dejavu"
WORDS = (
    "account settings profile password security billing invoice payment "
    "address shipping order status pending approved rejected download upload "
    "export import delete archive search filter results customer project "
    "dashboard report monthly weekly revenue expenses budget forecast total "
    "network storage backup restore schedule calendar meeting reminder notes "
    "language region timezone privacy notifications email phone device "
    "version update install license support contact feedback history"
).split()


def font(size, bold=False):
    name = "DejaVuSans-Bold.ttf" if bold else "DejaVuSans.ttf"
    return ImageFont.truetype(os.path.join(FONT_DIR, name), size)


def phrase(rng, n):
    return " ".join(rng.choice(WORDS).capitalize() if i == 0 else rng.choice(WORDS) for i in range(n))


class Canvas:
    def __init__(self, w, h, bg="white"):
        self.img = Image.new("RGB", (w, h), bg)
        self.draw = ImageDraw.Draw(self.img)
        self.words = []

    def text(self, xy, s, size, fill="black", bold=False):
        self.draw.text(xy, s, font=font(size, bold), fill=fill)
        self.words += s.split()

    def rotated(self, xy, s, size, angle, fill="black"):
        f = font(size)
        l, t, r, b = f.getbbox(s)
        tile = Image.new("RGBA", (r - l + 8, b - t + 8), (0, 0, 0, 0))
        ImageDraw.Draw(tile).text((4 - l, 4 - t), s, font=f, fill=fill)
        tile = tile.rotate(angle, expand=True, resample=Image.BICUBIC)
        self.img.paste(tile, xy, tile)
        self.words += s.split()


def dialog(rng, size):
    c = Canvas(rng.randint(520, 720), rng.randint(320, 460))
    c.draw.rectangle((0, 0, c.img.width, 30), fill=(230, 232, 236))
    c.text((12, 8), phrase(rng, 3), size + 1, bold=True)
    y = 50
    while y < c.img.height - 80:
        c.text((20, y), phrase(rng, rng.randint(1, 3)) + ":", size)
        c.draw.rectangle((200, y - 4, c.img.width - 30, y + size + 6), outline=(160, 160, 170))
        c.text((208, y), phrase(rng, rng.randint(1, 4)), size, fill=(60, 60, 60))
        y += size + 22
    for i, label in enumerate((rng.choice(WORDS).capitalize(), rng.choice(WORDS).capitalize())):
        x = c.img.width - 220 + i * 105
        c.draw.rectangle((x, c.img.height - 50, x + 95, c.img.height - 20), fill=(60, 110, 200))
        c.text((x + 10, c.img.height - 43), label, size, fill="white")
    return c


def chart(rng):
    c = Canvas(rng.randint(560, 760), rng.randint(380, 480))
    w, h = c.img.size
    c.text((w // 4, 10), phrase(rng, 4), 15, bold=True)
    c.draw.line((100, 50, 100, h - 70), fill="black", width=2)
    c.draw.line((100, h - 70, w - 20, h - 70), fill="black", width=2)
    for i in range(6):
        x = 120 + i * (w - 160) // 6
        bh = rng.randint(40, h - 140)
        c.draw.rectangle((x, h - 70 - bh, x + 30, h - 71), fill=(110, 140, 200))
        c.text((x - 4, h - 62), rng.choice(WORDS)[:5], 11)
    c.rotated((20, 80), phrase(rng, 3), 14, 90)  # bottom-to-top axis title
    c.text((w // 3, h - 35), phrase(rng, 2), 14)
    return c


def table(rng):
    c = Canvas(rng.randint(600, 760), 360)
    for i in range(5):
        c.rotated((40 + i * 120, 10), phrase(rng, 2), 13, 90)
    for r in range(6):
        for i in range(5):
            c.text((40 + i * 120, 150 + r * 32), rng.choice(WORDS), 13)
    return c


def photo(rng, angle):
    c = Canvas(640, 420, bg=(250, 248, 240))
    for i in range(7):
        c.text((30, 30 + i * 52), phrase(rng, rng.randint(4, 6)), 22)
    c.img = c.img.rotate(angle, expand=True, fillcolor=(250, 248, 240))
    return c


def capture(rng):
    c = Canvas(2600, 1500)
    for row in range(22):
        for col in range(4):
            c.text((40 + col * 640, 40 + row * 64), phrase(rng, 3), 16)
    return c


def build(out):
    rng = random.Random(666)
    items = []
    for i in range(8):
        items.append(("dialog", dialog(rng, rng.choice((12, 13, 14)))))
    for i in range(6):
        items.append(("chart", chart(rng)))
    for i in range(4):
        items.append(("table", table(rng)))
    for angle in (90, 180, 270, 180):
        items.append((f"photo{angle}", photo(rng, angle)))
    for i in range(2):
        items.append(("capture", capture(rng)))
    os.makedirs(out, exist_ok=True)
    truth = {}
    for n, (kind, c) in enumerate(items):
        path = os.path.join(out, f"{n:02d}_{kind}.png")
        c.img.save(path)
        truth[path] = c.words
    return truth


def kind_of(path):
    return os.path.basename(path)[3:-4]


def norm(words):
    out = []
    for w in words:
        w = re.sub(r"[^\w]", "", w.lower())
        if w:
            out.append(w)
    return out


def recall(truth, read):
    hit = tot = 0
    for path, words in truth.items():
        gt = collections.Counter(norm(words))
        got = collections.Counter(norm(read.get(path, "").replace("|", " ").split()))
        tot += sum(gt.values())
        hit += sum(min(n, got[w]) for w, n in gt.items())
    return hit / tot if tot else 0.0


def docling_rs(paths, *flags):
    exe = ["cargo", "run", "--release", "-q", "-p", "docling-pdf", "--example", "picture_ocr", "--"]
    out = subprocess.run(exe + list(flags) + paths, capture_output=True, text=True, check=True).stdout
    return dict(line.split("\t", 1) for line in out.splitlines() if "\t" in line)


def rapidocr(paths):
    try:
        from rapidocr import RapidOCR
    except ImportError:
        return None
    engine = RapidOCR()
    return {p: " ".join(engine(p).txts or ()) for p in paths}


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/picture_ocr_recall"
    truth = build(out)
    paths = sorted(truth)
    kinds = sorted({kind_of(p) for p in paths})
    runs = {
        "docling.rs default": docling_rs(paths),
        "docling.rs scale 1.0": docling_rs(paths, "--scale", "1.0"),
        "docling.rs orient": docling_rs(paths, "--orient"),
        "docling.rs scale 1.0 + orient": docling_rs(paths, "--scale", "1.0", "--orient"),
    }
    ref = rapidocr(paths)
    if ref is not None:
        runs["RapidOCR 3.9.2 (defaults)"] = ref
    print(f"| run | all | " + " | ".join(kinds) + " |")
    print("|---|---|" + "---|" * len(kinds))
    for name, read in runs.items():
        cells = [recall({p: w for p, w in truth.items() if kind_of(p) == k}, read) for k in kinds]
        print(f"| {name} | {recall(truth, read):.3f} | " + " | ".join(f"{v:.3f}" for v in cells) + " |")


if __name__ == "__main__":
    main()
