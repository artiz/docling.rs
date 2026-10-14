"""LangChain document loader backed by the docling.rs engine — the port of
``langchain_docling.loader`` (docling-project/docling-langchain).

Same class names, parameters and output as the upstream loader, so a
LangChain pipeline switches engines by changing one import::

    # was: from langchain_docling import DoclingLoader
    from docling_rs.langchain import DoclingLoader

    docs = DoclingLoader(file_path="report.pdf").load()

What differs is only underneath: conversion runs in docling.rs's Rust engine
(no PyTorch), and the default chunker is :class:`docling_rs.chunking.HybridChunker`
— the native port of docling's ``HybridChunker`` with the same default
tokenizer (all-MiniLM-L6-v2) and token budget (256). Its ``tokenizer.json``
(~0.5 MB) is fetched into the model cache on first use when not present, the
way docling's chunker pulls it from the Hugging Face Hub. Chunk metadata
carries docling's own ``DocMeta`` JSON (``dl_meta``: the chunk's document
items with their provenance, its headings and the document origin), built
by resolving the native chunk's item refs against the converted document —
the same dictionary ``langchain-docling`` produces.

Any docling-compatible pieces plug in, as with the upstream loader: docling's
own chunkers (``docling.chunking.HybridChunker`` with a Hugging Face
tokenizer) work on the converted document because it *is* a docling-core
``DoclingDocument``, and ``converter`` takes any object whose
``convert(source=...)`` returns a result with a ``.document`` — this
package's :class:`~docling_rs.DocumentConverter` (the default), docling's
own, or docling's ``DoclingServiceClient``.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from enum import Enum
from typing import Any, Callable, Dict, Iterable, Iterator, Optional, Union

try:
    from typing import Protocol
except ImportError:  # pragma: no cover - Python < 3.8
    from typing_extensions import Protocol  # type: ignore

try:
    from langchain_core.document_loaders import BaseLoader
    from langchain_core.documents import Document
except ImportError as e:  # pragma: no cover - exercised only without the extra
    raise ImportError(
        "docling_rs.langchain needs langchain-core: pip install 'docling-rs[langchain]'"
    ) from e

from .. import models
from .._chunk_meta import docling_chunk as _docling_chunk
from ..chunking import HybridChunker


class ExportType(str, Enum):
    """Enumeration of available export types."""

    MARKDOWN = "markdown"
    DOC_CHUNKS = "doc_chunks"


class ConversionBackend(Protocol):
    """Structural interface for local and service-backed conversion: anything
    with a ``convert(source=..., **kwargs)`` returning a result that has a
    ``.document`` (a docling-core ``DoclingDocument``)."""

    @property
    def convert(self) -> Callable[..., Any]:
        """Return the backend-specific conversion callable."""
        ...


class BaseMetaExtractor(ABC):
    """BaseMetaExtractor."""

    @abstractmethod
    def extract_chunk_meta(self, file_path: str, chunk: Any) -> Dict[str, Any]:
        """Extract chunk meta."""
        raise NotImplementedError()

    @abstractmethod
    def extract_dl_doc_meta(self, file_path: str, dl_doc: Any) -> Dict[str, Any]:
        """Extract Docling document meta."""
        raise NotImplementedError()


class MetaExtractor(BaseMetaExtractor):
    """MetaExtractor — upstream's default: ``source`` plus, per chunk, the
    docling ``DocMeta`` JSON as ``dl_meta``."""

    def extract_chunk_meta(self, file_path: str, chunk: Any) -> Dict[str, Any]:
        """Extract chunk meta."""
        return {
            "source": file_path,
            "dl_meta": chunk.meta.export_json_dict(),
        }

    def extract_dl_doc_meta(self, file_path: str, dl_doc: Any) -> Dict[str, Any]:
        """Extract Docling document meta."""
        return {"source": file_path}


class DoclingLoader(BaseLoader):
    """Docling Loader."""

    def __init__(
        self,
        file_path: Union[str, Iterable[str]],
        *,
        converter: Optional[ConversionBackend] = None,
        convert_kwargs: Optional[Dict[str, Any]] = None,
        export_type: ExportType = ExportType.DOC_CHUNKS,
        md_export_kwargs: Optional[Dict[str, Any]] = None,
        chunker: Optional[Any] = None,
        meta_extractor: Optional[BaseMetaExtractor] = None,
    ):
        """Initialize with a file path.

        Args:
            file_path: File source as single str (URL or local file) or Iterable
                thereof.
            converter: A :class:`docling_rs.DocumentConverter` (configured with
                any of its options), docling's ``DocumentConverter`` or
                ``DoclingServiceClient``, or any compatible conversion backend.
                Defaults to `None` (a default ``docling_rs.DocumentConverter``).
            convert_kwargs: Any backend-specific kwargs to pass to the conversion
                invocation. Defaults to `None` (no extra kwargs).
            export_type: The type to export to: either `ExportType.MARKDOWN` (outputs
                Markdown of whole input file) or `ExportType.DOC_CHUNKS` (outputs chunks
                based on chunker).
            md_export_kwargs: Any specific kwargs to pass to Markdown export (in case of
                `ExportType.MARKDOWN`). Defaults to `None` (i.e.
                ``{"image_placeholder": ""}``, as upstream).
            chunker: Any chunker with ``chunk(dl_doc)`` and
                ``contextualize(chunk=...)`` — ``docling_rs.chunking``'s or
                docling's own (in case of `ExportType.DOC_CHUNKS`). Defaults to
                `None` (``docling_rs.chunking.HybridChunker()``).
            meta_extractor: The extractor instance to use for populating the output
                document metadata; if not set, a system default is used.
        """
        self._file_paths = (
            file_path
            if isinstance(file_path, Iterable) and not isinstance(file_path, str)
            else [file_path]
        )

        if converter is None:
            from .. import DocumentConverter

            self._converter: ConversionBackend = DocumentConverter()
        else:
            self._converter = converter
        self._convert_kwargs = convert_kwargs if convert_kwargs is not None else {}
        self._export_type = export_type
        self._md_export_kwargs = (
            md_export_kwargs if md_export_kwargs is not None else {"image_placeholder": ""}
        )
        # The default chunker's tokenizer is resolved (and fetched if missing)
        # on first use, not here: constructing a loader stays offline.
        self._default_chunker = export_type == ExportType.DOC_CHUNKS and chunker is None
        if self._export_type == ExportType.DOC_CHUNKS:
            self._chunker = chunker or HybridChunker()
        self._meta_extractor = meta_extractor or MetaExtractor()

    def _ensure_tokenizer(self) -> None:
        if self._default_chunker and self._chunker.tokenizer is None:
            path = models.chunk_tokenizer(fetch=True)
            if path is not None:
                self._chunker.tokenizer = str(path)
            self._default_chunker = False

    def lazy_load(
        self,
    ) -> Iterator[Document]:
        """Lazy load documents."""
        for file_path in self._file_paths:
            conv_res = self._converter.convert(
                source=file_path,
                **self._convert_kwargs,
            )
            dl_doc = conv_res.document
            if self._export_type == ExportType.MARKDOWN:
                yield Document(
                    page_content=dl_doc.export_to_markdown(**self._md_export_kwargs),
                    metadata=self._meta_extractor.extract_dl_doc_meta(
                        file_path=file_path,
                        dl_doc=dl_doc,
                    ),
                )
            elif self._export_type == ExportType.DOC_CHUNKS:
                self._ensure_tokenizer()
                chunk_iter = self._chunker.chunk(dl_doc)
                for chunk in chunk_iter:
                    yield Document(
                        page_content=self._chunker.contextualize(chunk=chunk),
                        metadata=self._meta_extractor.extract_chunk_meta(
                            file_path=file_path,
                            chunk=_docling_chunk(chunk, dl_doc),
                        ),
                    )

            else:
                raise ValueError(f"Unexpected export type: {self._export_type}")
