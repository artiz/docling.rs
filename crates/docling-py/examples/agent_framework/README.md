# docling.rs × Microsoft Agent Framework

`pip install "docling-rs[agent-framework]"` adds `docling_rs.agent_framework`,
the successor of the [AutoGen integration](../autogen/) — AutoGen is in
maintenance mode, and Agent Framework is where Microsoft moved its agents:

- `DoclingIndexer(collection)` — `await indexer.index_documents([...])`
  converts each file or URL with docling.rs, chunks it with docling's hybrid
  chunker and upserts one `DoclingChunk` per chunk into any Agent Framework
  vector collection — `InMemoryCollection`, or a connector's (Azure AI Search,
  Redis, Qdrant, Postgres, …) — which embeds it with its embedding client.
  Every record carries `source`, `headings` and `pages`, and a stable key, so
  indexing a document again overwrites its chunks.
- `DoclingContextProvider(collection)` — for `Agent(context_providers=[...])`:
  each turn, the chunks closest to the user's message go into the context
  under a citation line (`[report.pdf, pp. 3,4 — Results > Tables]`);
  `docling_search_tool(collection)` gives the agent a `search_documents`
  tool instead.
- `DoclingConvertTool()` — a `FunctionTool` for `Agent(tools=[...])`: the
  agent reads a document as Markdown when it decides to.

[`rag_agent.py`](./rag_agent.py) puts them on one `Agent`: index the Docling
technical report, answer a question from the retrieved chunks with page
citations, and let the agent open further documents itself.

| AutoGen (`docling_rs.autogen`) | Agent Framework (`docling_rs.agent_framework`) |
| --- | --- |
| `DoclingIndexer(memory)` → `MemoryContent` | `DoclingIndexer(collection)` → `DoclingChunk` records |
| `AssistantAgent(memory=[memory])` | `Agent(context_providers=[DoclingContextProvider(collection)])` |
| `DoclingConvertTool()` (`FunctionTool`) | `DoclingConvertTool()` (`FunctionTool`) |
