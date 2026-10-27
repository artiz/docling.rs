"""docling_rs.agent_framework — DoclingIndexer into an Agent Framework vector
collection, DoclingContextProvider / docling_search_tool for retrieval and
DoclingConvertTool for an agent. Offline: a hashed bag-of-words embedding
client stands in for a model's, and a chat client that records what it was
sent stands in for the LLM."""

import asyncio
import hashlib
import json
import math
import re
from pathlib import Path
from unittest.mock import MagicMock

import pytest

pytest.importorskip("agent_framework")
docling_rs = pytest.importorskip("docling_rs")

from agent_framework import (  # noqa: E402
    Agent,
    ChatResponse,
    Embedding,
    GeneratedEmbeddings,
    InMemoryCollection,
    Message,
)
from docling_core.types.doc import DoclingDocument  # noqa: E402

from docling_rs import DocumentConverter  # noqa: E402
from docling_rs.agent_framework import (  # noqa: E402
    DoclingChunk,
    DoclingContextProvider,
    DoclingConvertTool,
    DoclingIndexer,
    chunk_id,
    docling_chunk_model,
    docling_search_tool,
    format_chunk,
)
from docling_rs.chunking import HierarchicalChunker  # noqa: E402

REPO = Path(__file__).resolve().parents[3]
DATA = Path(__file__).parent / "data/langchain"
DIM = 64
Chunk64 = docling_chunk_model(DIM)


class HashEmbeddings:
    """Bag of lower-cased words hashed into ``DIM`` buckets, L2-normalized:
    texts that share words are close under cosine distance."""

    additional_properties: dict = {}

    def __init__(self):
        self.calls = 0

    async def get_embeddings(self, values, *, options=None):
        self.calls += 1
        out = GeneratedEmbeddings()
        for text in values:
            v = [0.0] * DIM
            for w in re.findall(r"\w+", str(text).lower()):
                v[int(hashlib.md5(w.encode()).hexdigest(), 16) % DIM] += 1.0
            n = math.sqrt(sum(x * x for x in v)) or 1.0
            out.append(Embedding([x / n for x in v]))
        return out


class RecordingChatClient:
    """Answers "ok" and keeps the messages of every request."""

    additional_properties: dict = {}

    def __init__(self):
        self.requests = []

    def get_response(self, messages, *, stream=False, client_kwargs=None, **kwargs):
        self.requests.append(list(messages))

        async def _response():
            return ChatResponse(messages=[Message(role="assistant", contents=["ok"])], response_id="r")

        return _response()


def _mock_converter(doc):
    converter = MagicMock()
    converter.convert.return_value = MagicMock(document=doc)
    return converter


def _collection(record=Chunk64):
    return InMemoryCollection(record, embedding_generator=HashEmbeddings())


def _indexed(sources=("https://example.com/foo.pdf",)):
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    collection = _collection()
    indexer = DoclingIndexer(collection, converter=_mock_converter(doc), chunker=HierarchicalChunker(), batch_size=3)
    n = asyncio.run(indexer.index_documents(list(sources)))
    return collection, indexer, n


def test_indexer_upserts_one_record_per_chunk():
    """The same chunks the LangChain loader yields (upstream's fixture), as
    records with stable keys, page provenance and a stored vector."""
    expected = json.loads((DATA / "lc_doc_chunks_1.json").read_text())["root"]
    collection, indexer, n = _indexed()
    assert n == len(expected)
    source = "https://example.com/foo.pdf"
    records = asyncio.run(collection.get([chunk_id(source, i) for i in range(n)], include_vectors=True))
    assert len(records) == n
    for i, (rec, lc) in enumerate(zip(records, expected)):
        assert isinstance(rec, Chunk64)
        assert rec.id == chunk_id(source, i) and rec.chunk_index == i
        assert rec.text == lc["page_content"]
        assert rec.source == source
        pages = sorted({p["page_no"] for it in lc["metadata"]["dl_meta"]["doc_items"] for p in it["prov"]})
        assert rec.pages == ",".join(map(str, pages))
        headings = lc["metadata"]["dl_meta"].get("headings")
        assert rec.headings == (" > ".join(headings) if headings else "")
        assert isinstance(rec.embedding, list) and len(rec.embedding) == DIM


def test_reindexing_overwrites_instead_of_duplicating():
    collection, indexer, n = _indexed()
    asyncio.run(indexer.index_document("https://example.com/foo.pdf"))
    everything = asyncio.run(collection.get(top=1000))
    assert len(everything) == n
    # batch_size=3 → one embedding request per batch, both runs.
    assert collection.embedding_generator.calls == 2 * math.ceil(n / 3)


def test_search_finds_the_chunk_that_shares_the_words():
    collection, _, _ = _indexed()
    records = asyncio.run(collection.get(top=1000))
    target = max(records, key=lambda r: len(r.text))
    hits = asyncio.run(DoclingContextProvider(collection, top=1).retrieve(target.text))
    assert [h.id for h in hits] == [target.id]


