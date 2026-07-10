"""Tests for the docling-shaped configuration surface and re-exports
(declarative path only — no ML models required)."""

import io
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
HTML = REPO / "tests/data/html/sources/hyperlink_03.html"

docling_rs = pytest.importorskip("docling_rs")


def test_input_format_matches_docling_values():
    from docling_rs import InputFormat

    # docling's own members carry these exact string values.
    assert InputFormat.PDF == "pdf"
    assert InputFormat.DOCX == "docx"
    assert InputFormat.XML_JATS == "xml_jats"
    assert InputFormat.JSON_DOCLING == "json_docling"


def test_reexports_are_importable():
    import docling_rs as d

    for name in (
        "DocumentConverter",
        "ConversionResult",
        "ConversionStatus",
        "DoclingDocument",
        "ImageRefMode",
        "InputFormat",
        "DocumentStream",
        "PdfPipelineOptions",
        "PdfFormatOption",
        "AcceleratorOptions",
        "AcceleratorDevice",
        "TableFormerMode",
    ):
        assert hasattr(d, name), name


def test_pipeline_options_via_format_options_convert():
    from docling_rs import (
        DocumentConverter,
        InputFormat,
        PdfFormatOption,
        PdfPipelineOptions,
        AcceleratorOptions,
    )

    opts = PdfPipelineOptions(
        do_ocr=False,
        do_table_structure=False,
        accelerator_options=AcceleratorOptions(num_threads=2),
    )
    conv = DocumentConverter(
        format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)}
    )
    # Options are PDF-pipeline knobs; a declarative HTML convert still works.
    res = conv.convert(HTML)
    assert res.status == "success"
    assert res.document.export_to_markdown()


def test_shorthand_flags_convert():
    from docling_rs import DocumentConverter

    res = DocumentConverter(do_ocr=False, do_table_structure=True).convert(HTML)
    assert res.status == "success"


def test_document_stream_source():
    from docling_rs import DocumentConverter, DocumentStream

    stream = DocumentStream(name="hyperlink_03.html", stream=io.BytesIO(HTML.read_bytes()))
    res = DocumentConverter().convert(stream)
    assert res.status == "success"
    assert res.document.texts


def test_image_ref_mode_reexport_drives_export():
    from docling_rs import DocumentConverter, ImageRefMode

    doc = DocumentConverter().convert(HTML).document
    # docling-core's own export honours the re-exported enum.
    md = doc.export_to_markdown(image_mode=ImageRefMode.EMBEDDED)
    assert isinstance(md, str)
