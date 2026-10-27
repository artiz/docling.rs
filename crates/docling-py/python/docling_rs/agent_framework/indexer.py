"""Documents into an Agent Framework vector collection: docling.rs
conversion, docling's chunking, one record per chunk."""

from __future__ import annotations

import asyncio
from typing import Any, Callable, Dict, Iterable, Iterator, List, Optional

from .. import models
from .._chunk_meta import chunk_metadata, docling_chunk
from ..chunking import HybridChunker
from .records import chunk_id

MetadataFn = Callable[[str, Any], Dict[str, Any]]
RecordFn = Callable[[Dict[str, Any]], Any]


class DoclingIndexer:
    """Convert, chunk and upsert into a vector collection.

    :param collection: any Agent Framework vector collection —
        ``InMemoryCollection``, or a connector's (Azure AI Search, Redis,
        Qdrant, Postgres, …) — whose records are :data:`DoclingChunk` (or a
        :func:`docling_chunk_model` of your embedding size), with an
        ``embedding_generator`` to embed them on upsert.
    :param converter: a :class:`docling_rs.DocumentConverter` configured as
        you like, docling's own converter, or anything whose
        ``convert(source=…)`` returns a result with a ``.document``; a plain
        ``DocumentConverter()`` by default.
    :param convert_kwargs: extra keyword arguments for ``converter.convert``.
    :param chunker: anything with ``chunk(dl_doc)`` and
        ``contextualize(chunk=…)``; the native ``HybridChunker()`` with
        docling's defaults by default (its tokenizer is fetched into the model
        cache on first use, not at construction).
    :param metadata: ``(source, chunk) -> dict`` — :func:`chunk_metadata` by
        default (``source``, ``headings``, ``pages``).
    :param record: ``fields -> record`` for a record type of your own; the
        fields are ``id``, ``text``, ``embedding`` (the text to embed),
        ``chunk_index`` and whatever ``metadata`` returned. By default the
        collection's record type is built from them.
    :param batch_size: records per ``upsert`` call (one embedding request
        each).
    """

    def __init__(
        self,
        collection: Any,
        *,
        converter: Optional[Any] = None,
        convert_kwargs: Optional[Dict[str, Any]] = None,
        chunker: Optional[Any] = None,
        metadata: MetadataFn = chunk_metadata,
        record: Optional[RecordFn] = None,
        batch_size: int = 64,
    ):
        self._collection = collection
        if converter is None:
            from .. import DocumentConverter

            converter = DocumentConverter()
        self._converter = converter
        self._convert_kwargs = convert_kwargs or {}
        self._default_chunker = chunker is None
        self._chunker = chunker or HybridChunker()
        self._metadata = metadata
        self._record = record or self._default_record
        self._batch_size = max(1, batch_size)
        self._collection_ready = False

    @property
    def collection(self) -> Any:
        return self._collection

    def _default_record(self, fields: Dict[str, Any]) -> Any:
        record_type = getattr(self._collection, "record_type", None)
        if record_type is None or record_type is dict:
            return fields
        return record_type(**fields)

    def _ensure_tokenizer(self) -> None:
        if self._default_chunker and self._chunker.tokenizer is None:
            path = models.chunk_tokenizer(fetch=True)
            if path is not None:
                self._chunker.tokenizer = str(path)
            self._default_chunker = False

    def records(self, source: str) -> Iterator[Any]:
        """Convert ``source`` (a path or URL) and yield its chunks as records
        — without upserting them, for batching into a store yourself."""
        self._ensure_tokenizer()
        dl_doc = self._converter.convert(source=source, **self._convert_kwargs).document
        for i, chunk in enumerate(self._chunker.chunk(dl_doc)):
            text = self._chunker.contextualize(chunk=chunk)
            fields: Dict[str, Any] = {"source": source, "headings": "", "pages": ""}
            fields.update(self._metadata(source, docling_chunk(chunk, dl_doc)))
            fields.update(id=chunk_id(source, i), text=text, embedding=text, chunk_index=i)
            yield self._record(fields)

    async def index_document(self, source: str) -> int:
        """Convert, chunk and upsert one document; returns the number of
        chunks. The conversion runs on a worker thread, so the event loop —
        an agent serving other sessions — is not blocked by it."""
        records: List[Any] = await asyncio.to_thread(lambda: list(self.records(source)))
        if not self._collection_ready:
            await self._collection.ensure_collection_exists()
            self._collection_ready = True
        for start in range(0, len(records), self._batch_size):
            await self._collection.upsert(records[start : start + self._batch_size])
        return len(records)

    async def index_documents(self, sources: Iterable[str]) -> int:
        """:meth:`index_document` for each source; returns the total."""
        n = 0
        for source in sources:
            n += await self.index_document(source)
        return n
