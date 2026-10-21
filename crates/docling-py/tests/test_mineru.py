"""docling_rs.mineru — the MinerU-compatible facade.

The checks follow MinerU 4.0's SDK docs (opendatalab/MinerU, AGPL-3.0; the
API shapes, not its code): ``parse`` → ``ParseResult`` with ``pages``,
``markdown()``, ``to_json()`` / ``from_json()`` round trip, ``save(writer)``'s
file layout, the Middle JSON block shapes, the ``render`` targets, the
page-range grammar and the ``mineru-kit parse`` CLI. The PDF cases need the
models (``.models/`` at the repo root) and skip without them; the DOCX cases
run everywhere.
"""

import json
import os
import zipfile
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
PDF = REPO / "tests/data/pdf/sources/2206.01062.pdf"
DOCX = REPO / "tests/data/docx/sources/word_sample.docx"

pytest.importorskip("docling_rs")

from docling_rs.mineru import InvalidRequestError, MiddleJson, validate_tier  # noqa: E402
from docling_rs.mineru.parser import MinerUParser, ParseResult, parse, parse_async  # noqa: E402
from docling_rs.mineru.parser.page_range import (  # noqa: E402
    expand_page_range,
    normalize_page_range_input,
    parse_page_range,
    parse_page_range_set,
)
from docling_rs.mineru.parser.writer import FileBasedDataWriter  # noqa: E402
from docling_rs.mineru.render import HtmlRenderOptions, MarkdownRenderOptions, RenderFormat, RenderMode, render  # noqa: E402

needs_models = pytest.mark.skipif(
    not (REPO / ".models/layout_heron.onnx").exists(), reason="PDF models not downloaded"
)


@pytest.fixture(scope="module", autouse=True)
def _cwd():
    os.chdir(REPO)


@pytest.fixture(scope="module")
def paper():
    if not (REPO / ".models/layout_heron.onnx").exists():
        pytest.skip("PDF models not downloaded")
    return parse(str(PDF), tier="standard", page_range="1-2")


# ---- the SDK round trip ---------------------------------------------------


@needs_models
def test_parse_returns_markdown_and_pages(paper):
    assert isinstance(paper, ParseResult)
    assert [p.page_idx for p in paper.pages] == [0, 1]
    md = paper.markdown()
    assert "DocLayNet" in md and md.startswith("#")
    # pictures ride along as data: URIs until save() materializes them
    assert "![](data:image/png;base64," in md
    # docling.rs extension
    assert paper.document is not None and paper.document.texts


@needs_models
def test_markdown_modes(paper):
    default = paper.markdown()
    full = paper.markdown(add_markers=True)
    assert full.count("\n\n---\n\n") == 1  # one separator between the two pages
    assert "\n\n---\n\n" not in default
    # the page auxiliaries (the arXiv side note is a header block) only in FULL
    aux = [b for p in paper.pages for b in p.blocks if b["type"] in ("header", "footer", "aside_text", "page_number")]
    assert aux
    text = aux[0]["content"][0]["content"]
    assert text in full and text not in default
    assert paper.markdown(mode=RenderMode.FULL) == full


@needs_models
def test_middle_json_shape(paper):
    d = paper.to_dict()
    assert d["schema"] == "docvortex.middle" and d["schema_version"] == "2.0"
    assert d["metadata"]["file_suffix"] == "pdf" and d["metadata"]["producer"]["name"] == "docling.rs"
    assert d["metadata"]["document"]["title"].startswith("DocLayNet")
    assert d["extensions"]["mineru"] == {"tier": "standard", "parse_mode": "auto"}
    layout = d["extensions"]["docvortex_layout"]["pages"][0]
    assert layout == {"page_idx": 0, "width_pt": 612.0, "height_pt": 792.0}
    page = d["pages"][0]
    assert page["page_idx"] == 0
    indices = [b["index"] for b in page["blocks"]]
    assert indices == list(range(len(indices)))
    for block in page["blocks"]:
        x0, y0, x1, y1 = block["bbox"]
        assert 0 <= x0 < x1 <= 1 and 0 <= y0 < y1 <= 1
    kinds = {b["type"] for b in page["blocks"]}
    assert {"paragraph_title", "text", "image"} <= kinds
    image = next(b for b in page["blocks"] if b["type"] == "image")
    body = next(c for c in image["content"] if c["type"] == "image_body")
    assert body["index"] == image["index"] and body["bbox"] == image["bbox"]
    assert "image_base64" not in body  # a PDF's to_dict() leaves the bytes out, as MinerU's does
    assert any(c["type"] == "image_caption" for c in image["content"])
    title = next(b for b in page["blocks"] if b["type"] == "paragraph_title")
    assert title["level"] == 2 and title["content"] == [{"type": "text", "content": title["content"][0]["content"]}]
    lists = [b for p in d["pages"] for b in p["blocks"] if b["type"] == "list"]
    assert lists and lists[0]["sub_type"] == "text" and lists[0]["content"][0]["type"] == "text"


