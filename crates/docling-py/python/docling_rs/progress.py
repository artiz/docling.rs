"""Conversion progress events — docling's ``docling.datamodel.progress``
(docling#4560, 2.139; docling.rs #783).

Pass a callable to ``DocumentConverter(progress_callback=...)`` to receive
the events of every conversion, or to ``convert(source,
progress_callback=...)`` / ``convert_all(...)`` for that call's;
``DocumentConverter(show_progress=True)`` prints progress bars to stderr
instead (the engine's printer, no extra code). The classes here mirror
docling's one for one, so::

    from docling.datamodel.progress import PageCompletedProgress

becomes::

    from docling_rs.progress import PageCompletedProgress

Events of one document arrive from the threads that convert it (the page
events of a parallel PDF walk from its workers), so a callback must be
thread-safe; an exception it raises is printed and ignored, never breaking
the conversion. The sequence per document: ``DocumentStartedProgress``, a
``PageCompletedProgress`` per PDF / image page (``completed_pages`` counts up
to ``total_pages``, the pages ``page_range`` selected; a page the
``document_timeout`` cut off comes with ``success=False`` so the count still
reaches the total), an ``EnrichmentProgress`` per batch of the picture passes
(picture description, picture OCR), and last ``DocumentCompletedProgress``
with the result's status. docling's ``PhaseStartedProgress`` is not ported:
the engine has no separate build / assemble / enrich phases to time.
"""

from __future__ import annotations

import enum
from dataclasses import dataclass
from typing import Any, Callable, Dict, Union

__all__ = [
    "ConversionProgressKind",
    "EnrichmentStep",
    "BaseConversionProgress",
    "DocumentStartedProgress",
    "PageCompletedProgress",
    "EnrichmentProgress",
    "DocumentCompletedProgress",
    "ConversionProgressEvent",
    "ProgressCallback",
    "event_from_dict",
]


class ConversionProgressKind(str, enum.Enum):
    DOCUMENT_STARTED = "document_started"
    PAGE_COMPLETED = "page_completed"
    ENRICHMENT_PROGRESS = "enrichment_progress"
    DOCUMENT_COMPLETED = "document_completed"


class EnrichmentStep(str, enum.Enum):
    """Stable identifier of an enrichment pass, safe to match on."""

    CODE_FORMULA = "code_formula"
    PICTURE_CLASSIFICATION = "picture_classification"
    PICTURE_DESCRIPTION = "picture_description"
    CHART_EXTRACTION = "chart_extraction"
    OTHER = "other"


@dataclass(frozen=True)
class BaseConversionProgress:
    """Fields shared by all events.

    ``document_index`` is the 1-based position of the document in the
    ``convert_all`` call (always 1 for ``convert``). It tells apart documents
    with the same name and documents converted concurrently.
    """

    kind: ConversionProgressKind
    document_index: int
    document_name: str


@dataclass(frozen=True)
class DocumentStartedProgress(BaseConversionProgress):
    """Emitted once per input document, first."""


@dataclass(frozen=True)
class PageCompletedProgress(BaseConversionProgress):
    """Emitted once per page of a PDF / image conversion, after all
    page-level models (tables included). Pages may finish out of order, so
    drive a progress bar with ``completed_pages`` rather than ``page_no``;
    ``total_pages`` counts the pages selected by ``page_range``."""

    page_no: int
    success: bool
    completed_pages: int
    total_pages: int


@dataclass(frozen=True)
class EnrichmentProgress(BaseConversionProgress):
    """Item-level progress of one enrichment pass on the assembled document:
    ``completed_items=0`` first, then after every item, ending with
    ``completed_items == total_items``. Match on ``step``; ``label`` is a
    readable name for display and may change."""

    step: EnrichmentStep
    label: str
    completed_items: int
    total_items: int


@dataclass(frozen=True)
class DocumentCompletedProgress(BaseConversionProgress):
    """Emitted once as the last event for a document, also when it failed:
    ``status`` is ``"success"``, ``"partial_success"`` or ``"failure"`` (a
    ``docling_rs.ConversionStatus``)."""

    status: Any


ConversionProgressEvent = Union[
    DocumentStartedProgress,
    PageCompletedProgress,
    EnrichmentProgress,
    DocumentCompletedProgress,
]

ProgressCallback = Callable[[ConversionProgressEvent], None]


def event_from_dict(data: Dict[str, Any]) -> ConversionProgressEvent:
    """The typed event of the engine's flat event object (``kind`` plus the
    kind's fields) — what the native callback hands up."""
    from . import ConversionStatus

    kind = ConversionProgressKind(data["kind"])
    index = int(data["document_index"])
    name = str(data["document_name"])
    if kind is ConversionProgressKind.PAGE_COMPLETED:
        return PageCompletedProgress(
            kind,
            index,
            name,
            page_no=int(data["page_no"]),
            success=bool(data["success"]),
            completed_pages=int(data["completed_pages"]),
            total_pages=int(data["total_pages"]),
        )
    if kind is ConversionProgressKind.ENRICHMENT_PROGRESS:
        try:
            step = EnrichmentStep(data["step"])
        except ValueError:
            step = EnrichmentStep.OTHER
        return EnrichmentProgress(
            kind,
            index,
            name,
            step=step,
            label=str(data["label"]),
            completed_items=int(data["completed_items"]),
            total_items=int(data["total_items"]),
        )
    if kind is ConversionProgressKind.DOCUMENT_COMPLETED:
        return DocumentCompletedProgress(kind, index, name, status=ConversionStatus(data["status"]))
    return DocumentStartedProgress(kind, index, name)
