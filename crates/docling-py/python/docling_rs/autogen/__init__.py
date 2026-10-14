"""AutoGen integration for docling.rs (``pip install 'docling-rs[autogen]'``).

Two pieces, for the two ways an AutoGen agent meets a document:

* :class:`DoclingIndexer` — convert documents with docling.rs, chunk them with
  docling's hybrid chunker and add the chunks to any
  ``autogen_core.memory.Memory`` (``ListMemory``, autogen-ext's
  ``ChromaDBVectorMemory``, …): the indexing half of AutoGen's RAG recipe,
  with page provenance per chunk.
* :class:`DoclingConvertTool` — a ``FunctionTool`` for an ``AssistantAgent``'s
  ``tools=[…]``: the agent reads a file or URL as Markdown when it decides to.

::

    from autogen_core.memory import ListMemory
    from docling_rs.autogen import DoclingIndexer, DoclingConvertTool

    memory = ListMemory()
    await DoclingIndexer(memory).index_documents(["report.pdf", "https://…/paper.pdf"])
    agent = AssistantAgent("assistant", model_client=…, memory=[memory], tools=[DoclingConvertTool()])
"""

from .indexer import DoclingIndexer, chunk_metadata
from .tool import DoclingConvertTool

__all__ = ["DoclingConvertTool", "DoclingIndexer", "chunk_metadata"]