@needs_models
def test_json_round_trip(paper):
    restored = ParseResult.from_json(paper.to_json())
    assert restored.to_dict() == paper.to_dict()
    assert isinstance(restored.middle_json, MiddleJson)
    with pytest.raises(ValueError):
        ParseResult.from_dict({"schema": "other", "schema_version": "2.0"})


@needs_models
def test_render_targets(paper):
    v1 = render(paper.middle_json, RenderFormat.CONTENT_LIST)
    assert v1[0]["page_idx"] == 0 and all(isinstance(v, int) and 0 <= v <= 1000 for v in v1[0]["bbox"])
    title = next(i for i in v1 if i.get("text_level"))
    assert title["type"] == "text" and title["text_level"] == 2
    image = next(i for i in v1 if i["type"] == "image")
    assert image["img_path"].startswith("data:image/") and image["image_caption"] and image["image_footnote"] == []
    lst = next(i for i in v1 if i["type"] == "list")
    assert lst["sub_type"] == "text" and lst["list_items"]
    v2 = render(paper.middle_json, RenderFormat.CONTENT_LIST_V2)
    assert len(v2) == 2 and v2[0][0]["page_idx"] == 0
    assert any(i["type"] == "title" and i["content"]["level"] == 2 for i in v2[0])
    assert any(i["type"] == "paragraph" and i["content"]["paragraph_content"][0]["type"] == "text" for i in v2[0])
    sc = paper.structured_content()
    assert set(sc) == {"pages", "metadata", "extensions", "is_full_document"}
    img = next(b for b in sc["pages"][0]["blocks"] if b["type"] == "image")
    assert img["captions"] and img["image_source"].startswith("data:image/") and "index" not in img
    html = render(paper.middle_json, RenderFormat.HTML, options=HtmlRenderOptions(document_title="T"))
    assert html.startswith("<!doctype html>") and "<title>T</title>" in html and "<h2>" in html
    fragment = render(paper.middle_json, RenderFormat.HTML, options=HtmlRenderOptions(standalone=False))
    assert not fragment.startswith("<!doctype")
    md = render(paper.middle_json, RenderFormat.MARKDOWN, options=MarkdownRenderOptions(mode=RenderMode.FULL))
    assert md == paper.markdown(add_markers=True)
    with pytest.raises(NotImplementedError):
        render(paper.middle_json, RenderFormat.DOCX)


@needs_models
def test_save_layout(paper, tmp_path):
    paper.save(FileBasedDataWriter(str(tmp_path)))
    names = sorted(os.listdir(tmp_path))
    assert names == ["images", "markdown.md", "middle_json.json", "structured_content.json"]
    images = sorted(os.listdir(tmp_path / "images"))
    assert images and all(n.startswith("page_0_image_") and n.endswith(".png") for n in images)
    md = (tmp_path / "markdown.md").read_text()
    assert f"![](images/{images[0]})" in md and "data:image" not in md
    middle = json.loads((tmp_path / "middle_json.json").read_text())
    body = next(c for b in middle["pages"][0]["blocks"] if b["type"] == "image" for c in b["content"] if c["type"] == "image_body")
    assert body["image_path"] == f"images/{images[0]}"
    sc = json.loads((tmp_path / "structured_content.json").read_text())
    assert sc["pages"][0]["page_idx"] == 0
    # the in-memory result is untouched
    assert "image_base64" in json.dumps(paper.middle_json.to_dict(skip_defaults=False))


