"""Build the DOCX test documents of #757, #764 and #765 the way upstream's
tests do.

Each document matches a test of docling 2.140.0's tests/test_backend_msword*.py
(named next to each builder below), except `style_bold`, docling.rs's own
probe of the Word backend's paragraph-level bold. Run with python-docx 1.2 and lxml from
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


# --- #764: blank paragraphs before a resumed list item (docling#4674) --------
# test_backend_msword_spacer.py:
# test_spacers_before_a_resumed_list_item_in_a_table_cell_keep_the_table,
# test_text_after_a_resumed_list_keeps_only_its_own_comment


def resumed_list_in_cell(name):
    doc = Document()
    table = doc.add_table(rows=3, cols=1)
    table.cell(0, 0).paragraphs[0].text = "row zero"
    cell = table.cell(1, 0)
    cell.paragraphs[0].text = "steps"
    for text in ("first", "second", "", "", "third"):
        cell.add_paragraph(text, style="List Number" if text else None)
    table.cell(2, 0).paragraphs[0].text = "row two"
    doc.add_paragraph("after the table")
    doc.save(f"{name}.docx")


def commented_spacers(name):
    doc = Document()
    doc.add_paragraph("first", style="List Number")
    spacer_runs = [doc.add_paragraph().add_run("") for _ in range(2)]
    doc.add_paragraph("second", style="List Number")
    note = doc.add_paragraph("note")
    for run in spacer_runs:
        doc.add_comment(run, text="on a spacer")
    doc.add_comment(note.runs, text="on the note")
    doc.save(f"{name}.docx")


# --- #765: numbering through a linked numbering style (docling#4727) --------
# Upstream's tests are unit tests of `_get_level_element`, `_add_heading`
# and `_apply_caption_numbering` (test_backend_msword_lists.py:
# test_get_level_element_follows_num_style_link; test_backend_msword.py:
# test_numbered_heading_syncs_list_counter,
# test_custom_caption_numbering_is_added_to_paragraph_text); these documents
# put the same numbering definitions through a whole conversion.

from docx.enum.style import WD_STYLE_TYPE  # noqa: E402
from docx.oxml import OxmlElement  # noqa: E402
from docx.oxml.ns import qn  # noqa: E402


def _num(numbering, num_id, abstract_id):
    num = OxmlElement("w:num")
    num.set(qn("w:numId"), num_id)
    ref = OxmlElement("w:abstractNumId")
    ref.set(qn("w:val"), abstract_id)
    num.append(ref)
    numbering.append(num)


def _lvl(ilvl, fmt, text, start="1"):
    level = OxmlElement("w:lvl")
    level.set(qn("w:ilvl"), str(ilvl))
    for tag, val in (("w:start", start), ("w:numFmt", fmt), ("w:lvlText", text)):
        el = OxmlElement(tag)
        el.set(qn("w:val"), val)
        level.append(el)
    return level


def _numbered(paragraph, num_id, ilvl):
    ppr = paragraph._p.get_or_add_pPr()
    numpr = OxmlElement("w:numPr")
    for tag, val in (("w:ilvl", str(ilvl)), ("w:numId", num_id)):
        el = OxmlElement(tag)
        el.set(qn("w:val"), val)
        numpr.append(el)
    ppr.append(numpr)


def num_style_link(name):
    doc = Document()
    numbering = doc.part.numbering_part.element
    linked_abstract = OxmlElement("w:abstractNum")
    linked_abstract.set(qn("w:abstractNumId"), "300")
    style_link = OxmlElement("w:numStyleLink")
    style_link.set(qn("w:val"), "LinkedNumberingStyle")
    linked_abstract.append(style_link)
    numbering.append(linked_abstract)
    linked_style = doc.styles.add_style("LinkedNumberingStyle", WD_STYLE_TYPE.LIST)
    ppr = linked_style.element.get_or_add_pPr()
    numpr = OxmlElement("w:numPr")
    num_id = OxmlElement("w:numId")
    num_id.set(qn("w:val"), "401")
    numpr.append(num_id)
    ppr.append(numpr)
    target_abstract = OxmlElement("w:abstractNum")
    target_abstract.set(qn("w:abstractNumId"), "400")
    target_abstract.append(_lvl(0, "lowerRoman", "%1)"))
    numbering.append(target_abstract)
    for nid, aid in (("301", "300"), ("401", "400")):
        _num(numbering, nid, aid)
    doc.add_paragraph("Before the list")
    for text in ("alpha", "beta", "gamma"):
        _numbered(doc.add_paragraph(text), "301", 0)
    doc.add_paragraph("After the list")
    doc.save(f"{name}.docx")


def caption_numbering(name):
    doc = Document()
    numbering = doc.part.numbering_part.element
    abstract = OxmlElement("w:abstractNum")
    abstract.set(qn("w:abstractNumId"), "500")
    abstract.append(_lvl(0, "decimal", "%1"))
    for ilvl in range(1, 6):
        abstract.append(_lvl(ilvl, "decimal", f"%1.%{ilvl + 1}"))
    abstract.append(_lvl(6, "decimal", "Tabell %1.%7"))
    numbering.append(abstract)
    _num(numbering, "501", "500")
    for chapter, captions in (
        ("Innledning", ["Saker i tingrettene", "Saker i lagmannsrettene"]),
        ("Mekling", ["Antall meklinger"]),
        ("Avslutning", ["Oppsummering"]),
    ):
        _numbered(doc.add_paragraph(chapter, style="Heading 1"), "501", 0)
        doc.add_paragraph("Body text of the chapter.")
        for caption in captions:
            _numbered(doc.add_paragraph(caption, style="Caption"), "501", 6)
    doc.save(f"{name}.docx")


# --- paragraph-level bold (`_get_format_from_run`) ---------------------------
# Not an upstream test: docling.rs's own probe of the paragraph-mark
# (`w:pPr/w:rPr/w:b`) and style-chain bold steps of docling 2.140.0's
# `_get_format_from_run` — which paragraph kinds print it (body text, list
# items, rich cells, hyperlinks) and which do not (title, headings, a plain
# cell).


def _mark_bold(paragraph, val=None):
    rpr = OxmlElement("w:rPr")
    b = OxmlElement("w:b")
    if val is not None:
        b.set(qn("w:val"), val)
    rpr.append(b)
    paragraph._p.get_or_add_pPr().append(rpr)


def _hyperlink(paragraph, url, text):
    from docx.opc.constants import RELATIONSHIP_TYPE as RT

    rid = paragraph.part.relate_to(url, RT.HYPERLINK, is_external=True)
    link = OxmlElement("w:hyperlink")
    link.set(qn("r:id"), rid)
    run = OxmlElement("w:r")
    t = OxmlElement("w:t")
    t.text = text
    run.append(t)
    link.append(run)
    paragraph._p.append(link)


def style_bold(name):
    doc = Document()
    styles = doc.styles
    base = styles.add_style("BoldBase", WD_STYLE_TYPE.PARAGRAPH)
    base.font.bold = True
    child = styles.add_style("BoldChild", WD_STYLE_TYPE.PARAGRAPH)
    child.base_style = base
    off = styles.add_style("NotBold", WD_STYLE_TYPE.PARAGRAPH)
    off.font.bold = False
    off.base_style = base

    doc.add_paragraph("Probe title", style="Title")
    doc.add_heading("Plain heading", 1)
    h = doc.add_heading("", 2)
    h.add_run("Bold").bold = True
    h.add_run(" and ")
    h.add_run("italic").italic = True
    h.add_run(" heading")
    _hyperlink(doc.add_heading("Heading with ", 2), "https://example.com/h", "a link")
    doc.add_paragraph("A caption in the Caption style", style="Caption")
    _mark_bold(doc.add_paragraph("Paragraph-mark bold text"))
    _mark_bold(doc.add_paragraph("Paragraph-mark bold off"), "0")
    p = doc.add_paragraph("Mark bold before ")
    _mark_bold(p)
    _hyperlink(p, "https://example.com/a", "the link")
    p.add_run(" and after")
    p = doc.add_paragraph("Chain bold before ", style="BoldChild")
    _hyperlink(p, "https://example.com/b", "styled link")
    p.add_run(" after")
    p = doc.add_paragraph("", style="BoldChild")
    p.add_run("explicit run bold off").bold = False
    p.add_run(" tail")
    doc.add_paragraph("Style bold off overrides base", style="NotBold")
    doc.add_paragraph("", style="BoldChild").add_run("italic in bold style").italic = True
    _mark_bold(doc.add_paragraph("Bullet with bold mark", style="List Bullet"))
    doc.add_paragraph("Plain bullet", style="List Bullet")
    doc.add_paragraph("Numbered one", style="List Number")
    _mark_bold(doc.add_paragraph("Numbered bold mark", style="List Number"))
    # A table straight after a list joins the list group upstream; a body
    # paragraph keeps this probe about bold.
    doc.add_paragraph("Before the table.")
    table = doc.add_table(rows=2, cols=2)
    cell = table.cell(0, 0)
    cell.paragraphs[0].text = "single bold-style cell"
    cell.paragraphs[0].style = styles["BoldChild"]
    cell = table.cell(0, 1)
    cell.paragraphs[0].text = "rich first"
    cell.add_paragraph("rich bold second", style="BoldChild")
    cell = table.cell(1, 0)
    cell.paragraphs[0].add_run("run bold").bold = True
    cell.paragraphs[0].add_run(" plain")
    _mark_bold(cell.paragraphs[0])
    cell = table.cell(1, 1)
    cell.paragraphs[0].text = "plain"
    _mark_bold(cell.paragraphs[0])
    doc.add_paragraph("End.")
    doc.save(f"{name}.docx")


if __name__ == "__main__":
    textbox_in_cell("textbox_cell_vml_single_para", "vml_single_para", "MARKA")
    textbox_in_cell("textbox_cell_vml_multi_para", "vml_multi_para", "MARKA")
    textbox_in_cell("textbox_cell_word_mc", "word_mc_alternate", "MARKW")
    textbox_in_single_cell("textbox_single_cell")
    resumed_list_in_cell("resumed_list_in_cell")
    commented_spacers("commented_spacers")
    num_style_link("num_style_link")
    caption_numbering("caption_numbering")
    style_bold("style_bold")
