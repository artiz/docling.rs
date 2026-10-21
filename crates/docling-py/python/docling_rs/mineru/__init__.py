"""MinerU-compatible facade — OpenDataLab's MinerU 4 Python SDK over the
docling.rs engine::

    # was: from mineru.parser import parse, ParseResult
    #      from mineru.parser.writer import FileBasedDataWriter
    from docling_rs.mineru.parser import parse, ParseResult
    from docling_rs.mineru.parser.writer import FileBasedDataWriter

    result = parse("paper.pdf", tier="standard", ocr_mode="auto", page_range="1-3")
    print(result.markdown())                     # MinerU's Markdown conventions
    result.save(FileBasedDataWriter("out/paper"))  # markdown.md, middle_json.json,
                                                   # structured_content.json, images/

``parse`` / ``parse_async``, ``ParseResult`` (``pages``, ``markdown()``,
``structured_content()``, ``to_dict()`` / ``to_json()`` / ``from_dict()`` /
``from_json()``, ``save(writer)``, ``export_pages()``), ``MinerUParser`` /
``DocumentParser``, the ``render(middle_json, RenderFormat.X)`` entry with
its option classes, the writers and the page-range helpers follow MinerU
4.0's ``mineru.parser`` / ``mineru.render`` / ``mineru.types`` modules, so
code written against MinerU runs unchanged after the import swap. What
differs, deliberately:

* **The engine is docling.rs**: layout analysis, TableFormer and PP-OCR on
  ONNX Runtime — no DocVortex, no PyTorch, no llama.cpp VLM server.
  ``tier`` maps onto the engine: ``flash`` converts from the text layer
  (no models, as MinerU's flash does for native PDFs); ``basic``,
  ``standard`` and ``advanced`` all run the one full pipeline. ``ocr_mode``
  ``txt`` turns OCR off, ``ocr`` forces it on every page, ``auto`` lets the
  engine decide. ``image_analysis``, ``vlm_config`` and ``source_context``
  are accepted and ignored.
* **The Middle JSON is DocVortex-shaped** (``schema: docvortex.middle``,
  ``schema_version: 2.0``): one page per page, top-level blocks with
  ``type`` / ``index`` / ``bbox`` (normalized to the page, top-left
  origin), inline-span ``content`` for text and titles, ``image`` /
  ``table`` / ``code`` parents with their body and annotation children,
  ``list`` blocks, ``header`` / ``footer`` / ``page_footnote``. docling's
  labels map onto MinerU's block types; the producer is ``docling.rs``.
  It round-trips through ``from_dict`` / ``to_dict`` here; MinerU's own
  strict validator is not reproduced.
* **Every format docling.rs reads** goes through ``parse`` at any tier,
  where MinerU parses Office, HTML and EPUB at ``flash`` only; a document
  without pages (DOCX, HTML …) is one page ``0`` without boxes, as in
  MinerU.
* **The renders**: Markdown (``#`` titles, HTML tables, ``$$`` equations,
  fenced code, ``![](images/page_0_image_3.png)`` after ``save``, ``data:``
  URIs before, ``---`` page separators and headers/footers in ``FULL``
  mode), HTML, Structured Content, Content List V1 (``bbox`` in 0–1000,
  ``page_idx``) and V2 (per page). LaTeX, DOCX, EPUB and PDF renders raise
  ``NotImplementedError``.
* **``MinerUApiParser``** (a remote MinerU server) is not provided; the
  equivalent service is docling-serve.

``parse(..., docling_options={...})`` (a docling.rs extension) forwards
keyword arguments to :class:`docling_rs.DocumentConverter`; the converted
``DoclingDocument`` is on ``result.document``.

``python -m docling_rs.mineru parse FILE|FOLDER -o OUT`` mirrors
``mineru-kit parse``.
"""

from __future__ import annotations

import asyncio
import base64
import enum
import html as _html
import json
import re
import warnings
from abc import ABC, abstractmethod
from copy import deepcopy
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, List, Literal, Optional, Tuple, Union

from pydantic import BaseModel, ConfigDict, Field

from .. import DocumentConverter
from .. import __version__ as _version

__all__ = [
    "Tier",
    "validate_tier",
    "DEPLOYMENT_TIERS",
    "BlockType",
    "MiddleJson",
    "PageInfo",
    "MIDDLE_JSON_SCHEMA_VERSION",
    "ParseResult",
    "DocumentParser",
    "MinerUParser",
    "parse",
    "parse_async",
    "DataWriter",
    "FileBasedDataWriter",
    "RenderFormat",
    "RenderMode",
    "MarkdownRenderOptions",
    "HtmlRenderOptions",
    "StructuredContentRenderOptions",
    "ContentListRenderOptions",
    "ContentListV2RenderOptions",
    "render",
    "render_markdown",
    "render_html",
    "render_structured_content",
    "render_content_list",
    "render_content_list_v2",
    "InvalidRequestError",
    "PAGE_RANGE_DESCRIPTION",
    "normalize_page_range_input",
    "parse_page_range",
    "expand_page_range",
    "parse_page_range_set",
    "count_pages_in_range",
    "format_page_range",
]


# ---- MinerU's types ----------------------------------------------------------

Tier = Literal["flash", "basic", "standard", "advanced"]
DEPLOYMENT_TIERS: Tuple[str, ...] = ("flash", "basic", "standard", "advanced")
OcrMode = Literal["auto", "txt", "ocr"]


def validate_tier(tier: str) -> str:
    """MinerU's ``validate_tier``: the tier name, lower-cased, or ``ValueError``."""
    normalized = (tier or "").strip().lower()
    if normalized not in DEPLOYMENT_TIERS:
        raise ValueError(f"Unsupported tier '{tier}'. Supported tiers: {', '.join(DEPLOYMENT_TIERS)}")
    return normalized


class BlockType(str, enum.Enum):
    """DocVortex's ``BlockType`` (``mineru.types.BlockType``)."""

    IMAGE = "image"
    IMAGE_BODY = "image_body"
    IMAGE_CAPTION = "image_caption"
    IMAGE_FOOTNOTE = "image_footnote"
    TABLE = "table"
    TABLE_BODY = "table_body"
    TABLE_CAPTION = "table_caption"
    TABLE_FOOTNOTE = "table_footnote"
    CHART = "chart"
    CHART_BODY = "chart_body"
    CHART_CAPTION = "chart_caption"
    CHART_FOOTNOTE = "chart_footnote"
    CODE = "code"
    CODE_BODY = "code_body"
    ALGORITHM_BODY = "algorithm_body"
    CODE_CAPTION = "code_caption"
    CODE_FOOTNOTE = "code_footnote"
    TEXT = "text"
    EQUATION = "equation"
    LIST = "list"
    INDEX = "index"
    REF_TEXT = "ref_text"
    HEADER = "header"
    FOOTER = "footer"
    PAGE_NUMBER = "page_number"
    ASIDE_TEXT = "aside_text"
    PAGE_FOOTNOTE = "page_footnote"
    DOC_TITLE = "doc_title"
    PARAGRAPH_TITLE = "paragraph_title"

    def __str__(self) -> str:
        return self.value


PAGE_AUXILIARY_BLOCK_TYPES = {"header", "footer", "page_number", "aside_text"}
_VISUAL = {"image": ("image_body", "image_caption", "image_footnote"),
           "table": ("table_body", "table_caption", "table_footnote"),
           "chart": ("chart_body", "chart_caption", "chart_footnote"),
           "code": ("code_body", "code_caption", "code_footnote")}


