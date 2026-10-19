"""#783: ``progress_callback`` / ``show_progress`` — docling's conversion
progress events (docling#4560), on the text-layer path (no models)."""

from pathlib import Path

import docling_rs
from docling_rs import ConversionStatus, DocumentConverter
from docling_rs.progress import (
    ConversionProgressKind,
    DocumentCompletedProgress,
    DocumentStartedProgress,
    PageCompletedProgress,
)

REPO = Path(__file__).resolve().parents[3]
THREE_PAGES = REPO / "tests/data/pdf/sources/skipped_1page.pdf"
DOCX = REPO / "tests/data/docx/sources/word_sample.docx"


def pages(events):
    return [
        (e.page_no, e.success, e.completed_pages, e.total_pages)
        for e in events
        if isinstance(e, PageCompletedProgress)
    ]


def test_page_window_reports_its_pages_as_typed_events():
    events = []
    converter = DocumentConverter(text_layer_only=True, progress_callback=events.append)
    result = converter.convert(THREE_PAGES, page_range=(2, 3))
    assert result.status == ConversionStatus.SUCCESS
    assert isinstance(events[0], DocumentStartedProgress)
    assert events[0].kind is ConversionProgressKind.DOCUMENT_STARTED
    assert events[0].document_index == 1 and events[0].document_name == "skipped_1page"
    assert isinstance(events[-1], DocumentCompletedProgress)
    assert events[-1].status == ConversionStatus.SUCCESS
    page_events = pages(events)
    assert sorted(p[0] for p in page_events) == [2, 3]
    assert [p[1:] for p in page_events] == [(True, 1, 2), (True, 2, 2)]


def test_cut_off_pages_are_failed_and_the_call_callback_adds_to_the_constructors():
    seen_ctor, seen_call = [], []
    converter = DocumentConverter(
        text_layer_only=True, document_timeout=1e-9, progress_callback=seen_ctor.append
    )
    result = converter.convert(THREE_PAGES, progress_callback=seen_call.append)
    assert result.status == ConversionStatus.PARTIAL_SUCCESS
    for events in (seen_ctor, seen_call):
        assert pages(events) == [(1, False, 1, 3), (2, False, 2, 3), (3, False, 3, 3)]
        assert events[-1].status == ConversionStatus.PARTIAL_SUCCESS


def test_convert_all_numbers_the_documents_and_a_docx_has_no_pages():
    events = []
    converter = DocumentConverter(text_layer_only=True)
    results = list(converter.convert_all([DOCX, THREE_PAGES], progress_callback=events.append))
    assert [r.status for r in results] == [ConversionStatus.SUCCESS] * 2
    by_index = {}
    for e in events:
        by_index.setdefault(e.document_index, []).append(e)
    assert sorted(by_index) == [1, 2]
    assert [type(e) for e in by_index[1]] == [DocumentStartedProgress, DocumentCompletedProgress]
    assert by_index[1][0].document_name == "word_sample"
    assert len(pages(by_index[2])) == 3


def test_a_raising_callback_does_not_break_the_conversion(capsys):
    def boom(event):
        raise RuntimeError("observer bug")

    result = DocumentConverter(text_layer_only=True, progress_callback=boom).convert(THREE_PAGES)
    assert result.status == ConversionStatus.SUCCESS
    assert "observer bug" in capsys.readouterr().err


def test_show_progress_prints_to_stderr(capfd):
    DocumentConverter(text_layer_only=True, show_progress=True).convert(THREE_PAGES)
    err = capfd.readouterr().err
    assert "Converting skipped_1page" in err
    assert "pages 3/3" in err
    assert "Finished skipped_1page: success" in err


def test_the_module_mirrors_doclings_names():
    assert docling_rs.progress.EnrichmentStep.PICTURE_DESCRIPTION.value == "picture_description"
    assert docling_rs.progress.ConversionProgressKind.PAGE_COMPLETED.value == "page_completed"
