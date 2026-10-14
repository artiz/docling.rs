"""docling_rs.autogen — DoclingIndexer into an AutoGen memory and
DoclingConvertTool for an agent."""

import asyncio
import json
from pathlib import Path
from unittest.mock import MagicMock

import pytest

pytest.importorskip("autogen_core")
docling_rs = pytest.importorskip("docling_rs")

from autogen_core import CancellationToken  # noqa: E402
from autogen_core.memory import ListMemory, MemoryMimeType  # noqa: E402
from docling_core.types.doc import DoclingDocument  # noqa: E402

from docling_rs import DocumentConverter  # noqa: E402
from docling_rs.autogen import DoclingConvertTool, DoclingIndexer, chunk_metadata  # noqa: E402
from docling_rs.chunking import HierarchicalChunker  # noqa: E402

REPO = Path(__file__).resolve().parents[3]
DATA = Path(__file__).parent / "data/langchain"


def _mock_converter(doc):
    converter = MagicMock()
    converter.convert.return_value = MagicMock(document=doc)
    return converter


def test_indexer_adds_one_memory_item_per_chunk():
    """The same chunks the LangChain loader yields (upstream's fixture), as
    MemoryContent with flat metadata."""
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    expected = json.loads((DATA / "lc_doc_chunks_1.json").read_text())["root"]
    memory = ListMemory()
    indexer = DoclingIndexer(
        memory, converter=_mock_converter(doc), chunker=HierarchicalChunker()
    )
    n = asyncio.run(indexer.index_documents(["https://example.com/foo.pdf"]))
    assert n == len(expected) == len(memory.content)
    for item, lc in zip(memory.content, expected):
        assert item.content == lc["page_content"]
        assert item.mime_type == MemoryMimeType.TEXT
        assert item.metadata["source"] == "https://example.com/foo.pdf"
        pages = sorted({p["page_no"] for it in lc["metadata"]["dl_meta"]["doc_items"] for p in it["prov"]})
        assert item.metadata.get("pages") == (",".join(map(str, pages)) if pages else None)
        headings = lc["metadata"]["dl_meta"].get("headings")
        assert item.metadata.get("headings") == (" > ".join(headings) if headings else None)
        assert all(isinstance(v, (str, int, float, bool)) for v in item.metadata.values())


def test_indexer_custom_metadata_and_mime_type():
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    memory = ListMemory()
    indexer = DoclingIndexer(
        memory,
        converter=_mock_converter(doc),
        chunker=HierarchicalChunker(),
        mime_type=MemoryMimeType.MARKDOWN,
        metadata=lambda source, chunk: {"doc": source, "n_items": len(chunk.meta.doc_items)},
    )
    asyncio.run(indexer.index_document("foo.pdf"))
    assert memory.content[0].mime_type == MemoryMimeType.MARKDOWN
    assert memory.content[0].metadata["doc"] == "foo.pdf"
    assert memory.content[0].metadata["n_items"] >= 1


def test_convert_kwargs_reach_the_backend():
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    converter = _mock_converter(doc)
    options = object()
    indexer = DoclingIndexer(
        ListMemory(), converter=converter, convert_kwargs={"options": options}, chunker=HierarchicalChunker()
    )
    list(indexer.chunks("foo.pdf"))
    converter.convert.assert_called_once_with(source="foo.pdf", options=options)


def test_indexer_on_a_real_file():
    path = REPO / "tests/data/md/sources/mixed.md"
    if not path.exists():
        pytest.skip("fixture missing")
    memory = ListMemory()
    n = asyncio.run(DoclingIndexer(memory, chunker=HierarchicalChunker()).index_document(str(path)))
    assert n == len(memory.content) > 1
    assert all(c.metadata["source"] == str(path) for c in memory.content)
    assert "pages" not in memory.content[0].metadata  # Markdown has no pages
    text = path.read_text(encoding="utf-8")
    assert any(w in text for w in memory.content[0].content.split()[:3])


def test_indexer_pages_come_from_item_provenance():
    """A converted PDF's chunks carry the page numbers of their items."""
    path = REPO / "tests/data/pdf/sources/multi_page.pdf"
    if not path.exists():
        pytest.skip("fixture missing")
    memory = ListMemory()
    converter = DocumentConverter()
    try:
        asyncio.run(DoclingIndexer(memory, converter=converter, chunker=HierarchicalChunker()).index_document(str(path)))
    except docling_rs.ConversionError as e:
        if "model not found" in str(e):
            pytest.skip("ML models not fetched")
        raise
    pages = {int(p) for c in memory.content for p in c.metadata["pages"].split(",")}
    assert len(pages) > 1 and min(pages) == 1
    assert all(c.metadata["pages"] == ",".join(sorted(c.metadata["pages"].split(","), key=int)) for c in memory.content)


def test_tool_schema_and_truncation():
    doc = DocumentConverter().convert(str(REPO / "crates/docling/sample.html")).document
    tool = DoclingConvertTool(converter=_mock_converter(doc), max_chars=40)
    assert tool.name == "convert_document"
    params = tool.schema["parameters"]
    assert list(params["properties"]) == ["source"]
    assert params["required"] == ["source"]
    full = doc.export_to_markdown(image_placeholder="")
    assert len(full) > 40
    out = asyncio.run(tool.run_json({"source": "foo.pdf"}, CancellationToken()))
    assert out.startswith(full[:40])
    assert out.endswith(f"[… {len(full) - 40} more characters]")
    assert DoclingConvertTool(converter=_mock_converter(doc), max_chars=None).convert("foo.pdf") == full


def test_tool_reads_a_real_document():
    path = REPO / "crates/docling/sample.html"
    if not path.exists():
        pytest.skip("fixture missing")
    tool = DoclingConvertTool(DocumentConverter(), name="read_document", description="Read it.")
    assert tool.name == "read_document"
    md = asyncio.run(tool.run_json({"source": str(path)}, CancellationToken()))
    assert md.strip() == DocumentConverter().convert(str(path)).document.export_to_markdown(image_placeholder="").strip()
    assert len(md) > 50


def test_chunk_metadata_without_pages_or_headings():
    chunk = MagicMock()
    chunk.meta.headings = None
    chunk.meta.doc_items = ["#/texts/0"]  # unresolved refs: no provenance
    assert chunk_metadata("a.md", chunk) == {"source": "a.md"}
