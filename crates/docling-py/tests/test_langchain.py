"""docling_rs.langchain — the port of docling-project/docling-langchain.

The first two tests are the upstream package's own (test/test_loader.py), run
against this loader with upstream's fixtures (tests/data/langchain/), so the
LangChain output is checked to be *identical* to langchain-docling's."""

import functools
import http.server
import json
import threading
import warnings
from pathlib import Path
from unittest.mock import MagicMock

import pytest

pytest.importorskip("langchain_core")
docling_rs = pytest.importorskip("docling_rs")

from docling_core.types.doc import DoclingDocument  # noqa: E402
from docling_core.types.doc.document import RefItem  # noqa: E402

from docling_rs import DocumentConverter, InputFormat, PdfFormatOption, PdfPipelineOptions  # noqa: E402
from docling_rs.chunking import HierarchicalChunker, WindowChunker  # noqa: E402
from docling_rs.langchain import (  # noqa: E402
    DoclingLoader,
    ExportType,
    PictureDescriptionLangChainOptions,
)

REPO = Path(__file__).resolve().parents[3]
DATA = Path(__file__).parent / "data/langchain"


def _mock_converter(doc):
    converter = MagicMock()
    converter.convert.return_value = MagicMock(document=doc)
    return converter


def _dump(lc_docs):
    return {"root": [d.model_dump() for d in lc_docs]}


def test_load_as_markdown_matches_upstream():
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    loader = DoclingLoader(
        file_path="https://example.com/foo.pdf",
        converter=_mock_converter(doc),
        export_type=ExportType.MARKDOWN,
    )
    docs = list(loader.lazy_load())
    assert len(docs) == 1
    assert _dump(docs) == json.loads((DATA / "lc_doc_md_1.json").read_text())


def test_load_as_doc_chunks_matches_upstream():
    """Native chunker + ref resolution reproduce docling's chunk text and
    ``dl_meta`` (full items, origin) exactly."""
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    loader = DoclingLoader(
        file_path="https://example.com/foo.pdf",
        converter=_mock_converter(doc),
        export_type=ExportType.DOC_CHUNKS,
        chunker=HierarchicalChunker(),
    )
    docs = list(loader.lazy_load())
    assert len(docs) == 2
    assert _dump(docs) == json.loads((DATA / "lc_doc_chunks_1.json").read_text())


def test_convert_kwargs_reach_the_backend():
    """upstream's service-client test, minus the docling dependency: any
    backend with ``convert(source=..., **kwargs)`` works."""
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    converter = _mock_converter(doc)
    options = object()
    docs = DoclingLoader(
        file_path="https://example.com/foo.pdf",
        converter=converter,
        convert_kwargs={"options": options},
        chunker=HierarchicalChunker(),
    ).load()
    converter.convert.assert_called_once_with(source="https://example.com/foo.pdf", options=options)
    assert len(docs) == 2
    assert all(d.metadata["source"] == "https://example.com/foo.pdf" for d in docs)
    assert all("dl_meta" in d.metadata for d in docs)


@pytest.mark.parametrize(
    "fixture",
    [
        "tests/data/html/sources/hyperlink_06.html",
        "tests/data/html/sources/formatting.html",
        "tests/data/docx/sources/lorem_ipsum.docx",
        "tests/data/docx/sources/docx_lists.docx",
        "tests/data/md/sources/mixed.md",
    ],
)
def test_chunk_items_point_at_their_own_text(fixture):
    """Every ``dl_meta`` text item resolves to text that is in the chunk —
    the native chunker numbers the *re-imported* document, which drifts from
    the converted one (empty paragraphs, HTML/DOCX item trees, inline runs);
    chunking.py maps the items back."""
    path = REPO / fixture
    if not path.exists():
        pytest.skip(f"{fixture} missing")
    doc = DocumentConverter().convert(path).document
    checked = 0
    for chunk in HierarchicalChunker().chunk(doc):
        for ref in chunk.meta.doc_items:
            item = RefItem(cref=ref).resolve(doc)
            text = getattr(item, "text", "")
            if text.strip():
                assert all(w in chunk.text for w in text.split()[:3]), (ref, text, chunk.text)
                checked += 1
    assert checked > 0


def test_inline_runs_map_to_distinct_items():
    """An inline paragraph (``Jump to [Section 2](…) or visit …``) is one
    chunk whose items are its runs, as docling's chunker reports them — not
    one item repeated."""
    path = REPO / "tests/data/html/sources/hyperlink_06.html"
    doc = DocumentConverter().convert(path).document
    first = next(c for c in HierarchicalChunker().chunk(doc) if c.text.startswith("Jump to"))
    texts = [RefItem(cref=r).resolve(doc).text for r in first.meta.doc_items]
    assert texts[:3] == ["Jump to", "Section 2", "or visit"]
    assert len(set(first.meta.doc_items)) == len(first.meta.doc_items)


def test_default_chunker_resolves_its_tokenizer_on_first_load(monkeypatch):
    """Constructing the loader stays offline; the first DOC_CHUNKS load asks
    for the tokenizer with ``fetch=True`` (docling pulls its tokenizer from
    the Hub the same way)."""
    tok = REPO / ".models/chunk/tokenizer.json"
    if not tok.exists():
        pytest.skip(".models/chunk/tokenizer.json missing")
    from docling_rs import models

    calls = []

    def fake(fetch=False, progress=False):
        calls.append(fetch)
        return tok

    monkeypatch.setattr(models, "chunk_tokenizer", fake)
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    loader = DoclingLoader(file_path="x.html", converter=_mock_converter(doc))
    assert calls == []
    docs = loader.load()
    assert calls == [True]
    # undersized peers merge (docling's delimiter is one newline); the merged
    # chunk lists both items
    assert [d.page_content for d in docs] == ["Some text\nAnother paragraph"]
    refs = [it["self_ref"] for it in docs[0].metadata["dl_meta"]["doc_items"]]
    assert refs == ["#/texts/0", "#/texts/1"]


