"""Regenerate the #666 text-line orientation fixtures (Pillow + DejaVu Sans).

    python3 crates/docling/tests/fixtures/textline/generate.py

upside_down.png  an upright line over the same kind of line turned 180°
axis_title.png   a chart-like picture with a bottom-to-top axis title
skewed.png       a line and its twin rotated ~10°
"""
import os
from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
FONT = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 28)


def line(text, pad=12):
    l, t, r, b = FONT.getbbox(text)
    img = Image.new("RGB", (r - l + 2 * pad, b - t + 2 * pad), "white")
    ImageDraw.Draw(img).text((pad - l, pad - t), text, font=FONT, fill="black")
    return img


def save(img, name):
    img.save(os.path.join(HERE, name), optimize=True)


def upside_down():
    a = line("Quarterly revenue grew by 12 percent")
    b = line("Open the Settings dialog to continue").rotate(180)
    img = Image.new("RGB", (720, 180), "white")
    img.paste(a, (20, 20))
    img.paste(b, (20, 100))
    save(img, "upside_down.png")


def axis_title():
    img = Image.new("RGB", (640, 420), "white")
    d = ImageDraw.Draw(img)
    d.line((110, 30, 110, 360), fill="black", width=2)
    d.line((110, 360, 610, 360), fill="black", width=2)
    for i, h in enumerate((120, 200, 90, 260, 170)):
        x = 140 + i * 90
        d.rectangle((x, 360 - h, x + 50, 359), fill=(120, 140, 200))
    title = line("Revenue in millions").rotate(90, expand=True)
    img.paste(title, (30, 360 - title.height))
    months = line("Jan Feb Mar Apr May")
    img.paste(months, (150, 368))
    save(img, "axis_title.png")


def skewed():
    text = "Shipping address confirmed"
    a = line(text)
    b = line(text).rotate(10, expand=True, fillcolor="white", resample=Image.BICUBIC)
    img = Image.new("RGB", (560, 260), "white")
    img.paste(a, (20, 20))
    img.paste(b, (20, 90))
    save(img, "skewed.png")


upside_down()
axis_title()
skewed()
