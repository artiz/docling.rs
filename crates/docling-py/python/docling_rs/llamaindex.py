"""LlamaIndex integration for docling.rs — the port of LlamaIndex's
``llama-index-readers-docling`` (``pip install 'docling-rs[llamaindex]'``).

Swap the import, keep the code::

    # was:  from llama_index.readers.docling import DoclingReader
    from docling_rs.llamaindex import DoclingReader

    reader = DoclingReader()
    docs = reader.load_data(file_path="https://arxiv.org/pdf/2408.09869")

:class:`DoclingReader` is a ``BasePydanticReader``: ``load_data`` /
``lazy_load_data`` take one path or URL, or an iterable of them, and return
one LlamaIndex ``Document`` per input — its text the document's Markdown
(``ExportType.MARKDOWN``, the default) or docling's native JSON
(``ExportType.JSON``, for a Docling node parser downstream). It plugs into
``SimpleDirectoryReader`` through ``file_extractor`` like the upstream reader.
The conversion is docling.rs's :class:`docling_rs.DocumentConverter`:
configure it as usual (``do_ocr=False``, ``ocr_lang=…``, ``text_layer_only``,
…) and pass it as ``doc_converter``.

Differences from the upstream reader: ``extra_info`` is copied into each
document's metadata (upstream shares the one dict between them), and the
``fs`` argument is accepted for signature compatibility but not used, as
upstream does not use it either.
"""

from __future__ import annotations

import json
import uuid
from enum import Enum
from pathlib import Path
from typing import Any, Dict, Iterable, Optional, Protocol, Union, runtime_checkable

try:
    from llama_index.core.readers.base import BasePydanticReader
    from llama_index.core.schema import Document as LIDocument
except ImportError as e:  # pragma: no cover - exercised only without the extra
    raise ImportError(
        "docling_rs.llamaindex needs llama-index-core: pip install 'docling-rs[llamaindex]'"
    ) from e
from pydantic import ConfigDict, Field

from . import DocumentConverter

__all__ = ["DoclingReader"]


def _default_converter() -> DocumentConverter:
    return DocumentConverter()


class DoclingReader(BasePydanticReader):
    """Docling.rs Reader.

    Extracts PDF, DOCX, PPTX, XLSX, HTML and the other formats docling.rs
    converts into LlamaIndex Documents, as Markdown or as docling's native
    JSON — the API of LlamaIndex's ``DoclingReader``, on the Rust engine.

    Args:
        export_type (Literal["markdown", "json"], optional): The type to export
            to. Defaults to "markdown".
        doc_converter (DocumentConverter, optional): The docling.rs converter
            to use. Default factory: ``docling_rs.DocumentConverter()``.
        md_export_kwargs (Dict[str, Any], optional): Kwargs to use in case of
            markdown export. Defaults to ``{"image_placeholder": ""}``.
        id_func (DocIDGenCallable, optional): Doc ID generation function to
            use. Default: a random UUID per document.
    """

    class ExportType(str, Enum):
        MARKDOWN = "markdown"
        JSON = "json"

    @runtime_checkable
    class DocIDGenCallable(Protocol):
        def __call__(self, doc: Any, file_path: Union[str, Path]) -> str: ...

    @staticmethod
    def _uuid4_doc_id_gen(doc: Any, file_path: Union[str, Path]) -> str:
        return str(uuid.uuid4())

    model_config = ConfigDict(arbitrary_types_allowed=True)

    export_type: ExportType = ExportType.MARKDOWN
    doc_converter: DocumentConverter = Field(default_factory=_default_converter)
    md_export_kwargs: Dict[str, Any] = {"image_placeholder": ""}
    id_func: DocIDGenCallable = _uuid4_doc_id_gen

    @classmethod
    def class_name(cls) -> str:
        return "DoclingReader"

    def lazy_load_data(
        self,
        file_path: Union[str, Path, Iterable[str], Iterable[Path]],
        extra_info: Optional[dict] = None,
        fs: Optional[Any] = None,
    ) -> Iterable[LIDocument]:
        """Lazily load from the given source.

        Args:
            file_path (str | Path | Iterable[str] | Iterable[Path]): Document
                file source as single str (URL or local file) or Path, or an
                iterable of these.
            extra_info (dict | None, optional): Any pre-existing metadata to
                include. Defaults to None.
            fs: accepted for signature compatibility with the upstream reader;
                sources are read by docling.rs itself (paths and http(s) URLs).

        Returns:
            Iterable[Document]: One LlamaIndex Document per input.
        """
        sources: Iterable[Union[str, Path]]
        if isinstance(file_path, (str, Path)):
            sources = [file_path]
        else:
            sources = file_path
        for source in sources:
            dl_doc = self.doc_converter.convert(str(source)).document
            if self.export_type == self.ExportType.MARKDOWN:
                text = dl_doc.export_to_markdown(**self.md_export_kwargs)
            elif self.export_type == self.ExportType.JSON:
                text = json.dumps(dl_doc.export_to_dict())
            else:  # pragma: no cover - the enum has two members
                raise ValueError(f"Unexpected export type: {self.export_type}")
            li_doc = LIDocument(doc_id=self.id_func(doc=dl_doc, file_path=source), text=text)
            li_doc.metadata = dict(extra_info or {})
            yield li_doc
