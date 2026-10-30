"""#934: ``pdf_report_repairs`` reports what the PDF reader repaired to read
the input on ``result.pdf_repairs`` — off by default. The PDFs are built in
memory (text-layer path, so no models are needed)."""

import pytest

docling_rs = pytest.importorskip("docling_rs")

CONTENT = b"BT /F1 12 Tf 20 100 Td (Repaired stream text) Tj ET\n"


def _pdf(declared: int) -> bytes:
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R"
        b" /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length %d >>\nstream\n" % declared + CONTENT + b"endstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = b"%PDF-1.4\n"
    offsets = []
    for i, body in enumerate(objects):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % (i + 1) + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    for offset in offsets:
        out += b"%010d 00000 n \n" % offset
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objects) + 1,
        xref,
    )
    return out


def _convert(tmp_path, declared, **kwargs):
    path = tmp_path / "in.pdf"
    path.write_bytes(_pdf(declared))
    return docling_rs.DocumentConverter(text_layer_only=True, **kwargs).convert(path)


def test_off_by_default(tmp_path, monkeypatch):
    monkeypatch.delenv("DOCLING_RS_PDF_REPORT_REPAIRS", raising=False)
    result = _convert(tmp_path, len(CONTENT) + 7)
    assert result.pdf_repairs is None
    assert "Repaired stream text" in result.document.export_to_markdown()


def test_reports_an_overstated_length(tmp_path):
    result = _convert(tmp_path, len(CONTENT) + 7, pdf_report_repairs=True)
    [repair] = result.pdf_repairs
    assert repair["kind"] == "stream_length"
    assert repair["object"] == [4, 0]
    assert repair["declared"] == len(CONTENT) + 7
    assert repair["recovered"] == len(CONTENT) - 1
    # The conversion itself is the same.
    assert "Repaired stream text" in result.document.export_to_markdown()


def test_a_well_formed_file_reports_nothing(tmp_path):
    result = _convert(tmp_path, len(CONTENT), pdf_report_repairs=True)
    assert result.pdf_repairs == []
