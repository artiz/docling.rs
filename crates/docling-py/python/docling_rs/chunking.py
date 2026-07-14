"""Rust-native chunkers — docling's chunking API backed by ``docling::chunker``.

The Rust port of ``docling_core.transforms.chunker`` runs the chunking; this
module mirrors docling's chunker API shape so call sites translate directly::

    from docling_rs import DocumentConverter
    from docling_rs.chunking import HierarchicalChunker, HybridChunker

    doc = DocumentConverter().convert("report.docx").document

    for chunk in HierarchicalChunker().chunk(doc):          # structure-driven
        print(chunk.meta.headings, chunk.text)

    chunker = HybridChunker(tokenizer="tokenizer.json", max_tokens=256)
    for chunk in chunker.chunk(doc):                         # tokenization-aware
        embed_me = chunker.contextualize(chunk)              # heading path + text

Differences from docling's ``docling.chunking``:

* ``HybridChunker(tokenizer=...)`` takes a **path to a HuggingFace
  ``tokenizer.json``** (e.g. ``sentence-transformers/all-MiniLM-L6-v2``'s),
  not a tokenizer object — the Rust side loads it with the ``tokenizers``
  crate, so no Python ``transformers`` install is needed.
* ``chunk.meta.doc_items`` holds the items' JSON-pointer refs (``"#/texts/12"``)
  rather than resolved item objects.

Both chunkers accept any ``docling_core.types.doc.DoclingDocument`` (or a
plain docling-JSON ``dict``/``str``). Since this package's ``result.document``
*is* a genuine ``DoclingDocument``, docling's own Python chunkers also keep
working on it — these classes are the faster, dependency-free native path.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Iterator, List, Optional

from ._native import chunk_document as _chunk_document

__all__ = ["DocMeta", "DocChunk", "HierarchicalChunker", "HybridChunker"]


@dataclass
class DocMeta:
    """Chunk metadata — the analogue of docling's ``DocMeta``."""

    #: Heading path above the chunk, outermost first; ``None`` above any heading.
    headings: Optional[List[str]] = None
    #: JSON-pointer refs of the document items the chunk was built from.
    doc_items: List[str] = field(default_factory=list)


@dataclass
class DocChunk:
    """One chunk — the analogue of docling's ``DocChunk``."""

    text: str
    meta: DocMeta
    #: The embedding-ready rendering (heading path + text), precomputed by the
    #: Rust side; read it via ``chunker.contextualize(chunk)``.
    _contextualized: str = ""


def _document_json(dl_doc: Any) -> str:
    """Accept a DoclingDocument, a docling-JSON dict, or a JSON string."""
    if isinstance(dl_doc, str):
        return dl_doc
    if isinstance(dl_doc, dict):
        return json.dumps(dl_doc)
    if hasattr(dl_doc, "export_to_dict"):
        return json.dumps(dl_doc.export_to_dict())
    raise TypeError(
        "expected a DoclingDocument, a docling-JSON dict, or a JSON string; "
        f"got {type(dl_doc).__name__}"
    )


def _run(dl_doc: Any, tokenizer: Optional[str], max_tokens: int, merge_peers: bool) -> Iterator[DocChunk]:
    records = json.loads(_chunk_document(_document_json(dl_doc), tokenizer, max_tokens, merge_peers))
    for r in records:
        yield DocChunk(
            text=r["text"],
            meta=DocMeta(headings=r["headings"], doc_items=r["doc_items"]),
            _contextualized=r["contextualize"],
        )


class _BaseChunker:
    def contextualize(self, chunk: DocChunk) -> str:
        """The text to embed: heading path + chunk body, newline-joined."""
        return chunk._contextualized


class HierarchicalChunker(_BaseChunker):
    """docling's structure-driven chunker: one chunk per document item (whole
    lists, triplet-serialized tables, picture captions), heading path as
    metadata."""

    def chunk(self, dl_doc: Any) -> Iterator[DocChunk]:
        return _run(dl_doc, None, 0, True)


class HybridChunker(_BaseChunker):
    """docling's tokenization-aware chunker: hierarchical chunks split against
    a token budget and undersized same-heading neighbours merged.

    :param tokenizer: path to a HuggingFace ``tokenizer.json``.
    :param max_tokens: token budget per chunk (docling's default for the
        MiniLM embedding model is 256).
    :param merge_peers: merge undersized peer chunks with identical headings
        (docling's default ``True``).
    """

    def __init__(self, tokenizer: str, max_tokens: int = 256, merge_peers: bool = True):
        if not isinstance(tokenizer, str):
            raise TypeError(
                "HybridChunker(tokenizer=...) takes a path to a HuggingFace "
                "tokenizer.json (docling_rs loads it natively)"
            )
        self.tokenizer = tokenizer
        self.max_tokens = max_tokens
        self.merge_peers = merge_peers

    def chunk(self, dl_doc: Any) -> Iterator[DocChunk]:
        return _run(dl_doc, self.tokenizer, self.max_tokens, self.merge_peers)
