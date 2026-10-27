"""Documents into an AutoGen memory: docling.rs conversion, docling's chunking,
one ``MemoryContent`` per chunk."""

from __future__ import annotations

from typing import Any, Callable, Dict, Iterable, Iterator, Optional, Union

try:
    from autogen_core.memory import Memory, MemoryContent, MemoryMimeType
except ImportError as e:  # pragma: no cover - exercised only without the extra
    raise ImportError(
        "docling_rs.autogen needs autogen-core: pip install 'docling-rs[autogen]'"
    ) from e

from .. import models
from .._chunk_meta import chunk_metadata, docling_chunk
from ..chunking import HybridChunker

MetadataFn = Callable[[str, Any], Dict[str, Any]]


class DoclingIndexer:
    """Convert, chunk and add to a memory.

    :param memory: any ``autogen_core.memory.Memory`` — ``ListMemory`` (every
        chunk joins the context), ``ChromaDBVectorMemory`` and the other
        autogen-ext stores (the relevant chunks are retrieved per turn).
    :param converter: a :class:`docling_rs.DocumentConverter` configured as
        you like (``do_ocr=False``, ``ocr_lang=…``), docling's own converter,
        or anything whose ``convert(source=…)`` returns a result with a
        ``.document``. Defaults to a plain ``docling_rs.DocumentConverter()``.
    :param convert_kwargs: extra keyword arguments for ``converter.convert``.
    :param chunker: anything with ``chunk(dl_doc)`` and
        ``contextualize(chunk=…)`` — :mod:`docling_rs.chunking`'s chunkers or
        docling's. Defaults to the native ``HybridChunker()`` with docling's
        defaults (all-MiniLM-L6-v2 tokenizer, 256 tokens); its
        ``tokenizer.json`` is fetched into the model cache on first use.
    :param mime_type: the ``MemoryContent`` MIME type, ``text/plain`` by
        default (``MemoryMimeType.MARKDOWN`` if your memory treats it).
    :param metadata: ``(source, chunk) -> dict`` for each chunk's metadata;
        :func:`chunk_metadata` by default.
    """

    def __init__(
        self,
        memory: Memory,
        *,
        converter: Optional[Any] = None,
        convert_kwargs: Optional[Dict[str, Any]] = None,
        chunker: Optional[Any] = None,
        mime_type: Union[MemoryMimeType, str] = MemoryMimeType.TEXT,
        metadata: MetadataFn = chunk_metadata,
    ):
        self._memory = memory
        if converter is None:
            from .. import DocumentConverter

            converter = DocumentConverter()
        self._converter = converter
        self._convert_kwargs = convert_kwargs or {}
        # The default chunker's tokenizer is resolved (fetched if missing) on
        # first use, not here: constructing an indexer stays offline.
        self._default_chunker = chunker is None
        self._chunker = chunker or HybridChunker()
        self._mime_type = mime_type
        self._metadata = metadata

    @property
    def memory(self) -> Memory:
        return self._memory

    def _ensure_tokenizer(self) -> None:
        if self._default_chunker and self._chunker.tokenizer is None:
            path = models.chunk_tokenizer(fetch=True)
            if path is not None:
                self._chunker.tokenizer = str(path)
            self._default_chunker = False

    def chunks(self, source: str) -> Iterator[MemoryContent]:
        """Convert ``source`` (a path or URL) and yield its chunks as
        ``MemoryContent`` — heading path plus text, the way the chunker
        contextualizes them for embedding — without adding them anywhere."""
        self._ensure_tokenizer()
        dl_doc = self._converter.convert(source=source, **self._convert_kwargs).document
        for chunk in self._chunker.chunk(dl_doc):
            yield MemoryContent(
                content=self._chunker.contextualize(chunk=chunk),
                mime_type=self._mime_type,
                metadata=self._metadata(source, docling_chunk(chunk, dl_doc)),
            )

    async def index_document(self, source: str) -> int:
        """Convert, chunk and add one document; returns the number of chunks."""
        n = 0
        for item in self.chunks(source):
            await self._memory.add(item)
            n += 1
        return n

    async def index_documents(self, sources: Iterable[str]) -> int:
        """:meth:`index_document` for each source; returns the total."""
        n = 0
        for source in sources:
            n += await self.index_document(source)
        return n