def test_window_chunks_keep_their_headings_meta():
    doc = DocumentConverter().convert(REPO / "tests/data/md/sources/mixed.md").document
    conv = _mock_converter(doc)
    docs = DoclingLoader(file_path="mixed.md", converter=conv, chunker=WindowChunker(max_words=50)).load()
    assert docs
    meta = docs[0].metadata["dl_meta"]
    assert meta["schema_name"] == "docling_core.transforms.chunker.DocMeta"
    assert meta["doc_items"] == []


def test_docling_chunkers_plug_in_unchanged():
    """docling-core's own chunker works too (the document is docling-core's)."""
    try:
        from docling_core.transforms.chunker import HierarchicalChunker as PyChunker
    except ImportError:
        pytest.skip("docling-core[chunking] not installed")
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    docs = DoclingLoader(
        file_path="https://example.com/foo.pdf", converter=_mock_converter(doc), chunker=PyChunker()
    ).load()
    assert _dump(docs) == json.loads((DATA / "lc_doc_chunks_1.json").read_text())


# --- URL sources -------------------------------------------------------------


@pytest.fixture
def http_server():
    """Serves the HTML fixture at an extension-less path (like arXiv's
    /pdf/<id>), so the converter must take the format from Content-Type."""
    body = (REPO / "tests/data/html/sources/hyperlink_06.html").read_bytes()

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}/docs/anchors"
    server.shutdown()


def test_convert_downloads_urls(http_server):
    result = DocumentConverter().convert(http_server)
    assert result.input.file.name == "anchors.html"
    assert "Content for section 2" in result.document.export_to_markdown()


def test_loader_takes_urls(http_server):
    docs = DoclingLoader(file_path=[http_server], export_type=ExportType.MARKDOWN).load()
    assert docs[0].metadata == {"source": http_server}
    assert "Jump to" in docs[0].page_content


# --- picture description -----------------------------------------------------


class _RecordingChatModel:
    """Duck-typed chat model: records the batch, answers per picture."""

    def __init__(self):
        self.batches = []

    def batch(self, messages):
        self.batches.append(messages)
        return [MagicMock(text=f"picture {i}") for i in range(len(messages))]


def _converter_with(llm, **kw):
    opts = PdfPipelineOptions(do_picture_description=True)
    opts.picture_description_options = PictureDescriptionLangChainOptions(llm=llm, **kw)
    return DocumentConverter(format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)})


def test_picture_description_with_a_chat_model():
    llm = _RecordingChatModel()
    conv = _converter_with(llm, prompt="Describe.", provenance="test-model", batch_size=2)
    doc = conv.convert(REPO / "tests/data/docx/sources/word_image_anchors.docx").document
    pics = [p for p in doc.pictures if p.image is not None]
    assert pics
    # batches of two, upstream's message shape: prompt + base64 PNG image_url
    assert [len(b) for b in llm.batches] == [2] * (len(pics) // 2) + ([1] if len(pics) % 2 else [])
    content = llm.batches[0][0][0]["content"]
    assert content[0] == {"type": "text", "text": "Describe."}
    assert content[1]["image_url"]["url"].startswith("data:image/png;base64,")
    for i, pic in enumerate(pics):
        assert pic.meta.description.text == f"picture {i % 2}"
        assert pic.meta.description.created_by == "langchain-test-model"


def test_picture_description_with_langchain_fake_model():
    from langchain_core.language_models.fake_chat_models import FakeListChatModel

    llm = FakeListChatModel(responses=["a chart"])
    doc = _converter_with(llm).convert(REPO / "tests/data/docx/sources/word_image_anchors.docx").document
    described = [p for p in doc.pictures if p.meta and p.meta.description]
    assert described and all(p.meta.description.text == "a chart" for p in described)
    assert described[0].meta.description.created_by == "langchain"


def test_picture_description_skips_small_pictures_and_needs_options():
    from docling_rs.picture_description import describe_pictures

    llm = _RecordingChatModel()
    doc = DocumentConverter().convert(REPO / "tests/data/docx/sources/word_image_anchors.docx").document
    opts = PictureDescriptionLangChainOptions(llm=llm)
    # no page provenance (DOCX) → every picture with an image is described
    assert describe_pictures(doc, opts) == sum(p.image is not None for p in doc.pictures)

    with pytest.raises(ValueError):
        PictureDescriptionLangChainOptions()
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        DocumentConverter(
            format_options={
                InputFormat.PDF: PdfFormatOption(
                    pipeline_options=PdfPipelineOptions(do_picture_description=True)
                )
            }
        )
    assert any("picture_description_options" in str(x.message) for x in w)


def test_langchain_docling_rs_package_reexports_the_loader():
    """The `langchain-docling-rs` distribution (crates/docling-py/langchain) is
    a re-export of this module under LangChain's package naming: the same
    objects, the public names `langchain_docling` has."""
    import importlib
    import sys

    src = Path(__file__).resolve().parents[1] / "langchain" / "src"
    sys.path.insert(0, str(src))
    try:
        shim = importlib.import_module("langchain_docling_rs")
    finally:
        sys.path.remove(str(src))
    import docling_rs.langchain as native

    assert shim.DoclingLoader is native.DoclingLoader
    assert shim.ExportType is native.ExportType
    assert shim.PictureDescriptionLangChainOptions is native.PictureDescriptionLangChainOptions
    assert set(shim.__all__) == set(native.__all__)
    for name in ("DoclingLoader", "ExportType", "MetaExtractor", "BaseMetaExtractor"):
        assert name in shim.__all__
