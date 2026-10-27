"""Retrieval for Agent Framework agents: the chunks relevant to each turn put
in the context (:class:`DoclingContextProvider`), or a search tool the agent
calls itself (:func:`docling_search_tool`)."""

from __future__ import annotations

from typing import Any, Callable, Optional

try:
    from agent_framework import ContextProvider, Message, create_vector_search_tool
except ImportError as e:  # pragma: no cover - exercised only without the extra
    raise ImportError(
        "docling_rs.agent_framework needs agent-framework-core: "
        "pip install 'docling-rs[agent-framework]'"
    ) from e

from .records import format_chunk

DEFAULT_HEADER = (
    "Excerpts from the indexed documents that may be relevant to the request; "
    "each starts with its source, pages and section. Cite the pages you use."
)


class DoclingContextProvider(ContextProvider):
    """Before every run, search ``collection`` with the user's messages and add
    the best chunks to the context, each under a citation line — what an
    AutoGen ``AssistantAgent(memory=[…])`` did with a vector memory, as an
    Agent Framework context provider::

        agent = Agent(client=…, context_providers=[DoclingContextProvider(collection)])

    :param collection: the collection :class:`DoclingIndexer` filled (anything
        with ``search(query, top=…)``).
    :param top: chunks per turn.
    :param score_threshold: passed to the search (its units and direction are
        the store's: a maximum cosine *distance* for the in-memory one).
    :param filter: a vector-store filter, e.g. one source's chunks only.
    :param search_type: ``"vector"`` or ``"keyword_hybrid"`` where the store
        supports it.
    :param header: the line above the excerpts; ``None`` for none.
    :param formatter: ``record -> str`` for one chunk (:func:`format_chunk`).
    :param source_id: the provider id Agent Framework attributes the context
        messages to.
    """

    def __init__(
        self,
        collection: Any,
        *,
        top: int = 4,
        score_threshold: Optional[float] = None,
        filter: Optional[Any] = None,
        search_type: str = "vector",
        header: Optional[str] = DEFAULT_HEADER,
        formatter: Callable[[Any], str] = format_chunk,
        source_id: str = "docling",
    ):
        super().__init__(source_id)
        self._collection = collection
        self._top = top
        self._score_threshold = score_threshold
        self._filter = filter
        self._search_type = search_type
        self._header = header
        self._formatter = formatter

    async def retrieve(self, query: str) -> list:
        """The records the provider would add for ``query``, best first."""
        kwargs: dict = {"top": self._top, "search_type": self._search_type}
        if self._score_threshold is not None:
            kwargs["score_threshold"] = self._score_threshold
        if self._filter is not None:
            kwargs["filter"] = self._filter
        results = await self._collection.search(query, **kwargs)
        return [hit["record"] async for hit in results]

    async def before_run(self, *, agent: Any, session: Any, context: Any, state: dict) -> None:
        query = "\n".join(
            m.text for m in (context.input_messages or []) if m.role == "user" and m.text
        ).strip()
        if not query:
            return
        records = await self.retrieve(query)
        if not records:
            return
        parts = [self._header] if self._header else []
        parts += [self._formatter(r) for r in records]
        context.extend_messages(self, [Message(role="user", contents=["\n\n".join(parts)])])


def docling_search_tool(
    collection: Any,
    *,
    name: str = "search_documents",
    description: str = (
        "Search the indexed documents. Returns the most relevant excerpts, each "
        "with its source, pages and section."
    ),
    top: int = 5,
    formatter: Callable[[Any], str] = format_chunk,
    **kwargs: Any,
) -> Any:
    """Agentic retrieval: a ``FunctionTool`` (``search_documents(query)``) over
    the collection :class:`DoclingIndexer` filled, answering with
    :func:`format_chunk` citations — Agent Framework's
    ``create_vector_search_tool`` with a result mapper for docling chunks.
    Extra keyword arguments (``filter``, ``search_type``, ``approval_mode``,
    …) go to it."""

    def result_mapper(response: Any) -> str:
        return formatter(response["record"])

    return create_vector_search_tool(
        collection,
        name=name,
        description=description,
        top=top,
        result_mapper=result_mapper,
        **kwargs,
    )