def test_context_provider_adds_cited_excerpts_to_the_run():
    """End to end through Agent.run: the request the model gets carries the
    retrieved chunk under its citation line, before the user's message."""
    collection, _, _ = _indexed()
    records = asyncio.run(collection.get(top=1000))
    target = max(records, key=lambda r: len(r.text))
    client = RecordingChatClient()
    agent = Agent(client=client, context_providers=[DoclingContextProvider(collection, top=2)])
    response = asyncio.run(agent.run(target.text))
    assert response.text == "ok"
    sent = "\n".join(m.text or "" for m in client.requests[0])
    assert format_chunk(target) in sent
    assert "Cite the pages you use." in sent
    # Nothing indexed matches → nothing added.
    empty = _collection()
    asyncio.run(empty.ensure_collection_exists())
    client2 = RecordingChatClient()
    asyncio.run(Agent(client=client2, context_providers=[DoclingContextProvider(empty)]).run("hello"))
    assert [m.text for m in client2.requests[0]] == ["hello"]


def test_search_tool_answers_with_citations():
    collection, _, _ = _indexed()
    records = asyncio.run(collection.get(top=1000))
    target = max(records, key=lambda r: len(r.text))
    tool = docling_search_tool(collection, top=1)
    assert tool.name == "search_documents"
    assert "query" in tool.parameters()["properties"]
    out = asyncio.run(tool.invoke(arguments={"query": target.text}))
    text = "\n".join(c.text for c in out) if isinstance(out, list) else str(out)
    assert format_chunk(target) in text


def test_format_chunk():
    rec = Chunk64(id="k", text="body", source="a.pdf", headings="Intro > Scope", pages="3,4")
    assert format_chunk(rec) == "[a.pdf, pp. 3,4 — Intro > Scope]\nbody"
    assert format_chunk({"text": "body", "source": "b.md"}) == "[b.md]\nbody"
    assert format_chunk({"text": "t", "source": "c.pdf", "pages": "7"}) == "[c.pdf, p. 7]\nt"


def test_default_model_and_keys():
    rec = DoclingChunk(id="k", text="t", source="s")
    assert rec.embedding is None and rec.pages == "" and rec.chunk_index == 0
    assert InMemoryCollection(DoclingChunk).collection_name == "docling_chunks"
    assert chunk_id("a.pdf", 0) == chunk_id("a.pdf", 0) != chunk_id("a.pdf", 1)
    import uuid

    uuid.UUID(chunk_id("a.pdf", 0))  # a valid UUID: Qdrant-style stores accept it


def test_custom_record_and_metadata():
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    seen = []
    indexer = DoclingIndexer(
        MagicMock(),
        converter=_mock_converter(doc),
        chunker=HierarchicalChunker(),
        metadata=lambda source, chunk: {"source": source, "n_items": len(chunk.meta.doc_items)},
        record=lambda fields: seen.append(fields) or fields,
    )
    out = list(indexer.records("foo.pdf"))
    assert out == seen and out[0]["n_items"] >= 1
    assert out[0]["embedding"] == out[0]["text"]
    assert set(out[0]) >= {"id", "text", "embedding", "chunk_index", "source", "headings", "pages"}


def test_convert_kwargs_reach_the_backend():
    doc = DoclingDocument.load_from_json(DATA / "dl_doc_1.json")
    converter = _mock_converter(doc)
    options = object()
    indexer = DoclingIndexer(
        _collection(), converter=converter, convert_kwargs={"options": options}, chunker=HierarchicalChunker()
    )
    list(indexer.records("foo.pdf"))
    converter.convert.assert_called_once_with(source="foo.pdf", options=options)


def test_indexer_on_a_real_file():
    path = REPO / "tests/data/md/sources/mixed.md"
    if not path.exists():
        pytest.skip("fixture missing")
    collection = _collection()
    n = asyncio.run(DoclingIndexer(collection, chunker=HierarchicalChunker()).index_document(str(path)))
    records = asyncio.run(collection.get(top=1000))
    assert n == len(records) > 1
    assert all(r.source == str(path) and r.pages == "" for r in records)  # Markdown has no pages


def test_tool_schema_and_truncation():
    doc = DocumentConverter().convert(str(REPO / "crates/docling/sample.html")).document
    tool = DoclingConvertTool(converter=_mock_converter(doc), max_chars=40)
    assert tool.name == "convert_document"
    assert tool.approval_mode == "never_require"
    params = tool.parameters()
    assert list(params["properties"]) == ["source"]
    assert params["required"] == ["source"]
    full = doc.export_to_markdown(image_placeholder="")
    assert len(full) > 40
    out = asyncio.run(tool.invoke(arguments={"source": "foo.pdf"}))
    text = "\n".join(c.text for c in out) if isinstance(out, list) else str(out)
    assert text.startswith(full[:40])
    assert text.endswith(f"[… {len(full) - 40} more characters]")
    assert DoclingConvertTool(converter=_mock_converter(doc), max_chars=None).convert("foo.pdf") == full
    assert DoclingConvertTool(converter=_mock_converter(doc), approval_mode="always_require").approval_mode == (
        "always_require"
    )


def test_tool_reads_a_real_document():
    path = REPO / "crates/docling/sample.html"
    if not path.exists():
        pytest.skip("fixture missing")
    tool = DoclingConvertTool(DocumentConverter(), name="read_document", description="Read it.")
    assert tool.name == "read_document"
    out = asyncio.run(tool.invoke(arguments={"source": str(path)}))
    md = "\n".join(c.text for c in out) if isinstance(out, list) else str(out)
    assert md.strip() == DocumentConverter().convert(str(path)).document.export_to_markdown(image_placeholder="").strip()
    assert len(md) > 50
