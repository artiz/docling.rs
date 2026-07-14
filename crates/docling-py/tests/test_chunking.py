"""Tests for ``docling_rs.chunking`` — the Rust-native HierarchicalChunker /
HybridChunker exposed to Python. Declarative path only, no ML models; the
hybrid tests use the MiniLM ``tokenizer.json`` checked in for the chunking
conformance suite."""

from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
TOKENIZER = REPO / "tests/data/chunks/tokenizer.json"

docling_rs = pytest.importorskip("docling_rs")

from docling_rs.chunking import DocChunk, HierarchicalChunker, HybridChunker  # noqa: E402

MD = b"# Guide\n\n## Setup\n\nInstall the tools.\n\n- clone\n- build\n\n## Usage\n\nRun it.\n"


def _document():
    return docling_rs.DocumentConverter().convert_bytes("guide.md", MD).document


def test_hierarchical_chunks_carry_headings_and_doc_items():
    chunks = list(HierarchicalChunker().chunk(_document()))
    assert len(chunks) >= 3
    setup = next(c for c in chunks if "Install" in c.text)
    assert isinstance(setup, DocChunk)
    assert setup.meta.headings == ["Guide", "Setup"]
    assert setup.meta.doc_items and setup.meta.doc_items[0].startswith("#/")
    # Lists stay whole: one chunk for both items.
    assert any(c.text == "- clone\n- build" for c in chunks)


def test_contextualize_prefixes_the_heading_path():
    chunker = HierarchicalChunker()
    setup = next(c for c in chunker.chunk(_document()) if "Install" in c.text)
    assert chunker.contextualize(setup) == "Guide\nSetup\nInstall the tools."


def test_chunk_accepts_dict_and_json_string():
    doc = _document()
    from_doc = [c.text for c in HierarchicalChunker().chunk(doc)]
    from_dict = [c.text for c in HierarchicalChunker().chunk(doc.export_to_dict())]
    import json

    from_str = [c.text for c in HierarchicalChunker().chunk(json.dumps(doc.export_to_dict()))]
    assert from_doc == from_dict == from_str


@pytest.mark.skipif(not TOKENIZER.exists(), reason="MiniLM tokenizer.json not checked out")
def test_hybrid_splits_against_the_token_budget():
    long_md = ("# Doc\n\n" + " ".join(f"Sentence number {i} padding words here." for i in range(40))).encode()
    doc = docling_rs.DocumentConverter().convert_bytes("l.md", long_md).document
    hier = list(HierarchicalChunker().chunk(doc))
    chunker = HybridChunker(tokenizer=str(TOKENIZER), max_tokens=64)
    hybrid = list(chunker.chunk(doc))
    assert len(hybrid) > len(hier)
    assert all(c.meta.headings == ["Doc"] for c in hybrid)
    assert chunker.contextualize(hybrid[0]).startswith("Doc\n")


def test_hybrid_requires_a_tokenizer_path():
    with pytest.raises(TypeError):
        HybridChunker(tokenizer=None)


def test_bad_tokenizer_path_raises_conversion_error():
    chunker = HybridChunker(tokenizer="/nonexistent/tokenizer.json")
    with pytest.raises(docling_rs.ConversionError):
        list(chunker.chunk(_document()))
