"""Agentic RAG over documents with Microsoft Agent Framework: DoclingIndexer →
a vector collection → an Agent that answers from the retrieved chunks and
cites pages.

    pip install "docling-rs[agent-framework]" agent-framework-openai
    export OPENAI_API_KEY=...          # or any OpenAI-compatible server:
    export OPENAI_BASE_URL=http://localhost:1234/v1 CHAT_MODEL=... EMBEDDING_MODEL=...
    python rag_agent.py "Which AI models does Docling use?"

The ingestion side is all docling.rs: conversion (Rust, no PyTorch) and
docling's hybrid chunking, each chunk carrying its source, heading path and
page numbers. The collection is Agent Framework's in-memory one; any
connector's collection (Azure AI Search, Redis, Qdrant, Postgres, …) of the
same record type works the same way.
"""

import asyncio
import os
import sys

from agent_framework import Agent, InMemoryCollection
from agent_framework.openai import OpenAIChatClient, OpenAIEmbeddingClient

from docling_rs.agent_framework import (
    DoclingContextProvider,
    DoclingConvertTool,
    DoclingIndexer,
    docling_chunk_model,
)

SOURCES = ["https://arxiv.org/pdf/2408.09869"]  # Docling Technical Report
QUESTION = " ".join(sys.argv[1:]) or "Which AI models does Docling use, and what do they do?"
# text-embedding-3-small: 1536 dimensions — the default DoclingChunk's; size
# the record to another embedding model with docling_chunk_model(N).
EMBEDDING_DIMENSIONS = int(os.getenv("EMBEDDING_DIMENSIONS", "1536"))


async def main() -> None:
    base_url = os.getenv("OPENAI_BASE_URL")
    collection = InMemoryCollection(
        docling_chunk_model(EMBEDDING_DIMENSIONS),
        embedding_generator=OpenAIEmbeddingClient(
            model=os.getenv("EMBEDDING_MODEL", "text-embedding-3-small"), base_url=base_url
        ),
    )
    # --- ingest ----------------------------------------------------------------
    n = await DoclingIndexer(collection).index_documents(SOURCES)
    print(f"indexed {n} chunks from {len(SOURCES)} document(s)")

    # --- agent -----------------------------------------------------------------
    agent = Agent(
        client=OpenAIChatClient(os.getenv("CHAT_MODEL", "gpt-5.4-mini"), base_url=base_url),
        name="docs",
        instructions=(
            "Answer from the document excerpts in your context; each starts with its "
            "source, pages and section — cite the pages you used. If the excerpts are "
            "not enough, read the document with convert_document."
        ),
        # the relevant chunks are added to the context each turn …
        context_providers=[DoclingContextProvider(collection, top=4)],
        # … and the agent can read any other document on demand
        tools=[DoclingConvertTool()],
    )
    session = agent.create_session()
    response = await agent.run(QUESTION, session=session)
    print(response.text)


if __name__ == "__main__":
    asyncio.run(main())
