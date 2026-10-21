"""docling_rs.marker — the Marker-compatible facade.

The checks follow Marker's documented API (README of datalab-to/marker 2.0,
MIT): the ``PdfConverter(artifact_dict=create_model_dict())`` /
``text_from_rendered`` round trip, the JSON block tree (ids, polygons,
section hierarchy, images), chunks, HTML, ``save_output``'s file layout,
``ConfigParser`` and the CLI. The PDF cases need the models
(``.models/`` at the repo root, as the Rust suites do) and skip without
them; the DOCX cases run everywhere.
"""

import json
import os
import warnings
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
PDF = REPO / "tests/data/pdf/sources/2206.01062.pdf"
DOCX = REPO / "tests/data/docx/sources/word_sample.docx"

pytest.importorskip("docling_rs")

from docling_rs.marker import BlockTypes, BlockId, parse_range_str  # noqa: E402
from docling_rs.marker.config.parser import ConfigParser  # noqa: E402
from docling_rs.marker.converters.ocr import OCRConverter  # noqa: E402
from docling_rs.marker.converters.pdf import PdfConverter  # noqa: E402
from docling_rs.marker.converters.table import TableConverter  # noqa: E402
from docling_rs.marker.models import create_model_dict  # noqa: E402
from docling_rs.marker.output import output_exists, save_output, text_from_rendered  # noqa: E402
from docling_rs.marker.renderers.chunk import ChunkOutput  # noqa: E402
from docling_rs.marker.renderers.html import HTMLOutput  # noqa: E402
from docling_rs.marker.renderers.json import JSONOutput  # noqa: E402
from docling_rs.marker.renderers.markdown import MarkdownOutput  # noqa: E402

needs_models = pytest.mark.skipif(
    not (REPO / ".models/layout_heron.onnx").exists(), reason="PDF models not downloaded"
)


@pytest.fixture(scope="module")
def models():
    os.chdir(REPO)
    return create_model_dict()


@pytest.fixture(scope="module")
def two_pages(models):
    if not (REPO / ".models/layout_heron.onnx").exists():
        pytest.skip("PDF models not downloaded")
    converter = PdfConverter(artifact_dict=models, config={"page_range": [0, 1], "paginate_output": True})
    rendered = converter(str(PDF))
    return converter, rendered


# ---- the README round trip -------------------------------------------------


def test_create_model_dict_has_markers_keys(models):
    assert isinstance(models, dict)
    for key in ("layout_model", "recognition_model", "ocr_error_model", "inference_manager"):
        assert key in models


@needs_models
def test_readme_round_trip(two_pages):
    converter, rendered = two_pages
    assert isinstance(rendered, MarkdownOutput)
    text, ext, images = text_from_rendered(rendered)
    assert ext == "md"
    assert "DocLayNet" in text
    assert converter.page_count == 2
    # the images dict: Marker's `_page_<p>_Picture_<n>.jpeg` names, PIL images
    assert images and all(name.startswith("_page_0_Picture_") and name.endswith(".jpeg") for name in images)
    image = next(iter(images.values()))
    assert hasattr(image, "size") and image.mode == "RGB"
    # … referenced from the Markdown with Marker's `![](name)`
    for name in images:
        assert f"![]({name})" in text
    # docling.rs extension: the DoclingDocument rides along
    assert rendered.document is not None and rendered.document.texts


@needs_models
def test_paginate_output_uses_markers_separator(two_pages):
    _, rendered = two_pages
    assert rendered.markdown.startswith("{0}" + "-" * 48 + "\n\n")
    assert "\n\n{1}" + "-" * 48 + "\n\n" in rendered.markdown


@needs_models
def test_metadata_shape(two_pages):
    _, rendered = two_pages
    meta = rendered.metadata
    toc = meta["table_of_contents"]
    assert toc and {"title", "heading_level", "page_id", "polygon"} <= set(toc[0])
    assert any(t["title"].startswith("DocLayNet") for t in toc)
    stats = meta["page_stats"]
    assert [s["page_id"] for s in stats] == [0, 1]
    assert stats[0]["text_extraction_method"] == "pdftext"
    assert stats[0]["block_counts"] and all(len(pair) == 2 for pair in stats[0]["block_counts"])
    assert set(stats[0]["block_metadata"]) == {"llm_request_count", "llm_error_count", "llm_tokens_used"}


# ---- the other renderers ------------------------------------------------------


