"""Pluggable backends (#680): Python callables registered as backends."""

import io
import json
import sys
import zipfile
from pathlib import Path

import pytest

import docling_rs
from docling_rs import DocumentConverter, DocumentStream, InputFormat

DATA = Path(__file__).parent / "data" / "markitdown"


@pytest.fixture(autouse=True)
def _clean_registry():
    saved = list(docling_rs._BACKENDS)
    docling_rs._BACKENDS.clear()
    yield
    docling_rs._BACKENDS[:] = saved


def rtf2(data: bytes, name: str) -> str:
    """A toy format: `\\h1 Title`, `\\p text` lines -> Markdown."""
    out = []
    for line in data.decode().splitlines():
        tag, _, rest = line.partition(" ")
        if tag == "\\h1":
            out.append("# " + rest)
        elif tag == "\\p":
            out.append(rest)
    return "\n\n".join(out) + "\n"


SAMPLE = b"{\\rtf2}\n\\h1 Release notes\n\\p Plugins work.\n"


def test_a_registered_callable_reads_an_unknown_format(tmp_path):
    path = tmp_path / "notes.rtf2"
    path.write_bytes(b"\x00binary\x01" + SAMPLE)  # not text: unknown to the engine alone
    with pytest.raises(docling_rs.ConversionError):
        DocumentConverter().convert(path)

    docling_rs.register_backend(rtf2, extensions=[".RTF2"])
    result = DocumentConverter().convert(path)
    assert result.status == "success"
    md = result.document.export_to_markdown()
    assert "# Release notes" in md and "Plugins work." in md
    # Bytes and streams take the same route.
    result = DocumentConverter().convert(DocumentStream(name="n.rtf2", stream=io.BytesIO(SAMPLE)))
    assert "Plugins work." in result.document.export_to_markdown()


def test_decorator_media_type_sniff_and_priority():
    seen = []

    @docling_rs.register_backend(sniff=lambda data: data.startswith(b"{\\rtf2}"), name="sniffed")
    def by_content(data: bytes, name: str) -> str:
        seen.append(name)
        return "sniffed " + name

    conv = DocumentConverter()
    md = conv.convert_bytes("blob", SAMPLE).document.export_to_markdown()
    assert md.strip() == "sniffed blob" and seen == ["blob"]

    # Per-converter registration, higher priority first.
    conv.register_backend(lambda d, n: "urgent", extensions=["rtf2"], priority=5, name="urgent")
    assert [b.name for b in conv.registered_backends()] == ["urgent", "sniffed"]
    assert conv.convert_bytes("x.rtf2", SAMPLE).document.export_to_markdown().strip() == "urgent"
    assert "rtf2" in conv.supported_extensions()

    # By media type: the Content-Type of in-memory bytes, also in the facade.
    docling_rs.register_backend(lambda d, n: "typed " + n, media_types=["application/x-rtf2"], name="typed")
    typed = DocumentConverter().convert_bytes("upload", b"plain words", media_type="application/x-rtf2; v=2")
    assert typed.document.export_to_markdown().strip() == "typed upload"
    assert DocumentConverter().convert_bytes("upload", b"plain words").document.export_to_markdown().strip() == "plain words"
    from docling_rs.markitdown import MarkItDown, StreamInfo

    out = MarkItDown().convert_stream(io.BytesIO(b"plain words"), stream_info=StreamInfo(mimetype="application/x-rtf2"))
    assert out.markdown.strip().startswith("typed"), out.markdown


def test_a_backend_can_return_a_docling_document_and_take_over_a_format():
    from docling_core.types.doc import DoclingDocument

    def as_document(data: bytes, name: str) -> DoclingDocument:
        doc = DoclingDocument(name=name)
        doc.add_title(text="From a DoclingDocument")
        doc.add_text(label="text", text=f"{len(data)} bytes of {name}")
        return doc

    conv = DocumentConverter()
    conv.register_backend(as_document, extensions=["docx"], name="docx-override")
    result = conv.convert(DATA / "test.docx")
    md = result.document.export_to_markdown()
    assert md.startswith("# From a DoclingDocument"), md
    assert "bytes of test.docx" in md

    conv.register_backend(lambda d, n: as_document(d, n).export_to_dict(), extensions=["dict"])
    md = conv.convert_bytes("x.dict", b"abc").document.export_to_markdown()
    assert "3 bytes of x.dict" in md


def test_errors_are_reported_and_bad_registrations_refused():
    def broken(data: bytes, name: str) -> str:
        raise RuntimeError("cannot read this")

    conv = DocumentConverter()
    conv.register_backend(broken, extensions=["bad"])
    with pytest.raises(docling_rs.ConversionError, match="cannot read this"):
        conv.convert_bytes("x.bad", b"x")
    conv.register_backend(lambda d, n: 42, extensions=["num"])
    with pytest.raises(docling_rs.ConversionError, match="Markdown \\(str\\) or a DoclingDocument"):
        conv.convert_bytes("x.num", b"x")
    # A failing sniff claims nothing.
    conv.register_backend(lambda d, n: "never", sniff=lambda d: 1 / 0)
    assert "never" not in conv.convert_bytes("plain.md", b"# Title\n").document.export_to_markdown()

    with pytest.raises(ValueError):
        docling_rs.register_backend(rtf2)  # claims nothing
    with pytest.raises(TypeError):
        docling_rs.register_backend("not callable", extensions=["x"])
    with pytest.raises(TypeError):
        docling_rs.register_backend(rtf2, extensions="rtf2")


def test_module_registrations_apply_to_converters_created_afterwards():
    before = DocumentConverter()
    docling_rs.register_backend(rtf2, extensions=["rtf2"])
    after = DocumentConverter()
    assert [b.name for b in docling_rs.registered_backends()] == ["rtf2"]
    assert [b.name for b in after.registered_backends()] == ["rtf2"]
    assert before.registered_backends() == []
    assert InputFormat("plugin") is InputFormat.PLUGIN
