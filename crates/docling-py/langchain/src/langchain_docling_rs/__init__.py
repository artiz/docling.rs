"""LangChain integration for docling.rs — ``langchain-docling``'s API on the
Rust engine. The implementation is :mod:`docling_rs.langchain`, shipped in
the ``docling-rs`` wheel; this distribution gives it the ``langchain-<provider>``
name LangChain's integration listing expects and re-exports its names::

    # was:  from langchain_docling import DoclingLoader, ExportType
    from langchain_docling_rs import DoclingLoader, ExportType

    docs = DoclingLoader(file_path="https://arxiv.org/pdf/2408.09869").load()
"""

from docling_rs.langchain import (
    BaseMetaExtractor,
    ConversionBackend,
    DoclingLoader,
    ExportType,
    MetaExtractor,
    PictureDescriptionLangChainOptions,
)

__all__ = [
    "BaseMetaExtractor",
    "ConversionBackend",
    "DoclingLoader",
    "ExportType",
    "MetaExtractor",
    "PictureDescriptionLangChainOptions",
]