class InvalidRequestError(ValueError):
    """MinerU's ``InvalidRequestError`` (``code``, ``message``, ``param``)."""

    def __init__(self, code: str, message: str, param: Optional[str] = None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.param = param


# ---- page ranges (DocVortex's grammar) ----------------------------------------

PAGE_RANGE_DESCRIPTION = (
    "PDF page selection: 1-based inclusive ranges, e.g. '1-5,8,r3-r1'; "
    "'r1' is the last page and 'all' selects every page. Blank means unspecified. "
    "Selections are sorted and deduplicated; out-of-bounds pages are omitted. "
    "Reversed ranges, '~' and negative page numbers are invalid."
)
_SEGMENT = re.compile(r"(r?[1-9][0-9]*)(?:\s*-\s*(r?[1-9][0-9]*))?")


def _invalid_range(raw: Optional[str], reason: str) -> InvalidRequestError:
    return InvalidRequestError("page_range_invalid", f"Invalid page range {raw!r}: {reason}", "page_range")


def _endpoint(token: str) -> int:
    return -int(token[1:]) if token.startswith("r") else int(token)


def _segments(raw: Optional[str]) -> Optional[List[Tuple[int, int]]]:
    value = (raw or "").strip()
    if not value or value == "all":
        return None
    out = []
    for part in value.split(","):
        m = _SEGMENT.fullmatch(part.strip())
        if m is None:
            raise _invalid_range(raw, "expected a page, an inclusive range, or 'all'")
        start = _endpoint(m[1])
        end = _endpoint(m[2]) if m[2] else start
        if (start > 0) == (end > 0) and start > end:
            raise _invalid_range(raw, "reversed ranges are not supported")
        out.append((start, end))
    return out


def _merge(intervals: Iterable[Tuple[int, int]]) -> List[Tuple[int, int]]:
    merged: List[Tuple[int, int]] = []
    for start, end in sorted(intervals):
        if merged and start <= merged[-1][1] + 1:
            merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
        else:
            merged.append((start, end))
    return merged


def _fmt(value: int) -> str:
    return str(value) if value > 0 else f"r{-value}"


def _fmt_intervals(intervals: Iterable[Tuple[int, int]]) -> str:
    return ",".join(_fmt(s) if s == e else f"{_fmt(s)}-{_fmt(e)}" for s, e in intervals)


def normalize_page_range_input(raw: Optional[str]) -> str:
    segments = _segments(raw)
    if segments is None:
        return "all" if (raw or "").strip() else ""
    if all(s > 0 and e > 0 for s, e in segments):
        segments = _merge(segments)
    return _fmt_intervals(segments)


def _resolved(raw: Optional[str], page_count: int) -> List[Tuple[int, int]]:
    segments = _segments(raw)
    if page_count <= 0:
        raise _invalid_range(raw, "document has no available pages")
    if segments is None:
        return [(1, page_count)]
    intervals = []
    for start, end in segments:
        start = start if start > 0 else page_count + start + 1
        end = end if end > 0 else page_count + end + 1
        if start > end:
            raise _invalid_range(raw, "reversed ranges are not supported")
        lo, hi = max(1, start), min(page_count, end)
        if lo <= hi:
            intervals.append((lo, hi))
    if not intervals:
        raise _invalid_range(raw, "selection does not contain any available pages")
    return _merge(intervals)


def parse_page_range(raw: str, page_count: int) -> List[int]:
    """0-based page indices of ``raw`` within ``page_count`` pages."""
    return [i for s, e in _resolved(raw, page_count) for i in range(s - 1, e)]


def expand_page_range(raw: Optional[str], page_count: int) -> str:
    return _fmt_intervals(_resolved(raw, page_count))


def parse_page_range_set(raw: str) -> set:
    segments = _segments(raw) or []
    if any(s < 0 or e < 0 for s, e in segments):
        raise _invalid_range(raw, "page count is required to resolve all/rN")
    return {i for s, e in _merge(segments) for i in range(s, e + 1)}


def count_pages_in_range(raw: str) -> int:
    return len(parse_page_range_set(raw))


def format_page_range(page_numbers: Iterable[int]) -> str:
    return _fmt_intervals(_merge((p, p) for p in page_numbers))


# ---- the Middle JSON ---------------------------------------------------------------

MIDDLE_JSON_SCHEMA_VERSION = "2.0"


class PageInfo(BaseModel):
    """One page: ``page_idx`` (0-based) and its top-level blocks — plain
    dicts in DocVortex's shape (``type``, ``index``, ``bbox``, ``content`` …)."""

    model_config = ConfigDict(extra="forbid")

    page_idx: int = Field(ge=0)
    blocks: List[Dict[str, Any]] = Field(default_factory=list)


class MiddleJson(BaseModel):
    """DocVortex's ``MiddleJson`` document: ``metadata``, ``extensions``,
    ``pages``, ``is_full_document``, ``schema`` / ``schema_version``."""

    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    metadata: Dict[str, Any]
    extensions: Dict[str, Any] = Field(default_factory=dict)
    pages: List[PageInfo]
    is_full_document: bool = True
    schema_id: str = Field(default="docvortex.middle", alias="schema")
    schema_version: str = MIDDLE_JSON_SCHEMA_VERSION

    def to_dict(self, *, skip_defaults: bool = True, exclude_none: bool = False,
                exclude_block_fields: Optional[set] = None) -> Dict[str, Any]:
        payload: Dict[str, Any] = {
            "metadata": deepcopy(self.metadata),
            "extensions": deepcopy(self.extensions),
            "pages": [{"page_idx": p.page_idx, "blocks": deepcopy(p.blocks)} for p in self.pages],
            "is_full_document": self.is_full_document,
            "schema": self.schema_id,
            "schema_version": self.schema_version,
        }
        if exclude_block_fields:
            payload["pages"] = _remove_block_fields(payload["pages"], set(exclude_block_fields))
        if exclude_none:
            payload = _drop_none(payload)
        return payload

    @classmethod
    def from_dict(cls, value: Dict[str, Any]) -> "MiddleJson":
        if (not isinstance(value, dict) or value.get("schema") != "docvortex.middle"
                or value.get("schema_version") != MIDDLE_JSON_SCHEMA_VERSION):
            raise ValueError(f"Expected docvortex.middle schema version {MIDDLE_JSON_SCHEMA_VERSION}; reparse the source document")
        return cls.model_validate(value)

    @classmethod
    def from_json(cls, value: Union[str, bytes]) -> "MiddleJson":
        return cls.from_dict(json.loads(value))


def _remove_block_fields(value: Any, fields: set) -> Any:
    if isinstance(value, list):
        return [_remove_block_fields(v, fields) for v in value]
    if not isinstance(value, dict):
        return value
    return {k: _remove_block_fields(v, fields) for k, v in value.items() if not ("type" in value and k in fields)}


def _drop_none(value: Any) -> Any:
    if isinstance(value, list):
        return [_drop_none(v) for v in value]
    if isinstance(value, dict):
        return {k: _drop_none(v) for k, v in value.items() if v is not None}
    return value


# ---- writers -------------------------------------------------------------------


class DataWriter(ABC):
    """MinerU's ``DataWriter``."""

    @abstractmethod
    def write(self, path: str, data: bytes) -> None: ...

    def write_string(self, path: str, data: str) -> None:
        try:
            payload = data.encode("utf-8")
        except UnicodeEncodeError:
            payload = data.encode("ascii", "replace")
        self.write(path, payload)


class FileBasedDataWriter(DataWriter):
    """MinerU's ``FileBasedDataWriter``: relative paths under ``parent_dir``."""

    def __init__(self, parent_dir: str = ""):
        self._parent_dir = parent_dir

    def write(self, path: str, data: bytes) -> None:
        fn_path = Path(path)
        if not fn_path.is_absolute() and self._parent_dir:
            fn_path = Path(self._parent_dir) / fn_path
        fn_path.parent.mkdir(parents=True, exist_ok=True)
        with open(fn_path, "wb") as fh:
            fh.write(data)


# ---- render contracts ---------------------------------------------------------------


class RenderFormat(str, enum.Enum):
    MARKDOWN = "markdown"
    HTML = "html"
    LATEX = "latex"
    DOCX = "docx"
    EPUB = "epub"
    STRUCTURED_CONTENT = "structured_content"
    CONTENT_LIST = "content_list"
    CONTENT_LIST_V2 = "content_list_v2"
    PDF = "pdf"


class RenderMode(str, enum.Enum):
    DEFAULT = "default"
    FULL = "full"


ImageRenderer = Callable[[Dict[str, Any]], Optional[str]]


@dataclass(frozen=True)
class LatexDelimiters:
    left: str = "$$"
    right: str = "$$"


@dataclass(frozen=True)
class LatexDelimitersConfig:
    display: LatexDelimiters = field(default_factory=LatexDelimiters)
    inline: LatexDelimiters = field(default_factory=lambda: LatexDelimiters("$", "$"))


@dataclass(frozen=True)
class MarkdownRenderOptions:
    latex_delimiters: Optional[LatexDelimitersConfig] = None
    mode: RenderMode = RenderMode.DEFAULT
    asset_base_url: str = ""
    image_renderer: Optional[ImageRenderer] = None


@dataclass(frozen=True)
class HtmlRenderOptions:
    latex_delimiters: Optional[LatexDelimitersConfig] = None
    mode: RenderMode = RenderMode.DEFAULT
    asset_base_url: str = ""
    standalone: bool = True
    document_title: Optional[str] = None


@dataclass(frozen=True)
class StructuredContentRenderOptions:
    latex_delimiters: Optional[LatexDelimitersConfig] = None
    asset_base_url: str = ""


@dataclass(frozen=True)
class ContentListRenderOptions:
    latex_delimiters: Optional[LatexDelimitersConfig] = None
    asset_base_url: str = ""


@dataclass(frozen=True)
class ContentListV2RenderOptions:
    latex_delimiters: Optional[LatexDelimitersConfig] = None
    asset_base_url: str = ""


# ---- rendering helpers ---------------------------------------------------------------


def _spans_text(spans: Any) -> str:
    """Inline spans → Markdown text (``**bold**``, ``*italic*``, `` `code` ``,
    ``$x$``, ``[text](url)``)."""
    if isinstance(spans, str):
        return spans
    out = []
    for span in spans or []:
        kind = span.get("type")
        if kind == "text":
            text = span.get("content", "")
            styles = span.get("styles") or []
            if "bold" in styles:
                text = f"**{text}**"
            if "italic" in styles:
                text = f"*{text}*"
            out.append(text)
        elif kind == "equation_inline":
            out.append(f"${span.get('content', '').strip()}$")
        elif kind == "code_inline":
            out.append(f"`{span.get('content', '')}`")
        elif kind == "hyperlink":
            out.append(f"[{_spans_text(span.get('content'))}]({span.get('url', '')})")
    return "".join(out)


def _plain(spans: Any) -> str:
    if isinstance(spans, str):
        return spans
    return "".join(_plain(s.get("content")) if s.get("type") == "hyperlink" else s.get("content", "") for s in spans or [])


def _join_url(base: str, rel: str) -> str:
    if not base or rel.startswith(("data:", "http://", "https://", "//", "/")):
        return rel
    return base.rstrip("/") + "/" + rel.lstrip("/")


def _image_source(block: Dict[str, Any], asset_base_url: str = "") -> str:
    if block.get("image_path"):
        return _join_url(asset_base_url, block["image_path"])
    if block.get("image_url"):
        return block["image_url"]
    return block.get("image_base64") or ""


def _body(block: Dict[str, Any]) -> Dict[str, Any]:
    kinds = _VISUAL[block["type"]]
    for child in block.get("content", []):
        if child.get("type") in (kinds[0], "algorithm_body"):
            return child
    return {}


def _annotations(block: Dict[str, Any], kind: str) -> List[Dict[str, Any]]:
    return [c for c in block.get("content", []) if c.get("type") == kind]


def _fence(content: str, language: Optional[str] = None) -> str:
    longest = max((len(m.group(0)) for m in re.finditer(r"`+", content)), default=0)
    fence = "`" * max(3, longest + 1)
    return f"{fence}{language or ''}\n{content}{'' if content.endswith(chr(10)) else chr(10)}{fence}"


def _render_list_md(block: Dict[str, Any], depth: int = 0) -> str:
    lines = []
    for child in block.get("content", []):
        if child.get("type") == "list":
            nested = _render_list_md(child, depth + 1)
            if nested:
                lines.extend(nested.splitlines())
            continue
        text = _spans_text(child.get("content"))
        if not text:
            continue
        marker = child.get("marker") or ""
        first, *rest = text.splitlines() or [text]
        indent = "    " * depth
        lines.append(f"{indent}{marker + ' ' if marker else '- '}{first}")
        lines.extend(f"{indent}  {line}" for line in rest)
    return "\n".join(lines)


def _render_block_md(block: Dict[str, Any], *, delimiters: LatexDelimitersConfig, asset_base_url: str,
                     image_renderer: Optional[ImageRenderer]) -> str:
    kind = block.get("type")
    if kind in ("doc_title", "paragraph_title"):
        level = min(max(int(block.get("level", 1)), 1), 6)
        return f"{'#' * level} {_spans_text(block.get('content'))}"
    if kind in ("text", "ref_text", "page_footnote") or kind in PAGE_AUXILIARY_BLOCK_TYPES:
        return _spans_text(block.get("content"))
    if kind == "equation":
        latex = (block.get("content") or "").strip()
        if latex:
            return f"{delimiters.display.left}\n{latex}\n{delimiters.display.right}"
        if image_renderer is not None and (rendered := image_renderer(block)):
            return rendered
        src = _image_source(block, asset_base_url)
        return f"![]({src})" if src else ""
    if kind in ("list", "index"):
        return _render_list_md(block)
    if kind in _VISUAL:
        parts = []
        for child in block.get("content", []):
            ctype = child.get("type", "")
            if ctype.endswith("_body"):
                parts.append(_render_body_md(block, child, delimiters, asset_base_url, image_renderer))
            else:
                parts.append(_spans_text(child.get("content")))
        return "\n\n".join(p.strip("\n") for p in parts if p and p.strip())
    return _spans_text(block.get("content")) if isinstance(block.get("content"), list) else str(block.get("content") or "")


def _render_body_md(parent: Dict[str, Any], body: Dict[str, Any], delimiters: LatexDelimitersConfig,
                    asset_base_url: str, image_renderer: Optional[ImageRenderer]) -> str:
    kind = body.get("type")
    if kind == "code_body":
        return _fence(body.get("content") or "", parent.get("guess_lang"))
    if kind == "algorithm_body":
        return _fence(_spans_text(body.get("content")))
    if kind == "table_body":
        content = (body.get("content") or "").strip()
        if content:
            return content
        if image_renderer is not None and (rendered := image_renderer(parent)):
            return rendered
        src = _image_source(body, asset_base_url)
        return f"![]({src})" if src else ""
    # image_body / chart_body
    if image_renderer is not None and (rendered := image_renderer(parent)):
        return rendered
    src = _image_source(body, asset_base_url)
    return f"![]({src})" if src else ""


def render_markdown(middle_json: MiddleJson, *, mode: RenderMode = RenderMode.DEFAULT, asset_base_url: str = "",
                    image_renderer: Optional[ImageRenderer] = None,
                    latex_delimiters: Optional[LatexDelimitersConfig] = None) -> str:
    """MinerU's ``render_markdown``: pages joined by a blank line (``DEFAULT``,
    headers / footers / page numbers / asides dropped) or by ``---``
    (``FULL``, everything kept)."""
    if not isinstance(middle_json, MiddleJson):
        raise TypeError("render_markdown expects a MiddleJson instance")
    if not isinstance(mode, RenderMode):
        raise TypeError("mode must be a RenderMode value")
    delimiters = latex_delimiters or LatexDelimitersConfig()
    pages = []
    for page in middle_json.pages:
        rendered = []
        for block in page.blocks:
            if mode is RenderMode.DEFAULT and block.get("type") in PAGE_AUXILIARY_BLOCK_TYPES:
                continue
            text = _render_block_md(block, delimiters=delimiters, asset_base_url=asset_base_url, image_renderer=image_renderer)
            if text and text.strip():
                rendered.append(text.strip("\n"))
        pages.append("\n\n".join(rendered))
    if mode is RenderMode.FULL:
        return "\n\n---\n\n".join(pages)
    return "\n\n".join(p for p in pages if p)


def _spans_html(spans: Any) -> str:
    if isinstance(spans, str):
        return _html.escape(spans)
    out = []
    for span in spans or []:
        kind = span.get("type")
        if kind == "text":
            text = _html.escape(span.get("content", ""))
            styles = span.get("styles") or []
            if "bold" in styles:
                text = f"<strong>{text}</strong>"
            if "italic" in styles:
                text = f"<em>{text}</em>"
            out.append(text)
        elif kind == "equation_inline":
            out.append(f"<span class=\"math-inline\">{_html.escape(span.get('content', ''))}</span>")
        elif kind == "code_inline":
            out.append(f"<code>{_html.escape(span.get('content', ''))}</code>")
        elif kind == "hyperlink":
            out.append(f"<a href=\"{_html.escape(span.get('url', ''), quote=True)}\">{_spans_html(span.get('content'))}</a>")
    return "".join(out)


def _render_list_html(block: Dict[str, Any]) -> str:
    items = []
    for child in block.get("content", []):
        if child.get("type") == "list":
            items.append(f"<li>{_render_list_html(child)}</li>")
        else:
            items.append(f"<li>{_spans_html(child.get('content'))}</li>")
    return "<ul>" + "".join(items) + "</ul>"


def _render_block_html(block: Dict[str, Any], asset_base_url: str) -> str:
    kind = block.get("type")
    if kind in ("doc_title", "paragraph_title"):
        level = min(max(int(block.get("level", 1)), 1), 6)
        return f"<h{level}>{_spans_html(block.get('content'))}</h{level}>"
    if kind in ("text", "ref_text"):
        return f"<p>{_spans_html(block.get('content'))}</p>"
    if kind == "page_footnote" or kind in PAGE_AUXILIARY_BLOCK_TYPES:
        return f"<p class=\"{kind}\">{_spans_html(block.get('content'))}</p>"
    if kind == "equation":
        return f"<div class=\"math-display\">{_html.escape((block.get('content') or '').strip())}</div>"
    if kind in ("list", "index"):
        return _render_list_html(block)
    if kind in _VISUAL:
        parts = []
        for child in block.get("content", []):
            ctype = child.get("type", "")
            if ctype == "table_body" and (child.get("content") or "").strip():
                parts.append(child["content"].strip())
            elif ctype == "code_body":
                parts.append(f"<pre><code>{_html.escape(child.get('content') or '')}</code></pre>")
            elif ctype == "algorithm_body":
                parts.append(f"<pre>{_spans_html(child.get('content'))}</pre>")
            elif ctype.endswith("_body"):
                src = _image_source(child, asset_base_url)
                if src:
                    parts.append(f"<img src=\"{_html.escape(src, quote=True)}\">")
            elif ctype.endswith("_caption"):
                parts.append(f"<figcaption>{_spans_html(child.get('content'))}</figcaption>")
            else:
                parts.append(f"<p class=\"{ctype}\">{_spans_html(child.get('content'))}</p>")
        return "<figure>" + "".join(parts) + "</figure>"
    return ""


def render_html(middle_json: MiddleJson, *, mode: RenderMode = RenderMode.DEFAULT, asset_base_url: str = "",
                standalone: bool = True, document_title: Optional[str] = None,
                latex_delimiters: Optional[LatexDelimitersConfig] = None) -> str:
    """MinerU's ``render_html``: a standalone page (``<!doctype html>``, the
    document title) or a fragment; ``FULL`` keeps the page auxiliaries and
    wraps every page in ``<section class="page" data-page-idx="N">``."""
    if not isinstance(middle_json, MiddleJson):
        raise TypeError("render_html expects a MiddleJson instance")
    pages = []
    for page in middle_json.pages:
        parts = [_render_block_html(b, asset_base_url) for b in page.blocks
                 if mode is RenderMode.FULL or b.get("type") not in PAGE_AUXILIARY_BLOCK_TYPES]
        body = "\n".join(p for p in parts if p)
        pages.append(f"<section class=\"page\" data-page-idx=\"{page.page_idx}\">\n{body}\n</section>" if mode is RenderMode.FULL else body)
    body = "\n".join(pages)
    if not standalone:
        return body
    title = document_title or (middle_json.metadata.get("document") or {}).get("title") or "Document"
    return ("<!doctype html>\n<html lang=\"und\">\n<head>\n<meta charset=\"utf-8\">\n"
            f"<title>{_html.escape(title)}</title>\n</head>\n<body>\n<div class=\"docvortex-document\">\n{body}\n</div>\n</body>\n</html>\n")


def _v1_bbox(bbox: Any) -> Optional[List[int]]:
    return [int(v * 1000) for v in bbox] if bbox else None


def _v1_item(block: Dict[str, Any], asset_base_url: str) -> Optional[Dict[str, Any]]:
    kind = block.get("type")
    if kind == "text":
        return {"type": "text", "text": _spans_text(block.get("content"))}
    if kind in ("doc_title", "paragraph_title"):
        return {"type": "text", "text": _spans_text(block.get("content")), "text_level": int(block.get("level", 1))}
    if kind == "page_footnote":
        return {"type": "page_footnote", "text": _spans_text(block.get("content"))}
    if kind in PAGE_AUXILIARY_BLOCK_TYPES:
        return {"type": kind, "text": _spans_text(block.get("content"))}
    if kind == "equation":
        item: Dict[str, Any] = {"type": "equation", "img_path": _image_source(block, asset_base_url)}
        if latex := (block.get("content") or "").strip():
            item["text"] = latex
            item["text_format"] = "latex"
        return item
    if kind in ("list", "index"):
        return {"type": kind, "sub_type": block.get("sub_type") or "text", "list_items": _list_items(block)} if kind == "list" \
            else {"type": "index", "list_items": _list_items(block)}
    if kind == "ref_text":
        return {"type": "list", "sub_type": "ref_text", "list_items": [_spans_text(block.get("content"))]}
    if kind in _VISUAL:
        body = _body(block)
        caps = [_spans_text(c.get("content")) for c in _annotations(block, _VISUAL[kind][1])]
        foots = [_spans_text(c.get("content")) for c in _annotations(block, _VISUAL[kind][2])]
        if kind == "code":
            return {"type": "code", "sub_type": "algorithm" if block.get("sub_type") == "algorithm" else "code",
                    "code_body": body.get("content") if body.get("type") == "code_body" else _spans_text(body.get("content")),
                    "code_caption": caps, "code_footnote": foots}
        item = {"type": kind, "img_path": _image_source(body, asset_base_url), f"{kind}_caption": caps, f"{kind}_footnote": foots}
        content = (body.get("content") or "").strip() if isinstance(body.get("content"), str) else ""
        if kind == "table":
            if content:
                item["table_body"] = content
        elif content or kind == "chart":
            item["content"] = content
        if block.get("sub_type"):
            item["sub_type"] = block["sub_type"]
        return item
    return None


def _list_items(block: Dict[str, Any], depth: int = 0) -> List[str]:
    items = []
    for child in block.get("content", []):
        if child.get("type") in ("list", "index"):
            items.extend(_list_items(child, depth + 1))
        elif text := _spans_text(child.get("content")):
            items.append(f"{'    ' * depth}{text}" if block.get("type") == "index" else text)
    return items


def render_content_list(middle_json: MiddleJson, *, asset_base_url: str = "",
                        latex_delimiters: Optional[LatexDelimitersConfig] = None) -> List[Dict[str, Any]]:
    """MinerU's Content List V1: one flat item per top-level block with
    ``bbox`` (0–1000) and ``page_idx``."""
    if not isinstance(middle_json, MiddleJson):
        raise TypeError("render_content_list expects a MiddleJson instance")
    out = []
    for page in middle_json.pages:
        for block in page.blocks:
            item = _v1_item(block, asset_base_url)
            if item is None:
                continue
            if bbox := _v1_bbox(block.get("bbox")):
                item["bbox"] = bbox
            item["page_idx"] = page.page_idx
            out.append(item)
    return out


def _v2_spans(spans: Any) -> List[Dict[str, Any]]:
    out = []
    for span in spans or []:
        kind = span.get("type")
        if kind == "text":
            payload: Dict[str, Any] = {"type": "text", "content": span.get("content", "")}
            if span.get("styles"):
                payload["style"] = list(span["styles"])
            out.append(payload)
        elif kind in ("equation_inline", "code_inline"):
            out.append({"type": kind, "content": span.get("content", "").strip()})
        elif kind == "hyperlink":
            out.append({"type": "hyperlink", "content": _plain(span.get("content")), "url": span.get("url", "")})
    return out


def _v2_annotations(block: Dict[str, Any], kind: str) -> List[List[Dict[str, Any]]]:
    return [_v2_spans(c.get("content")) for c in _annotations(block, kind)]


def _v2_item(block: Dict[str, Any], asset_base_url: str) -> Optional[Dict[str, Any]]:
    kind = block.get("type")
    if kind in ("text", "ref_text"):
        return {"type": "paragraph", "content": {"paragraph_content": _v2_spans(block.get("content"))}}
    if kind in ("doc_title", "paragraph_title"):
        return {"type": "title", "content": {"title_content": _v2_spans(block.get("content")), "level": int(block.get("level", 1))}}
    if kind == "page_footnote":
        return {"type": "page_footnote", "content": {"page_footnote_content": _v2_spans(block.get("content"))}}
    if kind in PAGE_AUXILIARY_BLOCK_TYPES:
        name = {"header": "page_header", "footer": "page_footer", "page_number": "page_number", "aside_text": "page_aside_text"}[kind]
        return {"type": name, "content": {f"{name}_content": _v2_spans(block.get("content"))}}
    if kind == "equation":
        return {"type": "equation_interline", "content": {"math_content": (block.get("content") or "").strip(), "math_type": "latex",
                                                          "image_source": {"path": _image_source(block, asset_base_url)}}}
    if kind in ("list", "index"):
        items = []
        stack = list(block.get("content", []))
        for child in _flatten(block):
            if text := child.get("content"):
                items.append({"item_type": "text", "item_content": _v2_spans(text)})
        list_type = "reference_list" if block.get("sub_type") == "ref_text" else "text_list"
        content: Dict[str, Any] = {"list_type": list_type, "list_items": items}
        if kind == "list" and list_type == "text_list":
            content["attribute"] = "ordered" if any(c.get("marker", "").rstrip(".)").isdigit() for c in stack if isinstance(c, dict)) else "unordered"
        return {"type": kind, "content": content}
    if kind in _VISUAL:
        body = _body(block)
        if kind == "code":
            return {"type": "code", "content": {
                "code_type": "algorithm" if block.get("sub_type") == "algorithm" else "code",
                "code_content": body.get("content") if body.get("type") == "code_body" else _plain(body.get("content")),
                "code_caption": _v2_annotations(block, "code_caption"), "code_footnote": _v2_annotations(block, "code_footnote"),
                **({"guess_lang": block["guess_lang"]} if block.get("guess_lang") else {}),
            }}
        content = {"image_source": {"path": _image_source(body, asset_base_url)},
                   f"{kind}_caption": _v2_annotations(block, f"{kind}_caption"),
                   f"{kind}_footnote": _v2_annotations(block, f"{kind}_footnote")}
        if kind == "table":
            html = (body.get("content") or "").strip() if isinstance(body.get("content"), str) else ""
            content.update({"html": html, "table_type": "complex" if "rowspan" in html or "colspan" in html else "simple", "table_nest_level": 0})
        else:
            text = (body.get("content") or "") if isinstance(body.get("content"), str) else ""
            if text or block.get("sub_type"):
                content["content"] = text
        item = {"type": kind, "content": content}
        if block.get("sub_type"):
            item["sub_type"] = block["sub_type"]
        return item
    return None


def _flatten(block: Dict[str, Any]) -> List[Dict[str, Any]]:
    out = []
    for child in block.get("content", []):
        if child.get("type") in ("list", "index"):
            out.extend(_flatten(child))
        else:
            out.append(child)
    return out


def render_content_list_v2(middle_json: MiddleJson, *, asset_base_url: str = "",
                           latex_delimiters: Optional[LatexDelimitersConfig] = None) -> List[List[Dict[str, Any]]]:
    """MinerU's Content List V2: the V2 items grouped per page."""
    if not isinstance(middle_json, MiddleJson):
        raise TypeError("render_content_list_v2 expects a MiddleJson instance")
    out = []
    for page in middle_json.pages:
        items = []
        for block in page.blocks:
            item = _v2_item(block, asset_base_url)
            if item is None:
                continue
            if bbox := _v1_bbox(block.get("bbox")):
                item["bbox"] = bbox
            item["page_idx"] = page.page_idx
            items.append(item)
        out.append(items)
    return out


_SC_REMOVED = {"content", "index", "guess_lang", "image_path", "image_base64", "image_url"}


def render_structured_content(middle_json: MiddleJson, *, asset_base_url: str = "",
                              latex_delimiters: Optional[LatexDelimitersConfig] = None) -> Dict[str, Any]:
    """MinerU's Structured Content: the document fields and, per page, every
    block with its metadata and its content as a Markdown string (visual
    blocks with ``captions`` / ``footnotes`` / ``image_source``)."""
    if not isinstance(middle_json, MiddleJson):
        raise TypeError("render_structured_content expects a MiddleJson instance")
    delimiters = latex_delimiters or LatexDelimitersConfig()
    pages = []
    for page in middle_json.pages:
        blocks = []
        for block in page.blocks:
            payload = {k: deepcopy(v) for k, v in block.items() if k not in _SC_REMOVED and v is not None}
            kind = block.get("type")
            if kind in _VISUAL:
                body = _body(block)
                payload["content"] = _render_body_md(block, body, delimiters, asset_base_url, None) if body else ""
                payload["captions"] = [{"type": c["type"], "content": _spans_text(c.get("content"))} for c in _annotations(block, _VISUAL[kind][1])]
                payload["footnotes"] = [{"type": c["type"], "content": _spans_text(c.get("content"))} for c in _annotations(block, _VISUAL[kind][2])]
                if src := _image_source(body, asset_base_url):
                    payload["image_source"] = src
            elif kind == "equation":
                payload["content"] = (block.get("content") or "").strip()
                if src := _image_source(block, asset_base_url):
                    payload["image_source"] = src
            elif kind in ("list", "index"):
                payload["content"] = _render_list_md(block)
            else:
                payload["content"] = _spans_text(block.get("content"))
            blocks.append(payload)
        pages.append({"page_idx": page.page_idx, "blocks": blocks})
    doc = {"metadata": deepcopy(middle_json.metadata), "extensions": deepcopy(middle_json.extensions),
           "is_full_document": middle_json.is_full_document}
    return {"pages": pages, **doc}


_UNSUPPORTED = {RenderFormat.LATEX, RenderFormat.DOCX, RenderFormat.EPUB, RenderFormat.PDF}


def render(middle_json: MiddleJson, output_format: RenderFormat, *, options: Any = None) -> Any:
    """MinerU's ``render``: the Middle JSON as one of the render targets."""
    if not isinstance(middle_json, MiddleJson):
        raise TypeError("render expects a MiddleJson instance")
    if not isinstance(output_format, RenderFormat):
        raise TypeError("output_format must be a RenderFormat value")
    if output_format in _UNSUPPORTED:
        raise NotImplementedError(f"docling_rs.mineru does not render {output_format.value}; use docling-rs --to for other formats")
    if output_format is RenderFormat.MARKDOWN:
        o = options or MarkdownRenderOptions()
        return render_markdown(middle_json, mode=o.mode, asset_base_url=o.asset_base_url, image_renderer=o.image_renderer,
                               latex_delimiters=o.latex_delimiters)
    if output_format is RenderFormat.HTML:
        o = options or HtmlRenderOptions()
        return render_html(middle_json, mode=o.mode, asset_base_url=o.asset_base_url, standalone=o.standalone,
                           document_title=o.document_title, latex_delimiters=o.latex_delimiters)
    if output_format is RenderFormat.STRUCTURED_CONTENT:
        o = options or StructuredContentRenderOptions()
        return render_structured_content(middle_json, asset_base_url=o.asset_base_url, latex_delimiters=o.latex_delimiters)
    if output_format is RenderFormat.CONTENT_LIST:
        o = options or ContentListRenderOptions()
        return render_content_list(middle_json, asset_base_url=o.asset_base_url, latex_delimiters=o.latex_delimiters)
    o = options or ContentListV2RenderOptions()
    return render_content_list_v2(middle_json, asset_base_url=o.asset_base_url, latex_delimiters=o.latex_delimiters)


# ---- the ParseResult ---------------------------------------------------------------


def _materialize(middle_json: MiddleJson) -> Tuple[MiddleJson, Dict[str, bytes]]:
    """DocVortex's ``materialize_middle``: every ``image_base64`` becomes a
    sidecar file ``images/page_<p>_<parent type>_<index>.<ext>`` on a copy,
    the bytes returned alongside."""
    copy = MiddleJson.from_dict(middle_json.to_dict())
    assets: Dict[str, bytes] = {}
    seen: Dict[str, bytes] = {}
    for page in copy.pages:
        for block in page.blocks:
            targets = [(block, block.get("type"), block.get("index"))] if block.get("type") == "equation" else []
            if block.get("type") in _VISUAL:
                targets = [(c, block["type"], block.get("index")) for c in block.get("content", []) if c.get("type", "").endswith("_body")]
            for target, parent_type, index in targets:
                uri = target.get("image_base64")
                if not uri:
                    continue
                m = re.match(r"data:image/([a-z0-9.+-]+);base64,(.*)", uri, flags=re.S | re.I)
                if not m:
                    continue
                ext = {"jpeg": "jpg", "svg+xml": "svg"}.get(m.group(1).lower(), m.group(1).lower())
                data = base64.b64decode(m.group(2))
                name = f"images/page_{page.page_idx}_{'equation' if parent_type == 'equation' else parent_type}_{index}.{ext}"
                n = 1
                while name in seen and seen[name] != data:
                    n += 1
                    name = f"images/page_{page.page_idx}_{parent_type}_{index}_duplicate_{n}.{ext}"
                seen[name] = data
                assets[name] = data
                target["image_path"] = name
                target.pop("image_base64", None)
    return copy, assets


@dataclass
class ParseResult:
    """MinerU's ``ParseResult``: the Middle JSON plus the lazily rendered
    Markdown / Structured Content; ``save(writer)`` persists everything."""

    middle_json: MiddleJson
    _model_output: Any = None
    _document: Any = field(default=None, repr=False, compare=False)

    @property
    def pages(self) -> List[PageInfo]:
        return self.middle_json.pages

    @property
    def document(self) -> Any:
        """docling.rs extension: the converted ``DoclingDocument``."""
        return self._document

    @staticmethod
    def from_dict(d: Dict[str, Any]) -> "ParseResult":
        return ParseResult(middle_json=MiddleJson.from_dict(d))

    def to_dict(self, *, skip_defaults: bool = True) -> Dict[str, Any]:
        exclude = {"image_base64"} if self.middle_json.metadata.get("file_suffix") == "pdf" else None
        return self.middle_json.to_dict(skip_defaults=skip_defaults, exclude_block_fields=exclude)

    @staticmethod
    def from_json(s: str) -> "ParseResult":
        data = json.loads(s)
        if not isinstance(data, dict):
            raise ValueError("ParseResult JSON must decode to a dict.")
        return ParseResult.from_dict(data)

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), ensure_ascii=False, indent=2)

    def markdown(self, *, add_markers: bool = False, mode: Optional[RenderMode] = None, asset_base_url: str = "",
                 image_renderer: Optional[ImageRenderer] = None) -> str:
        if mode is None:
            mode = RenderMode.FULL if add_markers else RenderMode.DEFAULT
        return render_markdown(self.middle_json, mode=mode, asset_base_url=asset_base_url, image_renderer=image_renderer)

    def structured_content(self, *, asset_base_url: str = "") -> Dict[str, Any]:
        return render_structured_content(self.middle_json, asset_base_url=asset_base_url)

    def save(self, writer: DataWriter) -> None:
        """``markdown.md``, ``middle_json.json``, ``structured_content.json``
        (and ``model_output.json`` when there is one) plus the images."""
        middle_json, assets = _materialize(self.middle_json)
        exported = ParseResult(middle_json=middle_json)
        files = {
            "markdown.md": exported.markdown(),
            "middle_json.json": exported.to_json(),
            "structured_content.json": json.dumps(exported.structured_content(), ensure_ascii=False, indent=2),
        }
        if self._model_output is not None:
            files["model_output.json"] = json.dumps(self._model_output, ensure_ascii=False, indent=2)
        for path, content in files.items():
            writer.write_string(path, content)
        for path, payload in sorted(assets.items()):
            writer.write(path, payload)

    def export_pages(self) -> List[PageInfo]:
        return deepcopy(self.pages)


