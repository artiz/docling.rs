"""ZIP archives as input (#557): ``DocumentConverter.convert_archive`` yields
one item per entry — converted, skipped (with the reason) or failed — and a
broken document fails only itself. Declarative formats only, no models."""

import io
import zipfile

import pytest

docling_rs = pytest.importorskip("docling_rs")


def _bundle() -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("docs/a.md", "# A\n\nalpha text\n")
        z.writestr("b.csv", "x,y\n1,2\n")
        z.writestr("tool.exe", "MZ")
        z.writestr("inner.zip", "PK\x03\x04")
        z.writestr("broken.docx", "not a zip at all")
    return buf.getvalue()


def test_each_entry_is_its_own_item(tmp_path):
    path = tmp_path / "bundle.zip"
    path.write_bytes(_bundle())
    items = list(docling_rs.DocumentConverter().convert_archive(path))
    assert [(i.path, i.outcome) for i in items] == [
        ("docs/a.md", "converted"),
        ("b.csv", "converted"),
        ("tool.exe", "skipped"),
        ("inner.zip", "skipped"),
        ("broken.docx", "failed"),
    ]
    a = items[0]
    assert isinstance(a, docling_rs.ArchiveItem)
    assert a.result.status == "success"
    assert a.result.input.file.name == "a"
    assert "alpha text" in a.result.document.export_to_markdown()
    assert items[2].error == "unsupported file type"
    assert items[3].error == "nested archive"
    assert items[4].result is None and "docx" in items[4].error


def test_bytes_and_streams_convert_too():
    conv = docling_rs.DocumentConverter()
    from_bytes = list(conv.convert_archive(_bundle()))
    stream = docling_rs.DocumentStream(name="bundle.zip", stream=io.BytesIO(_bundle()))
    from_stream = list(conv.convert_archive(stream))
    assert [i.outcome for i in from_bytes] == [i.outcome for i in from_stream]
    assert sum(i.outcome == "converted" for i in from_bytes) == 2


def test_nested_archives_open_under_max_depth(monkeypatch):
    """#677: a ZIP inside the archive is read under ``DOCLING_RS_ZIP_MAX_DEPTH``
    (read when the converter is made), its entries as ``inner.zip/<path>``."""
    inner = io.BytesIO()
    with zipfile.ZipFile(inner, "w") as z:
        z.writestr("doc.md", "# Inner\n")
    outer = io.BytesIO()
    with zipfile.ZipFile(outer, "w") as z:
        z.writestr("a.md", "# A\n")
        z.writestr("inner.zip", inner.getvalue())
    shut = list(docling_rs.DocumentConverter().convert_archive(outer.getvalue()))
    assert [(i.path, i.outcome) for i in shut] == [("a.md", "converted"), ("inner.zip", "skipped")]
    monkeypatch.setenv("DOCLING_RS_ZIP_MAX_DEPTH", "2")
    opened = list(docling_rs.DocumentConverter().convert_archive(outer.getvalue()))
    assert [(i.path, i.outcome) for i in opened] == [
        ("a.md", "converted"),
        ("inner.zip/doc.md", "converted"),
    ]
    assert "# Inner" in opened[1].result.document.export_to_markdown()


def test_not_an_archive_raises():
    with pytest.raises(docling_rs.ConversionError):
        list(docling_rs.DocumentConverter().convert_archive(b"plain text"))


def test_plain_convert_still_rejects_zip(tmp_path):
    path = tmp_path / "bundle.zip"
    path.write_bytes(_bundle())
    with pytest.raises(docling_rs.ConversionError):
        docling_rs.DocumentConverter().convert(path)
