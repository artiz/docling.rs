"""Build the DOCX test documents of #757, #764 and #765 the way upstream's
tests do.

Each document matches a test of docling 2.140.0's tests/test_backend_msword*.py
(named next to each builder below). Run with python-docx 1.2 and lxml from
this directory; then write each `<name>.{md,json}` with docling 2.140.0's
Word backend (see tests/docx_upstream.rs).
"""

from docx import Document
from lxml import etree

_VML_NS = "urn:schemas-microsoft-com:vml"
_W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"


# --- #757: textboxes in table cells (docling#4757) ---------------------------
# test_backend_msword.py: test_textbox_in_table_cell_is_emitted_once_inside_cell,
# test_textbox_in_single_cell_table_preserves_order_and_is_not_duplicated


def _anchor_vml_textbox(paragraph, text):
    pict_xml = (
        f'<w:r xmlns:w="{_W_NS}" xmlns:v="{_VML_NS}">'
        '<w:pict><v:shape id="s1" style="width:100pt;height:40pt">'
        f"<v:textbox><w:txbxContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p>"
        "</w:txbxContent></v:textbox></v:shape></w:pict></w:r>"
    )
    paragraph._p.append(etree.fromstring(pict_xml))


def _anchor_word_mc_textbox(paragraph, text):
    mc_ns = "http://schemas.openxmlformats.org/markup-compatibility/2006"
    wp_ns = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
    a_ns = "http://schemas.openxmlformats.org/drawingml/2006/main"
    wps_ns = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
    mc_xml = (
        f'<w:r xmlns:w="{_W_NS}" xmlns:v="{_VML_NS}" xmlns:mc="{mc_ns}" '
        f'xmlns:wp="{wp_ns}" xmlns:a="{a_ns}" xmlns:wps="{wps_ns}">'
        '<mc:AlternateContent><mc:Choice Requires="wps">'
        "<w:drawing><wp:anchor><a:graphic>"
        f'<a:graphicData uri="{wps_ns}">'
        f"<wps:wsp><wps:txbx><w:txbxContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p>"
        "</w:txbxContent></wps:txbx></wps:wsp></a:graphicData></a:graphic>"
        "</wp:anchor></w:drawing></mc:Choice><mc:Fallback>"
        '<w:pict><v:shape id="s1"><v:textbox><w:txbxContent>'
        f"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"
        "</w:txbxContent></v:textbox></v:shape></w:pict>"
        "</mc:Fallback></mc:AlternateContent></w:r>"
    )
    paragraph._p.append(etree.fromstring(mc_xml))


def textbox_in_cell(name, kind, marker):
    document = Document()
    document.add_paragraph("debut")
    table = document.add_table(rows=2, cols=2)
    table.cell(0, 1).text = "b"
    table.cell(1, 0).text = "c"
    table.cell(1, 1).text = "d"
    cell0 = table.cell(0, 0)
    if kind == "word_mc_alternate":
        _anchor_word_mc_textbox(cell0.paragraphs[0], marker)
    else:
        _anchor_vml_textbox(cell0.paragraphs[0], marker)
        if kind == "vml_multi_para":
            cell0.add_paragraph("")
    document.add_paragraph("fin")
    document.save(f"{name}.docx")


def textbox_in_single_cell(name):
    document = Document()
    table = document.add_table(rows=1, cols=1)
    cell = table.cell(0, 0)
    cell.paragraphs[0].text = "First paragraph in cell"
    second_para = cell.add_paragraph()
    _anchor_vml_textbox(second_para, "Textbox in second paragraph")
    document.save(f"{name}.docx")


if __name__ == "__main__":
    textbox_in_cell("textbox_cell_vml_single_para", "vml_single_para", "MARKA")
    textbox_in_cell("textbox_cell_vml_multi_para", "vml_multi_para", "MARKA")
    textbox_in_cell("textbox_cell_word_mc", "word_mc_alternate", "MARKW")
    textbox_in_single_cell("textbox_single_cell")