@needs_models
def test_json_block_tree(models):
    parser = ConfigParser({"output_format": "json", "page_range": "0"})
    converter = PdfConverter(
        artifact_dict=models, config=parser.generate_config_dict(), renderer=parser.get_renderer()
    )
    out = converter(str(PDF))
    assert isinstance(out, JSONOutput) and out.block_type == "Document"
    page = out.children[0]
    assert page.id == "/page/0/Page/0" and page.block_type == "Page"
    assert page.bbox == [0.0, 0.0, 612.0, 792.0] and len(page.polygon) == 4
    ids = [b.id for b in page.children]
    assert ids[0] == "/page/0/SectionHeader/0"
    # ids count up within the page across block types, Marker's numbering
    numbers = [int(i.rsplit("/", 1)[1]) for i in ids]
    assert numbers == sorted(numbers) and len(set(numbers)) == len(numbers)
    # the heading's own id is in its hierarchy, a paragraph's is the heading above it
    heading = page.children[0]
    assert heading.section_hierarchy and heading.id in heading.section_hierarchy.values()
    assert page.children[1].section_hierarchy == heading.section_hierarchy
    # Marker's polygon: four corners from the top-left, bbox [l, t, r, b]
    left, top, right, bottom = heading.bbox
    assert heading.polygon == [[left, top], [right, top], [right, bottom], [left, bottom]]
    assert 0 < top < bottom < 792
    # a figure with a caption is a PictureGroup of [Picture, Caption]; the
    # Picture carries its base64 image keyed by its id, the text inside
    # the figure stays inside it
    groups = [b for b in page.children if b.block_type == "PictureGroup"]
    assert groups
    kinds = [c.block_type for c in groups[0].children]
    assert kinds == ["Picture", "Caption"]
    picture = groups[0].children[0]
    assert picture.images and set(picture.images) == {picture.id}
    assert picture.html == f"<p><img src='_page_0_Picture_{picture.id.rsplit('/', 1)[1]}.jpeg'></p>"
    assert "<content-ref src='" + picture.id + "'>" in groups[0].html
    # text_from_rendered: the JSON without the metadata
    text, ext, images = text_from_rendered(out)
    assert ext == "json" and images == {}
    assert "metadata" not in json.loads(text)


@needs_models
def test_chunks(models):
    parser = ConfigParser({"output_format": "chunks", "page_range": "0-1"})
    converter = PdfConverter(
        artifact_dict=models, config=parser.generate_config_dict(), renderer=parser.get_renderer()
    )
    out = converter(str(PDF))
    assert isinstance(out, ChunkOutput)
    assert set(out.page_info) == {0, 1} and out.page_info[0]["bbox"] == [0.0, 0.0, 612.0, 792.0]
    assert {b.page for b in out.blocks} == {0, 1}
    assert out.blocks[0].block_type == "SectionHeader" and out.blocks[0].html.startswith("<h2>")
    group = next(b for b in out.blocks if b.block_type == "PictureGroup")
    assert "<img src='_page_0_Picture_" in group.html and group.images


@needs_models
def test_html(models):
    parser = ConfigParser({"output_format": "html", "page_range": "0-1", "paginate_output": True})
    converter = PdfConverter(
        artifact_dict=models, config=parser.generate_config_dict(), renderer=parser.get_renderer()
    )
    out = converter(str(PDF))
    assert isinstance(out, HTMLOutput)
    assert out.html.startswith("<!DOCTYPE html>") and '<meta charset="utf-8" />' in out.html
    assert "<div class='page' data-page-id='0'>" in out.html and "data-page-id='1'" in out.html
    assert all(f"<img src=\"{name}\"" in out.html or f"<img src='{name}'" in out.html for name in out.images)


@needs_models
def test_table_converter_keeps_tables_only(models):
    full = PdfConverter(artifact_dict=models)(str(PDF))
    table_page = min(t.prov[0].page_no for t in full.document.tables) - 1
    converter = TableConverter(artifact_dict=models, config={"page_range": [table_page]})
    out = converter(str(PDF))
    text = text_from_rendered(out)[0]
    assert text.lstrip().startswith("|")
    assert "DocLayNet contains" not in text
    tree = converter.build_document(str(PDF))
    assert all(b.type in (BlockTypes.TableGroup, BlockTypes.Table) for p in tree.pages for b in p.blocks)


@needs_models
def test_ocr_converter(models):
    converter = OCRConverter(artifact_dict=models, config={"page_range": [0]})
    out = converter(str(PDF))
    assert out.block_type == "Document" and out.metadata is None
    page = out.children[0]
    assert page.id == "/page/0/Page/0" and page.children
    line = page.children[0]
    assert line.block_type == "Line" and line.id.startswith("/page/0/Line/") and line.html
    assert len(line.polygon) == 4 and len(line.bbox) == 4


