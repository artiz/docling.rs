# docling.rs × AutoGen

`pip install "docling-rs[autogen]"` adds `docling_rs.autogen`:

- `DoclingIndexer(memory)` — `await indexer.index_documents([...])` converts
  each file or URL with docling.rs, chunks it with docling's hybrid chunker
  and adds one `MemoryContent` per chunk to any `autogen_core.memory.Memory`
  — `ListMemory`, or autogen-ext's `ChromaDBVectorMemory` and friends — with
  `source`, `headings` and `pages` metadata.
- `DoclingConvertTool()` — a `FunctionTool` for `AssistantAgent(tools=[...])`:
  the agent reads a document as Markdown when it decides to.

[`rag_agent.py`](./rag_agent.py) puts both on an `AssistantAgent`: index the
Docling technical report, answer a question from the retrieved chunks with page
citations, and let the agent open further documents itself.