# ---- building the Middle JSON from a DoclingDocument -----------------------------------


def _label(item: Any) -> str:
    label = getattr(item, "label", None)
    return getattr(label, "value", label) or ""


class _Builder:
    def __init__(self, document: Any, *, tier: str, ocr_mode: str, suffix: str, keep: Optional[set]):
        self.doc = document
        self.tier, self.ocr_mode, self.suffix, self.keep = tier, ocr_mode, suffix, keep
        self.sizes: Dict[int, Tuple[float, float]] = {}
        for no, page in sorted(getattr(document, "pages", {}).items()):
            size = getattr(page, "size", None)
            self.sizes[int(no) - 1] = (float(getattr(size, "width", 0) or 0), float(getattr(size, "height", 0) or 0))
        self.pages: Dict[int, List[Dict[str, Any]]] = {}

    # -- geometry ----------------------------------------------------------

    def _page_of(self, item: Any) -> int:
        prov = getattr(item, "prov", None)
        return int(prov[0].page_no) - 1 if prov else 0

    def _bbox(self, item: Any) -> Optional[List[float]]:
        prov = getattr(item, "prov", None)
        if not prov:
            return None
        page = int(prov[0].page_no) - 1
        width, height = self.sizes.get(page, (0.0, 0.0))
        if width <= 0 or height <= 0:
            return None
        b = prov[0].bbox
        origin = getattr(b.coord_origin, "value", str(b.coord_origin)).upper()
        top, bottom = (height - float(b.t), height - float(b.b)) if origin.endswith("BOTTOMLEFT") else (float(b.t), float(b.b))
        x0, x1 = sorted((float(b.l) / width, float(b.r) / width))
        y0, y1 = sorted((top / height, bottom / height))
        clamp = lambda v: round(min(1.0, max(0.0, v)), 3)  # noqa: E731
        x0, y0, x1, y1 = clamp(x0), clamp(y0), clamp(x1), clamp(y1)
        if x1 <= x0 or y1 <= y0:
            return None
        return [x0, y0, x1, y1]

    def _union(self, boxes: List[Optional[List[float]]]) -> Optional[List[float]]:
        boxes = [b for b in boxes if b]
        if not boxes:
            return None
        return [min(b[0] for b in boxes), min(b[1] for b in boxes), max(b[2] for b in boxes), max(b[3] for b in boxes)]

    # -- content -----------------------------------------------------------

    def _spans(self, item: Any) -> List[Dict[str, Any]]:
        text = getattr(item, "text", "") or ""
        if not text:
            return []
        span: Dict[str, Any] = {"type": "text", "content": text}
        fmt = getattr(item, "formatting", None)
        styles = [s for s, on in (("bold", getattr(fmt, "bold", False)), ("italic", getattr(fmt, "italic", False)),
                                  ("underline", getattr(fmt, "underline", False)), ("strikethrough", getattr(fmt, "strikethrough", False))) if on]
        if styles:
            span["styles"] = styles
        link = getattr(item, "hyperlink", None)
        if link:
            return [{"type": "hyperlink", "url": str(link), "content": [span]}]
        return [span]

    def _text_block(self, kind: str, item: Any, **extra: Any) -> Dict[str, Any]:
        block: Dict[str, Any] = {"type": kind, "content": self._spans(item)}
        if (bbox := self._bbox(item)) is not None:
            block["bbox"] = bbox
        block.update(extra)
        return block

    def _add(self, page: int, block: Dict[str, Any]) -> None:
        blocks = self.pages.setdefault(page, [])
        block = {"type": block["type"], "index": len(blocks), **{k: v for k, v in block.items() if k != "type"}}
        if block["type"] in _VISUAL:
            for child in block.get("content", []):
                if child.get("type", "").endswith("_body"):
                    child["index"] = block["index"]
                    if "bbox" in block:
                        child["bbox"] = block["bbox"]
        blocks.append(block)

    def build(self) -> MiddleJson:
        doc = self.doc
        for ref in getattr(doc.body, "children", []):
            self._walk(ref.resolve(doc))
        if not self.pages and not self.sizes:
            self.pages[0] = []
        page_ids = sorted(set(self.pages) | set(self.sizes))
        if self.keep is not None:
            page_ids = [p for p in page_ids if p in self.keep]
        pages = [PageInfo(page_idx=p, blocks=self.pages.get(p, [])) for p in page_ids]
        title = next((getattr(t, "text", None) for t in getattr(doc, "texts", []) if _label(t) == "title" and getattr(t, "text", "")), None)
        if title is None:
            title = next((getattr(t, "text", None) for t in getattr(doc, "texts", []) if _label(t) == "section_header"), None)
        props: Dict[str, Any] = {}
        if title:
            props["title"] = title
        if self.sizes:
            props["page_count"] = len(self.sizes)
            props["page_count_kind"] = "physical"
        metadata: Dict[str, Any] = {"file_suffix": self.suffix, "producer": {"name": "docling.rs", "version": _version}}
        if props:
            metadata["document"] = props
        extensions: Dict[str, Any] = {"mineru": {"tier": self.tier, "parse_mode": self.ocr_mode}}
        if self.sizes:
            extensions["docvortex_layout"] = {"version": 1, "pages": [
                {"page_idx": p, "width_pt": w, "height_pt": h} for p, (w, h) in sorted(self.sizes.items()) if self.keep is None or p in self.keep]}
        return MiddleJson(metadata=metadata, extensions=extensions, pages=pages, is_full_document=self.keep is None)

    def _walk(self, item: Any) -> None:
        doc = self.doc
        label = _label(item)
        children = [c.resolve(doc) for c in getattr(item, "children", [])]
        page = self._page_of(item)
        if label == "table":
            self._add(page, self._visual("table", item, children))
            return
        if label in ("picture", "chart"):
            self._add(page, self._visual("image", item, [c for c in children if _label(c) in ("caption", "footnote")]))
            return
        if hasattr(item, "text"):
            layer = getattr(getattr(item, "content_layer", None), "value", None)
            if label == "title":
                self._add(page, self._text_block("doc_title", item, level=1))
            elif label == "section_header":
                self._add(page, self._text_block("paragraph_title", item, level=min(6, max(2, int(getattr(item, "level", 1) or 1) + 1))))
            elif label == "page_header":
                self._add(page, self._text_block("header", item))
            elif label == "page_footer":
                self._add(page, self._text_block("footer", item))
            elif label == "footnote":
                self._add(page, self._text_block("page_footnote", item))
            elif label == "formula":
                block: Dict[str, Any] = {"type": "equation", "content": getattr(item, "text", "") or ""}
                if (bbox := self._bbox(item)) is not None:
                    block["bbox"] = bbox
                self._add(page, block)
            elif label == "code":
                lang = getattr(getattr(item, "code_language", None), "value", None) or "unknown"
                block = {"type": "code", "sub_type": "code", "guess_lang": str(lang).lower(),
                         "content": [{"type": "code_body", "content": getattr(item, "text", "") or ""}]}
                if (bbox := self._bbox(item)) is not None:
                    block["bbox"] = bbox
                self._add(page, block)
            elif label == "reference":
                self._add(page, self._text_block("ref_text", item))
            elif layer == "furniture":
                self._add(page, self._text_block("aside_text", item))
            else:  # text, paragraph, caption, list_item outside a list, key/value …
                self._add(page, self._text_block("text", item))
            for child in children:  # footnotes nested under a paragraph
                self._walk(child)
            return
        # a group
        if label in ("list", "ordered_list"):
            block = self._list(item, children)
            first = next((c for c in children if getattr(c, "prov", None)), None)
            self._add(self._page_of(first) if first else page, block)
            return
        for child in children:  # chapter / section / inline / sheet / picture_area …: flatten
            self._walk(child)

    def _list(self, group: Any, children: List[Any]) -> Dict[str, Any]:
        doc = self.doc
        items = []
        for child in children:
            label = _label(child)
            if label in ("list", "ordered_list"):
                items.append(self._list(child, [c.resolve(doc) for c in getattr(child, "children", [])]))
            elif hasattr(child, "text"):
                block = self._text_block("text", child)
                marker = getattr(child, "marker", None)
                if marker:
                    block["marker"] = str(marker)
                items.append(block)
        block: Dict[str, Any] = {"type": "list", "sub_type": "text", "content": items}
        if (bbox := self._union([i.get("bbox") for i in items])) is not None:
            block["bbox"] = bbox
        return block

    def _visual(self, kind: str, item: Any, children: List[Any]) -> Dict[str, Any]:
        body: Dict[str, Any] = {"type": f"{kind}_body"}
        if kind == "table":
            try:
                body["content"] = item.export_to_html(self.doc, add_caption=False)
            except TypeError:
                body["content"] = item.export_to_html(self.doc)
        else:
            body["content"] = ""
            ref = getattr(item, "image", None)
            uri = str(getattr(ref, "uri", "") or "") if ref is not None else ""
            if uri.startswith("data:image/"):
                body["image_base64"] = uri
            elif uri.startswith(("http://", "https://")):
                body["image_url"] = uri
            else:
                try:
                    image = item.get_image(self.doc)
                except Exception:  # noqa: BLE001 — a picture without data is a placeholder
                    image = None
                if image is not None:
                    import io

                    buf = io.BytesIO()
                    image.save(buf, format="PNG")
                    body["image_base64"] = "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode("ascii")
        content: List[Dict[str, Any]] = []
        captions = [c for c in children if _label(c) == "caption"]
        footnotes = [c for c in children if _label(c) == "footnote"]
        if kind == "table":
            content.extend(self._text_block(f"{kind}_caption", c) for c in captions)
            content.append(body)
        else:
            content.append(body)
            content.extend(self._text_block(f"{kind}_caption", c) for c in captions)
        content.extend(self._text_block(f"{kind}_footnote", c) for c in footnotes)
        block: Dict[str, Any] = {"type": kind, "content": content}
        if (bbox := self._bbox(item)) is not None:
            block["bbox"] = bbox
        return block


