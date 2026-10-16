"""LlamaIndex reader for docling.rs — ``llama_index.readers.docling``'s API on
the Rust engine. The implementation is :mod:`docling_rs.llamaindex`, shipped
in the ``docling-rs`` wheel; this package gives it the LlamaHub namespace::

    # was:  from llama_index.readers.docling import DoclingReader
    from llama_index.readers.docling_rs import DoclingRsReader

    docs = DoclingRsReader().load_data(file_path="https://arxiv.org/pdf/2408.09869")

``DoclingReader`` is the same class under the upstream name, for
find-and-replace migrations.
"""

from docling_rs.llamaindex import DoclingReader

DoclingRsReader = DoclingReader

__all__ = ["DoclingRsReader", "DoclingReader"]