@needs_models
def test_disable_image_extraction(models):
    parser = ConfigParser({"disable_image_extraction": True, "page_range": "0"})
    config = parser.generate_config_dict()
    assert config["extract_images"] is False
    out = PdfConverter(artifact_dict=models, config=config)(str(PDF))
    assert out.images == {} and "![" not in out.markdown and "<!-- image -->" not in out.markdown


@needs_models
def test_bytesio_input(models):
    import io

    out = PdfConverter(artifact_dict=models, config={"page_range": [0]})(io.BytesIO(PDF.read_bytes()))
    assert "DocLayNet" in out.markdown


# ---- every format, no models ----------------------------------------------------


def test_docx_goes_through_pdfconverter(models):
    out = PdfConverter(artifact_dict=models)(str(DOCX))
    assert isinstance(out, MarkdownOutput)
    assert "# Swimming in the lake" in out.markdown
    assert out.images and all(name.startswith("_page_0_Picture_") for name in out.images)
    assert out.metadata["page_stats"][0]["page_id"] == 0
    counts = dict(out.metadata["page_stats"][0]["block_counts"])
    assert counts.get("SectionHeader") and counts.get("ListItem")


def test_save_output_layout(models, tmp_path):
    out = PdfConverter(artifact_dict=models)(str(DOCX))
    assert not output_exists(tmp_path, "sample")
    save_output(out, tmp_path, "sample")
    names = sorted(os.listdir(tmp_path))
    assert "sample.md" in names and "sample_meta.json" in names
    assert any(n.startswith("_page_0_Picture_") and n.endswith(".jpeg") for n in names)
    assert output_exists(tmp_path, "sample")
    meta = json.loads((tmp_path / "sample_meta.json").read_text())
    assert "table_of_contents" in meta and "page_stats" in meta


def test_ignored_marker_options_warn_once(models):
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        PdfConverter(artifact_dict=models, config={"use_llm": True, "mode": "balanced"}, processor_list=["x.Y"])
        PdfConverter(artifact_dict=models, config={"use_llm": True})
    messages = [str(w.message) for w in caught]
    assert sum("use_llm" in m for m in messages) == 1
    assert any("mode=" in m for m in messages) and any("processor_list" in m for m in messages)


def test_docling_options_pass_through(models):
    with pytest.raises(TypeError):
        PdfConverter(artifact_dict=models, config={"docling_options": {"no_such_option": 1}})


# ---- config and ids --------------------------------------------------------------


def test_parse_range_str():
    assert parse_range_str("0,5-7,3") == [0, 3, 5, 6, 7]


def test_block_id():
    assert str(BlockId(2, 5, BlockTypes.Text)) == "/page/2/Text/5"
    assert BlockId(2, 5, BlockTypes.Text).to_path() == "_page_2_Text_5"
    assert str(BlockId(3)) == "/page/3"
    assert str(BlockTypes.SectionHeader) == "SectionHeader"


def test_config_parser():
    parser = ConfigParser({
        "output_format": "json", "page_range": "0,2-3", "disable_image_extraction": True,
        "disable_multiprocessing": True, "output_dir": "/tmp/out", "processors": "a.B, c.D", "debug": False,
    })
    config = parser.generate_config_dict()
    assert config["page_range"] == [0, 2, 3]
    assert config["extract_images"] is False and config["pdftext_workers"] == 1
    assert parser.get_renderer().endswith("renderers.json.JSONRenderer")
    assert parser.get_processors() == ["a.B", "c.D"]
    assert parser.get_converter_cls() is PdfConverter
    assert ConfigParser({"converter_cls": "marker.converters.table.TableConverter"}).get_converter_cls() is TableConverter
    assert parser.get_llm_service() is None
    assert parser.get_base_filename("/x/y/report.v2.pdf") == "report.v2"
    with pytest.raises(ValueError):
        ConfigParser({"output_format": "docx"}).get_renderer()


def test_cli_writes_markers_layout(tmp_path):
    from docling_rs.marker.__main__ import main

    assert main([str(DOCX), "--output_dir", str(tmp_path)]) == 0
    folder = tmp_path / "word_sample"
    assert (folder / "word_sample.md").exists() and (folder / "word_sample_meta.json").exists()
    assert "# Swimming in the lake" in (folder / "word_sample.md").read_text()
    # a folder of files, --skip_existing
    assert main([str(DOCX.parent), "--output_dir", str(tmp_path), "--max_files", "2", "--skip_existing", "--output_format", "json"]) == 0
    assert (folder / "word_sample.md").exists()