@needs_models
def test_page_range_from_the_end():
    result = parse(str(PDF), tier="flash", ocr_mode="txt", page_range="r1")
    assert [p.page_idx for p in result.pages] == [8]
    assert result.middle_json.is_full_document is False


@needs_models
def test_parser_object_and_async():
    with MinerUParser(tier="flash", parse_mode="txt") as parser:
        one = parser.parse(str(PDF), page_range="1")
        assert [p.page_idx for p in one.pages] == [0]
        assert one.to_dict()["extensions"]["mineru"] == {"tier": "flash", "parse_mode": "txt"}
    import asyncio

    result = asyncio.run(parse_async(str(PDF), tier="flash", ocr_mode="txt", page_range="1"))
    assert result.pages[0].page_idx == 0


# ---- every format, no models ----------------------------------------------------


def test_docx_at_any_tier():
    result = parse(str(DOCX), tier="flash")
    d = result.to_dict()
    assert d["metadata"]["file_suffix"] == "docx" and [p.page_idx for p in result.pages] == [0]
    assert "docvortex_layout" not in d["extensions"]
    assert all("bbox" not in b for b in d["pages"][0]["blocks"])
    md = result.markdown()
    assert "# Swimming in the lake" in md and "![](data:image/png;base64," in md
    assert "image_base64" in json.dumps(d)  # not a PDF: the bytes stay in the Middle JSON, as MinerU keeps them
    assert parse(str(DOCX), tier="standard").markdown() == md


def test_save_docx(tmp_path):
    parse(str(DOCX)).save(FileBasedDataWriter(str(tmp_path / "out")))
    assert (tmp_path / "out/markdown.md").exists()
    assert sorted(os.listdir(tmp_path / "out/images")) == ["page_0_image_3.png"]


def test_tier_and_mode_validation():
    assert validate_tier("Standard") == "standard"
    with pytest.raises(ValueError):
        validate_tier("turbo")
    with pytest.raises(ValueError):
        MinerUParser(parse_mode="fast")


def test_docling_options_pass_through():
    with pytest.raises(TypeError):
        parse(str(DOCX), docling_options={"no_such_option": 1})


# ---- page ranges and the CLI ------------------------------------------------------


def test_page_range_grammar():
    assert normalize_page_range_input("") == "" and normalize_page_range_input("all") == "all"
    assert normalize_page_range_input("3,1-2,2") == "1-3"
    assert normalize_page_range_input("r3-r1") == "r3-r1"
    assert parse_page_range("1-2,r1", 9) == [0, 1, 8]
    assert parse_page_range("all", 3) == [0, 1, 2]
    assert expand_page_range("r2-r1", 9) == "8-9"
    assert parse_page_range_set("1-3,7") == {1, 2, 3, 7}
    for bad in ("2-1", "0", "a", "1~3"):
        with pytest.raises(InvalidRequestError):
            normalize_page_range_input(bad)
    with pytest.raises(InvalidRequestError):
        parse_page_range("20", 9)


def test_cli_matches_mineru_kit(tmp_path):
    from docling_rs.mineru.__main__ import main

    out = tmp_path / "sample.md"
    assert main(["parse", str(DOCX), "-o", str(out), "--tier", "flash"]) == 0
    assert "# Swimming in the lake" in out.read_text()
    folder = tmp_path / "batch"
    assert main([str(DOCX), str(PDF.parent / "skipped_1page.pdf"), "-o", str(folder), "--format", "zip", "--tier", "flash", "--ocr-mode", "txt"]) == 0
    assert sorted(os.listdir(folder)) == ["skipped_1page.zip", "word_sample.zip"]
    with zipfile.ZipFile(folder / "word_sample.zip") as zf:
        assert "markdown.md" in zf.namelist() and "middle_json.json" in zf.namelist()
    assert main(["parse", str(DOCX), "-o", str(tmp_path / "x.json"), "--format", "middle_json"]) == 0
    assert json.loads((tmp_path / "x.json").read_text())["schema"] == "docvortex.middle"
    assert main(["parse", str(DOCX), "-o", str(tmp_path / "y.md"), "--pages", "2-1"]) == 2
