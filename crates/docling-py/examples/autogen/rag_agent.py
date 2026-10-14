"""Agentic RAG over documents with AutoGen: DoclingIndexer → a vector memory →
an AssistantAgent that answers from the retrieved chunks and cites pages.

    pip install "docling-rs[autogen]" "autogen-agentchat" "autogen-ext[openai,chromadb]"
    export OPENAI_API_KEY=...          # or any OpenAI-compatible server:
    export OPENAI_BASE_URL=http://localhost:1234/v1 CHAT_MODEL=...
    python rag_agent.py "Which AI models does Docling use?"

The ingestion side is all docling.rs: conversion (Rust, no PyTorch) and
docling's hybrid chunking, each chunk carrying its source, heading path and
page numbers. Without autogen-ext's Chroma memory the script falls back to
``ListMemory`` (every chunk goes into the context — fine for one short paper).
"""

import asyncio
import os
import sys

from autogen_agentchat.agents import AssistantAgent
from autogen_core.memory import ListMemory
from autogen_ext.models.openai import OpenAIChatCompletionClient

from docling_rs.autogen import DoclingConvertTool, DoclingIndexer

SOURCES = ["https://arxiv.org/pdf/2408.09869"]  # Docling Technical Report
QUESTION = " ".join(sys.argv[1:]) or "Which AI models does Docling use, and what do they do?"


def make_memory():
    try:
        from autogen_ext.memory.chromadb import ChromaDBVectorMemory, PersistentChromaDBVectorMemoryConfig
    except ImportError:
        return ListMemory()
    return ChromaDBVectorMemory(
        config=PersistentChromaDBVectorMemoryConfig(
            collection_name="docling", persistence_path=os.path.expanduser("~/.chromadb_docling"), k=4
        )
    )


async def main() -> None:
    memory = make_memory()
    # --- ingest ----------------------------------------------------------------
    n = await DoclingIndexer(memory).index_documents(SOURCES)
    print(f"indexed {n} chunks from {len(SOURCES)} document(s) into {type(memory).__name__}")

    # --- agent -----------------------------------------------------------------
    agent = AssistantAgent(
        "assistant",
        model_client=OpenAIChatCompletionClient(model=os.getenv("CHAT_MODEL", "gpt-4o-mini")),
        memory=[memory],  # the relevant chunks are added to the context each turn
        tools=[DoclingConvertTool()],  # and the agent can read any other document on demand
        system_message=(
            "Answer from the document excerpts in your context; each carries a source, "
            "a heading path and page numbers — cite the pages you used. If the excerpts "
            "are not enough, read the document with convert_document."
        ),
    )
    result = await agent.run(task=QUESTION)
    print(result.messages[-1].content)
    await memory.close()


if __name__ == "__main__":
    asyncio.run(main())
