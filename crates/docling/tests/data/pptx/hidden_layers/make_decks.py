"""Build the hidden-layer decks of #754/#755 the way upstream's tests do.

Each deck matches a test of docling 2.140.0 — tests/test_backend_pptx.py
(`test_pptx_hidden_slide_goes_to_invisible_layer`,
`test_pptx_footer_placeholders_go_to_furniture_layer`) and
tests/test_backend_pptx_hidden_shapes.py. Run with python-pptx 1.0.2 and
Pillow from this directory; then write each `<name>.{md,json}` with
docling 2.140.0's PowerPoint backend (see tests/pptx_hidden_layers.rs).
"""

from lxml import etree
from PIL import Image
from pptx import Presentation
from pptx.util import Inches

NS = (
    'xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" '
    'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
)


def hide(shape, value="1"):
    shape._element.xpath("./*/p:cNvPr")[0].set("hidden", value)


def hidden_slide():
    prs = Presentation()
    for title, body, hidden in [
        ("Visible slide", "Presented text", False),
        ("Hidden slide", "Backup text", True),
    ]:
        slide = prs.slides.add_slide(prs.slide_layouts[5])
        slide.shapes.title.text = title
        box = slide.shapes.add_textbox(Inches(1), Inches(2), Inches(4), Inches(1))
        box.text_frame.text = body
        if hidden:
            slide._element.set("show", "0")
        slide.notes_slide.notes_text_frame.text = f"Notes for {title}"
    prs.save("hidden_slide.pptx")


def footer():
    prs = Presentation()
    for body, hidden in [("Presented text", False), ("Backup text", True)]:
        slide = prs.slides.add_slide(prs.slide_layouts[6])
        box = slide.shapes.add_textbox(Inches(1), Inches(1), Inches(4), Inches(1))
        box.text_frame.text = body
        for idx, (ph_type, text) in enumerate(
            [("dt", "10 October 2026"), ("ftr", "Confidential"), ("sldNum", "1")],
            start=10,
        ):
            slide.shapes._spTree.append(
                etree.fromstring(
                    f'<p:sp {NS}><p:nvSpPr><p:cNvPr id="{idx}" name="{ph_type}"/>'
                    "<p:cNvSpPr/><p:nvPr>"
                    f'<p:ph type="{ph_type}" idx="{idx}"/></p:nvPr></p:nvSpPr>'
                    f"<p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>{text}</a:t>"
                    "</a:r></a:p></p:txBody></p:sp>"
                )
            )
        if hidden:
            slide._element.set("show", "0")
    prs.save("footer.pptx")


def hidden_shape():
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    shown = slide.shapes.add_textbox(Inches(1), Inches(1), Inches(6), Inches(1))
    shown.text_frame.text = "Shown shape text"
    hidden = slide.shapes.add_textbox(Inches(1), Inches(3), Inches(6), Inches(1))
    hidden.text_frame.text = "Hidden shape text"
    hide(hidden, "true")
    not_hidden = slide.shapes.add_textbox(Inches(1), Inches(5), Inches(6), Inches(1))
    not_hidden.text_frame.text = "Explicitly shown text"
    hide(not_hidden, "0")
    prs.save("hidden_shape.pptx")


def hidden_group():
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    hidden_group = slide.shapes.add_group_shape()
    for index, text in enumerate(["Hidden group first", "Hidden group second"]):
        box = hidden_group.shapes.add_textbox(
            Inches(1), Inches(1 + index), Inches(4), Inches(0.5)
        )
        box.text_frame.text = text
    hide(hidden_group)
    shown_group = slide.shapes.add_group_shape()
    kept = shown_group.shapes.add_textbox(Inches(1), Inches(4), Inches(4), Inches(0.5))
    kept.text_frame.text = "Shown group kept"
    dropped = shown_group.shapes.add_textbox(Inches(1), Inches(5), Inches(4), Inches(0.5))
    dropped.text_frame.text = "Shown group hidden member"
    hide(dropped)
    prs.save("hidden_group.pptx")


def hidden_table_picture():
    Image.new("RGB", (40, 20), "red").save("pixel.png")
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    frame = slide.shapes.add_table(1, 2, Inches(1), Inches(1), Inches(4), Inches(1))
    frame.table.cell(0, 0).text = "Hidden cell"
    frame.table.cell(0, 1).text = "Other cell"
    hide(frame)
    picture = slide.shapes.add_picture("pixel.png", Inches(1), Inches(3))
    hide(picture)
    shown = slide.shapes.add_textbox(Inches(1), Inches(5), Inches(4), Inches(1))
    shown.text_frame.text = "Shown text"
    prs.save("hidden_table_picture.pptx")


if __name__ == "__main__":
    hidden_slide()
    footer()
    hidden_shape()
    hidden_group()
    hidden_table_picture()
