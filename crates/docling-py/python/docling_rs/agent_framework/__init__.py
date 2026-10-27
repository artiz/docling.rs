"""Microsoft Agent Framework integration for docling.rs
(``pip install 'docling-rs[agent-framework]'``) — the successor of
:mod:`docling_rs.autogen`, AutoGen being in maintenance mode.

The ingestion half of Agent Framework's RAG over its vector stores, retrieval
for an agent, and a document-reading tool, all on the Rust engine:

* :class:`DoclingIndexer` — convert documents with docling.rs, chunk them with
  docling's hybrid chunker and upsert one :data:`DoclingChunk` per chunk into
  any Agent Framework vector collection (``InMemoryCollection``, Azure AI
  Search, Redis, Qdrant, …), embedded by the collection's embedding client,
  with source, heading path and pages on every record.
* :class:`DoclingContextProvider` — puts the chunks relevant to each turn in
  the agent's context, with citations; :func:`docling_search_tool` lets the
  agent search instead.
* :class:`DoclingConvertTool` — a ``FunctionTool`` for an ``Agent``'s
  ``tools=[…]``: the agent reads a file or URL as Markdown when it decides to.

::

    from agent_framework import Agent, InMemoryCollection
    from agent_framework.openai import OpenAIChatClient, OpenAIEmbeddingClient
    from docling_rs.agent_framework import (
        DoclingChunk, DoclingContextProvider, DoclingConvertTool, DoclingIndexer,
    )

    collection = InMemoryCollection(DoclingChunk, embedding_generator=OpenAIEmbeddingClient(…))
    await DoclingIndexer(collection).index_documents(["report.pdf"])
    agent = Agent(
        client=OpenAIChatClient(…),
        context_providers=[DoclingContextProvider(collection)],
        tools=[DoclingConvertTool()],
    )
"""

from .._chunk_meta import chunk_metadata
from .context import DoclingContextProvider, docling_search_tool
from .indexer import DoclingIndexer
from .records import DoclingChunk, chunk_id, docling_chunk_model, format_chunk
from .tool import DoclingConvertTool

__all__ = [
    "DoclingChunk",
    "DoclingContextProvider",
    "DoclingConvertTool",
    "DoclingIndexer",
    "chunk_id",
    "chunk_metadata",
    "docling_chunk_model",
    "docling_search_tool",
    "format_chunk",
]