# ---- parsers -------------------------------------------------------------------------

_SUFFIXES = {"jpg": "jpg", "jpeg": "jpg", "png": "png", "webp": "webp", "gif": "gif", "bmp": "bmp", "tif": "tiff", "tiff": "tiff",
             "doc": "doc", "docx": "docx", "ppt": "ppt", "pptx": "pptx", "xls": "xls", "xlsx": "xlsx", "rtf": "rtf", "odt": "odt",
             "ods": "ods", "odp": "odp", "epub": "epub", "html": "html", "htm": "html", "shtml": "html", "mhtml": "mhtml", "mht": "mhtml",
             "csv": "csv", "tsv": "tsv", "pdf": "pdf"}


def _engine_kwargs(tier: str, ocr_mode: str, docling_options: Optional[Dict[str, Any]]) -> Dict[str, Any]:
    kwargs: Dict[str, Any] = {}
    if tier == "flash" and ocr_mode != "ocr":
        kwargs["text_layer_only"] = True
    if ocr_mode == "txt":
        kwargs["do_ocr"] = False
    elif ocr_mode == "ocr":
        kwargs["force_full_page_ocr"] = True
    kwargs.update(docling_options or {})
    return kwargs


_warned: set = set()


def _warn_once(key: str, message: str) -> None:
    if key not in _warned:
        _warned.add(key)
        warnings.warn(message, stacklevel=3)


