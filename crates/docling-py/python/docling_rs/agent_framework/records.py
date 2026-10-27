"""The vector-store record a document chunk becomes.

No ``from __future__ import annotations`` here: the ``Annotated`` field
metadata of :func:`docling_chunk_model`'s class has to be evaluated where
``dimensions`` is in scope, which string annotations would defer past it.
"""

import uuid
import warnings
from dataclasses import dataclass
from typing import Annotated, Any, List, Optional, Union

try:
    from agent_framework import VectorStoreField, vectorstoremodel
except ImportError as e:  # pragma: no cover - exercised only without the extra
    raise ImportError(
        "docling_rs.agent_framework needs agent-framework-core: "
        "pip install 'docling-rs[agent-framework]'"
    ) from e

#: The collection :data:`DoclingChunk` names by default.
DEFAULT_COLLECTION = "docling_chunks"


def docling_chunk_model(
    dimensions: int = 1536,
    *,
    collection_name: str = DEFAULT_COLLECTION,
    distance_function: Optional[Any] = None,
    index_kind: Optional[Any] = None,
) -> type:
    """A ``@vectorstoremodel`` dataclass for document chunks, its vector
    field sized for your embedding model — :data:`DoclingChunk` is the one
    for 1536-dimensional embeddings (OpenAI's ``text-embedding-3-small``).

    Fields: ``id`` (the key — a UUID derived from the source and the chunk's
    position, so indexing a document again overwrites its chunks instead of
    duplicating them, and stores that want UUID keys, Qdrant's, accept it),
    ``text`` (the heading path plus the chunk text, as the chunker
    contextualizes it; full-text indexed for hybrid search), ``source``
    (filterable), ``headings`` (``A > B``), ``pages`` (``3,4``; empty for
    inputs without pages), ``chunk_index`` and ``embedding`` — the vector,
    generated from ``text`` by the collection's embedding client on upsert.

    :param dimensions: the length of your embedding model's vectors.
    :param collection_name: the collection the model names by default.
    :param distance_function: / :param index_kind: passed to the vector
        field (``None`` = the store's defaults; cosine for the in-memory one).
    """

    @vectorstoremodel(collection_name=collection_name)
    @dataclass
    class DoclingChunk:
        id: Annotated[str, VectorStoreField("key")]
        text: Annotated[str, VectorStoreField("data", is_full_text_indexed=True)]
        source: Annotated[str, VectorStoreField("data", is_indexed=True)]
        headings: Annotated[str, VectorStoreField("data")] = ""
        pages: Annotated[str, VectorStoreField("data")] = ""
        chunk_index: Annotated[int, VectorStoreField("data")] = 0
        # The source text on upsert (the collection embeds it), the vector
        # once stored.
        embedding: Annotated[
            Union[List[float], str, None],
            VectorStoreField(
                "vector",
                dimensions=dimensions,
                distance_function=distance_function,
                index_kind=index_kind,
            ),
        ] = None

    DoclingChunk.__qualname__ = DoclingChunk.__name__
    return DoclingChunk


# Agent Framework flags its vector-store API experimental and warns on every
# use. Defining the default model at import would warn for anyone importing
# the package — the convert tool's users included; using a collection still
# warns, which is where the experimental API is actually used.
with warnings.catch_warnings():
    warnings.filterwarnings("ignore", message=r"\[VECTOR_STORES\]")
    #: The chunk record for 1536-dimensional embeddings; see
    #: :func:`docling_chunk_model` for other sizes.
    DoclingChunk = docling_chunk_model()


def chunk_id(source: str, index: int) -> str:
    """The key of chunk ``index`` of ``source``: stable across runs, so a
    re-index upserts in place."""
    return str(uuid.uuid5(uuid.NAMESPACE_URL, f"{source}#{index}"))


def format_chunk(record: Any) -> str:
    """A retrieved chunk as the model reads it — a citation line (source,
    pages, heading path) over its text. Works on :data:`DoclingChunk` records
    and on mappings with the same fields."""
    get = record.get if isinstance(record, dict) else lambda k, d=None: getattr(record, k, d)
    cite = str(get("source", "") or "")
    pages = get("pages", "")
    if pages:
        cite += f", p. {pages}" if "," not in str(pages) else f", pp. {pages}"
    headings = get("headings", "")
    if headings:
        cite += f" — {headings}"
    return f"[{cite}]\n{get('text', '')}"
