"""A document-reading tool for Agent Framework agents."""

from __future__ import annotations

from typing import Annotated, Any, Dict, Optional

try:
    from agent_framework import FunctionTool
except ImportError as e:  # pragma: no cover - exercised only without the extra
    raise ImportError(
        "docling_rs.agent_framework needs agent-framework-core: "
        "pip install 'docling-rs[agent-framework]'"
    ) from e

DESCRIPTION = (
    "Read a document — PDF, Word, PowerPoint, Excel, HTML, an image or a scan — "
    "from a file path or URL and return its contents as Markdown, headings and "
    "tables included."
)


class DoclingConvertTool(FunctionTool):
    """``convert_document(source) -> str``: the document as Markdown, for an
    ``Agent``'s ``tools=[…]``. Agent Framework runs a synchronous tool on a
    worker thread, so the conversion does not block the agent loop.

    The tool reads whatever path or URL the model names, with this process's
    file access: where the conversation carries untrusted text (a document
    could tell the model to read ``~/.ssh/…``), set
    ``approval_mode="always_require"`` or hand in a converter that only
    reaches what it should.

    :param converter: a configured :class:`docling_rs.DocumentConverter`, or
        any object whose ``convert(source=…)`` returns a result with a
        ``.document``; a plain ``DocumentConverter()`` by default.
    :param max_chars: documents longer than this come back cut, with a
        ``[… N more characters]`` tail, so a 300-page manual does not land in
        the model context whole; ``None`` returns everything.
    :param convert_kwargs: extra keyword arguments for ``converter.convert``.
    :param md_export_kwargs: for ``export_to_markdown`` (pictures dropped by
        default, as the LangChain loader does).
    :param name: the tool name the model sees (``convert_document``).
    :param description: what the model is told the tool does.
    :param approval_mode: Agent Framework's ``"never_require"`` (default:
        reading is not a side effect) or ``"always_require"``.
    """

    def __init__(
        self,
        converter: Optional[Any] = None,
        *,
        max_chars: Optional[int] = 40_000,
        convert_kwargs: Optional[Dict[str, Any]] = None,
        md_export_kwargs: Optional[Dict[str, Any]] = None,
        name: str = "convert_document",
        description: str = DESCRIPTION,
        approval_mode: str = "never_require",
    ):
        if converter is None:
            from .. import DocumentConverter

            converter = DocumentConverter()
        self._converter = converter
        self._max_chars = max_chars
        self._convert_kwargs = convert_kwargs or {}
        self._md_export_kwargs = (
            md_export_kwargs if md_export_kwargs is not None else {"image_placeholder": ""}
        )

        def convert_document(
            source: Annotated[str, "File path or URL of the document to read"],
        ) -> str:
            return self.convert(source)

        super().__init__(
            name=name,
            description=description,
            func=convert_document,
            approval_mode=approval_mode,
        )

    def convert(self, source: str) -> str:
        """What the tool returns for ``source``, callable directly too."""
        result = self._converter.convert(source=source, **self._convert_kwargs)
        text = result.document.export_to_markdown(**self._md_export_kwargs)
        if self._max_chars is not None and len(text) > self._max_chars:
            text = text[: self._max_chars] + f"\n\n[… {len(text) - self._max_chars} more characters]"
        return text
