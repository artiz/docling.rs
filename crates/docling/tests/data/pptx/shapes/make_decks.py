"""Build the shape decks of #766/#767 the way upstream's tests do.

Each deck matches a test of docling 2.140.0 —
tests/test_backend_pptx_group_bbox.py (`group_bbox`: moved, resized,
nested and flipped groups) and tests/test_backend_pptx_shape_hyperlink.py
(`shape_link`, `list_link`, `relative_link`, `slide_jump`). Run with
python-pptx 1.0.2 from this directory; then write each `<name>.{md,json}`
with docling 2.140.0's PowerPoint backend (see tests/pptx_shapes.rs).
"""

from pptx import Presentation
from pptx.util import Emu, Inches


def add_text(shapes, text, left, top, width, height):
    box = shapes.add_textbox(left, top, width, height)
    box.text_frame.text = text
    return box


def group_bbox():
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    add_text(slide.shapes, "plain", Inches(0.5), Inches(0.5), Inches(2), Inches(1))

    moved = slide.shapes.add_group_shape()
    add_text(moved.shapes, "moved", Inches(1), Inches(2), Inches(1), Inches(1))
    moved.left, moved.top = Emu(Inches(4)), Emu(Inches(3))

    resized = slide.shapes.add_group_shape()
    add_text(resized.shapes, "resized-a", Inches(1), Inches(5), Inches(1), Inches(1))
    add_text(resized.shapes, "resized-b", Inches(2), Inches(5), Inches(1), Inches(1))
    resized.width = Emu(Inches(4))

    outer = slide.shapes.add_group_shape()
    inner = outer.shapes.add_group_shape()
    add_text(inner.shapes, "nested", Inches(6), Inches(1), Inches(1), Inches(1))
    add_text(outer.shapes, "outer", Inches(7), Inches(1), Inches(1), Inches(1))
    inner.left = Emu(Inches(6.5))
    outer.top = Emu(Inches(2))
    outer.height = Emu(Inches(2))

    flipped = slide.shapes.add_group_shape()
    add_text(flipped.shapes, "flip-left", Inches(1), Inches(6), Inches(1), Inches(1))
    add_text(flipped.shapes, "flip-right", Inches(3), Inches(6), Inches(1), Inches(1))
    flipped._element.grpSpPr.get_or_add_xfrm().set("flipH", "1")
    prs.save("group_bbox.pptx")


def shape_link():
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    plain = slide.shapes.add_textbox(Inches(1), Inches(1), Inches(6), Inches(1))
    plain.text_frame.text = "No link here"
    button = slide.shapes.add_textbox(Inches(1), Inches(3), Inches(4), Inches(1))
    button.text_frame.text = "Watch the talk"
    button.text_frame.add_paragraph().text = "Second line"
    button.click_action.hyperlink.address = "https://example.com/talk"
    prs.save("shape_link.pptx")


def list_link():
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[1])
    body = slide.placeholders[1]
    body.text_frame.text = "First point"
    body.text_frame.add_paragraph().text = "Second point"
    body.click_action.hyperlink.address = "https://example.com/points"
    prs.save("list_link.pptx")


def relative_link():
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    box = slide.shapes.add_textbox(Inches(1), Inches(1), Inches(4), Inches(1))
    box.text_frame.text = "Open the appendix"
    box.click_action.hyperlink.address = "appendix/notes.docx"
    prs.save("relative_link.pptx")


def slide_jump():
    prs = Presentation()
    first = prs.slides.add_slide(prs.slide_layouts[6])
    second = prs.slides.add_slide(prs.slide_layouts[6])
    box = first.shapes.add_textbox(Inches(1), Inches(1), Inches(4), Inches(1))
    box.text_frame.text = "Go to the next slide"
    box.click_action.target_slide = second
    prs.save("slide_jump.pptx")


if __name__ == "__main__":
    group_bbox()
    shape_link()
    list_link()
    relative_link()
    slide_jump()
