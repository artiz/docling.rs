"""#896: ``keep_picture_images=False`` reaches the warm PDF pipeline
(``initialize_pipeline`` / a second PDF), which converts outside
``DocumentConverter.convert``'s enrichment pass and used to keep every
picture's bytes. Needs the layout model (the pictures come from it); skips
without it."""

from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
MODELS = REPO / ".models"
PDF = REPO / "tests/data/pdf/sources/picture_classification.pdf"

docling_rs = pytest.importorskip("docling_rs")


def _counts(converter):
    doc = converter.convert(PDF).document
    return len(doc.pictures), sum(1 for p in doc.pictures if p.image is not None)


@pytest.mark.parametrize("warm", [False, True])
def test_keep_picture_images_false_reaches_pdf_pictures(monkeypatch, warm):
    if not any((MODELS / m).exists() for m in ("layout_heron.onnx", "layout_heron_int8.onnx")):
        pytest.skip("ML models not fetched")
    # Tests run from the crate dir: the repo's assets, as a deployment points
    # at its own (#285).
    monkeypatch.setenv("DOCLING_RS_MODELS_DIR", str(MODELS))
    from docling_rs import DocumentConverter, InputFormat

    counts = {}
    for keep in (True, False):
        converter = DocumentConverter(do_ocr=False, keep_picture_images=keep)
        if warm:
            converter.initialize_pipeline(InputFormat.PDF)
        counts[keep] = _counts(converter)
    assert counts[True][0] > 0, "the fixture has pictures"
    assert counts[True] == (counts[True][0], counts[True][0]), "kept by default"
    assert counts[False] == (counts[True][0], 0), "dropped on request"