class DocumentParser(ABC):
    """MinerU's ``DocumentParser``."""

    @abstractmethod
    def parse(self, path: Union[str, Path], *, page_range: str = "") -> ParseResult: ...

    async def parse_async(self, path: Union[str, Path], *, page_range: str = "") -> ParseResult:
        return await asyncio.to_thread(self.parse, path, page_range=page_range)

    def parse_batch(self, paths: List[Union[str, Path]], *, page_range: str = "") -> List[ParseResult]:
        return [self.parse(p, page_range=page_range) for p in paths]

    async def parse_batch_async(self, paths: List[Union[str, Path]], *, page_range: str = "") -> List[ParseResult]:
        return [await self.parse_async(p, page_range=page_range) for p in paths]

    def close(self) -> None:
        return None

    def __enter__(self) -> "DocumentParser":
        return self

    def __exit__(self, *args: object) -> None:
        self.close()


class MinerUParser(DocumentParser):
    """MinerU's ``MinerUParser``: ``tier`` and ``parse_mode`` (its name for
    ``ocr_mode``) decide the engine options; one engine per parser."""

    def __init__(self, tier: str = "standard", parse_mode: str = "auto", image_analysis: bool = True,
                 vlm_config: Any = None, docling_options: Optional[Dict[str, Any]] = None):
        self.tier = validate_tier(tier)
        if parse_mode not in ("auto", "txt", "ocr"):
            raise ValueError(f"Unsupported ocr_mode '{parse_mode}'; one of auto, txt, ocr")
        self.parse_mode = parse_mode
        self.image_analysis = image_analysis
        if vlm_config is not None:
            _warn_once("vlm_config", "docling_rs.mineru: vlm_config is accepted and ignored — docling.rs runs no VLM")
        self._engine = DocumentConverter(**_engine_kwargs(self.tier, parse_mode, docling_options))

    def parse(self, path: Union[str, Path], *, page_range: str = "", source_context: Any = None) -> ParseResult:
        path = Path(path)
        suffix = _SUFFIXES.get(path.suffix.lower().lstrip("."), path.suffix.lower().lstrip(".") or "pdf")
        normalized = normalize_page_range_input(page_range)
        selecting = bool(normalized) and normalized != "all" and suffix == "pdf"
        window = None
        keep = None
        if selecting and not re.search(r"r\d", normalized):
            # absolute pages: the engine converts the window alone
            pages = parse_page_range_set(normalized)
            window, keep = (min(pages), max(pages)), {p - 1 for p in pages}
        result = self._engine.convert(str(path), page_range=window)
        document = result.document
        if selecting and keep is None:
            # pages counted from the end ('r1') need the page count: the whole
            # document was converted, the selection is applied to it
            keep = set(parse_page_range(normalized, len(getattr(document, "pages", {})) or 1))
        builder = _Builder(document, tier=self.tier, ocr_mode=self.parse_mode, suffix=suffix, keep=keep)
        middle = builder.build()
        return ParseResult(middle_json=middle, _document=document)


def parse(path: Union[str, Path], *, tier: str = "standard", ocr_mode: str = "auto", image_analysis: bool = True,
          page_range: str = "", source_context: Any = None, vlm_config: Any = None,
          docling_options: Optional[Dict[str, Any]] = None) -> ParseResult:
    """MinerU's ``parse``: synchronous, one document."""
    parser = MinerUParser(tier=tier, parse_mode=ocr_mode, image_analysis=image_analysis, vlm_config=vlm_config,
                          docling_options=docling_options)
    return parser.parse(path, page_range=page_range, source_context=source_context)


async def parse_async(path: Union[str, Path], *, tier: str = "standard", ocr_mode: str = "auto", image_analysis: bool = True,
                      page_range: str = "", source_context: Any = None, vlm_config: Any = None,
                      docling_options: Optional[Dict[str, Any]] = None) -> ParseResult:
    """MinerU's ``parse_async``."""
    return await asyncio.to_thread(parse, path, tier=tier, ocr_mode=ocr_mode, image_analysis=image_analysis,
                                   page_range=page_range, source_context=source_context, vlm_config=vlm_config,
                                   docling_options=docling_options)
