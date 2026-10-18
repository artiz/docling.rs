"""Build the ODF test documents of #753, #758 and #759 the way upstream's
tests do.

Each document matches a test of docling 2.140.0's
tests/test_backend_opendocument.py (named in its builder). Run with odfdo
installed, passing docling's `tests/data/odf/sources` directory (the list
document reuses the image and chart frames of its `text_document_02.odt`):

    python make_docs.py <docling>/tests/data/odf/sources

`<name>.md`/`.json` are then written by docling's own OpenDocument
backends, never by docling.rs.
"""

import sys
import tempfile
from pathlib import Path

from odfdo import (
    Document as OdfDocument,
    DrawPage,
    Element,
    Frame,
    Header,
    List as OdfList,
    ListItem,
    Note,
    Paragraph,
    Section,
    Style,
    Table,
)
from PIL import Image

HERE = Path(__file__).parent


# --- #753: hidden sections and slides (docling#4767, docling#4860) ----------


def hidden_section(name):
    # test_odt_hidden_section_goes_to_invisible_layer
    source = OdfDocument("text")
    body = source.body
    body.clear()
    body.append(Paragraph("Question"))
    hidden = Section(name="Answers")
    hidden.set_attribute("text:display", "none")
    hidden.append(Header(1, "Answer key"))
    table = Table("Answers", width=1, height=1)
    table.set_value("A1", "42")
    hidden.append(table)
    nested = Section(name="Notes")
    nested.append(Paragraph("Nested note"))
    hidden.append(nested)
    body.append(hidden)
    shown = Section(name="Shown")
    shown.append(Paragraph("Shown section"))
    body.append(shown)
    source.save(str(HERE / f"{name}.odt"))


def hidden_slide(name):
    # test_odp_hidden_slide_goes_to_invisible_layer
    doc = OdfDocument("presentation")
    body = doc.body
    body.clear()
    hidden_style = Style("drawing-page", name="dp_hidden")
    properties = Element.from_tag("style:drawing-page-properties")
    properties.set_attribute("presentation:visibility", "hidden")
    hidden_style.append(properties)
    doc.insert_style(hidden_style, automatic=True)
    for index, (title, text, style_name) in enumerate(
        [
            ("Visible slide", "Presented text", None),
            ("Hidden slide", "Backup text", "dp_hidden"),
        ]
    ):
        page = DrawPage(f"page{index + 1}", name=title)
        if style_name is not None:
            page.set_attribute("draw:style-name", style_name)
        page.append(
            Frame.text_frame([text], size=("10cm", "2cm"), position=("1cm", "1cm"))
        )
        body.append(page)
    doc.save(str(HERE / f"{name}.odp"))


# --- #758: pictures anchored to an ODS sheet (docling#4678) -----------------


def sheet_picture(name, tmp):
    # test_ods_sheet_anchored_picture_is_kept
    image_path = tmp / "sheet_image.png"
    Image.new("RGB", (2, 2), "red").save(image_path)
    doc = OdfDocument("spreadsheet")
    body = doc.body
    body.clear()
    table = Table("Sheet1", width=2, height=2)
    body.append(table)
    cell = table.get_cell("B2")
    cell.append(Frame.image_frame(doc.add_file(str(image_path)), size=("1cm", "1cm")))
    table.set_cell("B2", cell)
    shapes = Element.from_tag("table:shapes")
    shapes.append(
        Frame.image_frame(
            doc.add_file(str(image_path)), size=("1cm", "1cm"), position=("1cm", "1cm")
        )
    )
    table.insert(shapes, position=0)
    doc.save(str(HERE / f"{name}.ods"))


def sheet_vector_picture(name, tmp):
    # test_ods_sheet_anchored_vector_picture_is_skipped
    image_path = tmp / "sheet_image.svg"
    image_path.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2">'
        '<rect width="2" height="2" fill="red"/></svg>'
    )
    doc = OdfDocument("spreadsheet")
    body = doc.body
    body.clear()
    table = Table("Sheet1", width=1, height=1)
    body.append(table)
    shapes = Element.from_tag("table:shapes")
    shapes.append(
        Frame.image_frame(
            doc.add_file(str(image_path)), size=("1cm", "1cm"), position=("1cm", "1cm")
        )
    )
    table.insert(shapes, position=0)
    doc.save(str(HERE / f"{name}.ods"))


# --- #759: list items with only an image, chart or note (docling#4693) ------


def list_item_only_image(name, sources):
    # test_odt_list_item_with_only_an_image_has_no_text
    source = OdfDocument(str(sources / "text_document_02.odt"))
    style = Style("list", name="Numbered")
    style.set_level_style(1, num_format="1", suffix=".")
    style.set_level_style(2, num_format="1", suffix=")")
    source.insert_style(style)
    body = source.body
    frames = {frame.name: frame.clone for frame in body.get_frames()}
    body.clear()
    image, chart, footnote = Paragraph(), Paragraph(), Paragraph()
    image.append(frames["Image1"])
    chart.append(frames["Object1"])
    footnote.append(Note(citation="1", body="Note body."))
    nested = OdfList(style="Numbered")
    nested.append(ListItem("Nested one"))
    odf_list = OdfList(style="Numbered")
    for children in [
        [Paragraph("First")],
        [image],
        [chart],
        [footnote],
        [image.clone, nested],
        [Paragraph("Last")],
    ]:
        item = ListItem()
        for child in children:
            item.append(child)
        odf_list.append(item)
    body.append(odf_list)
    source.save(str(HERE / f"{name}.odt"))


if __name__ == "__main__":
    sources = Path(sys.argv[1])
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        hidden_section("hidden_section")
        hidden_slide("hidden_slide")
        sheet_picture("sheet_picture", tmp)
        sheet_vector_picture("sheet_vector_picture", tmp)
        list_item_only_image("list_item_only_image", sources)
