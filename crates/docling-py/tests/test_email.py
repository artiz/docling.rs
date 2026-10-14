"""Attachment payloads of ``.eml`` / ``.msg`` (#561) — declarative, no ML
models: the fixtures carry a text attachment that converts as Markdown."""

import io
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
EML = REPO / "tests/data/email/sources/eml_with_attachment.eml"
MSG = REPO / "tests/data/email/sources/msg_with_attachment.msg"

docling_rs = pytest.importorskip("docling_rs")
from docling_rs import DocumentConverter, DocumentStream, EmailAttachment, InputFormat, email_attachments  # noqa: E402


def test_eml_attachment_payload_and_conversion():
    atts = email_attachments(EML)
    assert len(atts) == 1
    att = atts[0]
    assert isinstance(att, EmailAttachment)
    assert (att.index, att.name, att.content_type) == (0, "test.txt", "text/plain")
    assert att.format == InputFormat.MD
    assert att.inline is False and att.skipped is None
    assert att.data.startswith(b"This is a test attachment file.")
    assert att.size == len(att.data)
    result = DocumentConverter().convert(att.as_stream())
    assert "This is a test attachment file." in result.document.export_to_markdown()


def test_msg_attachments_match_the_eml_shape():
    atts = email_attachments(MSG)
    assert [(a.name, a.content_type, a.format, a.size) for a in atts] == [
        ("test.txt", "text/plain", InputFormat.MD, 64),
        ("report.pdf", "application/pdf", InputFormat.PDF, 26),
    ]
    assert all(a.data is not None and a.skipped is None for a in atts)


def test_bytes_and_stream_inputs_agree_with_the_path():
    data = EML.read_bytes()
    from_path = email_attachments(EML)
    assert email_attachments(data) == from_path
    assert email_attachments(DocumentStream(name="m.eml", stream=io.BytesIO(data))) == from_path


def test_limits_skip_and_drop_payloads():
    atts = email_attachments(MSG, max_entry_size=30)
    assert atts[0].skipped == "larger than the per-entry size limit" and atts[0].data is None
    assert atts[1].skipped is None and atts[1].data is not None
    with pytest.raises(ValueError):
        atts[0].as_stream()
    assert [a.skipped for a in email_attachments(MSG, max_entries=1)] == [None, "over the entry limit"]


def test_not_a_message_raises():
    with pytest.raises(docling_rs.ConversionError):
        email_attachments(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1 not really a compound file")


def _mixed_eml() -> bytes:
    """A message with a Latin-1 text attachment (base64), a PDF disposed
    `inline`, two attachments of the same name and a `scan.bin` sent as PDF."""
    import base64

    latin1 = "café".encode("latin-1")
    parts = [
        ("text/plain; charset=iso-8859-1", 'attachment; filename="note.txt"', latin1),
        ("application/pdf", 'inline; filename="report.pdf"', b"%PDF-1.4 one"),
        ("application/pdf", 'attachment; filename="report.pdf"', b"%PDF-1.4 two"),
        ("text/markdown", 'attachment; filename="scan.bin"', b"# Scan\n\nsniffed by type"),
    ]
    out = (
        "From: a@x.com\r\nSubject: S\r\nMIME-Version: 1.0\r\n"
        'Content-Type: multipart/mixed; boundary="bb"\r\n\r\n'
        "--bb\r\nContent-Type: text/plain\r\n\r\nBody.\r\n"
    )
    for ctype, disp, payload in parts:
        out += (
            f"--bb\r\nContent-Type: {ctype}\r\nContent-Disposition: {disp}\r\n"
            "Content-Transfer-Encoding: base64\r\n\r\n"
            + base64.b64encode(payload).decode()
            + "\r\n"
        )
    return (out + "--bb--\r\n").encode()


def test_payload_bytes_inline_images_only_and_unique_names():
    """#564: a text attachment keeps its own bytes (no UTF-8 re-encoding),
    `inline` marks images only, a repeated name gets a counter."""
    atts = email_attachments(_mixed_eml())
    assert [(a.name, a.inline) for a in atts] == [
        ("note.txt", False),
        ("report.pdf", False),
        ("report-2.pdf", False),
        ("scan.bin", False),
    ]
    assert atts[0].data == "café".encode("latin-1")
    assert atts[2].data == b"%PDF-1.4 two"


def test_as_stream_carries_the_detected_format():
    """#564: `scan.bin` sent as Markdown converts through `as_stream()`,
    whose `format` says what the extension cannot."""
    scan = email_attachments(_mixed_eml())[3]
    assert scan.format == InputFormat.MD
    stream = scan.as_stream()
    assert stream.format == InputFormat.MD
    result = DocumentConverter().convert(stream)
    assert "sniffed by type" in result.document.export_to_markdown()
    # A plain DocumentStream has only the name: `.bin` is no extension docling
    # knows, so the bytes are read as plain text (#671), not as Markdown.
    plain = DocumentConverter().convert(DocumentStream(name="scan.bin", stream=io.BytesIO(scan.data)))
    assert "sniffed by type" in plain.document.export_to_markdown()


def test_streams_are_rewound_before_reading():
    """#564: a DocumentStream already read once still lists its attachments."""
    stream = io.BytesIO(EML.read_bytes())
    stream.read()
    assert email_attachments(DocumentStream(name="m.eml", stream=stream)) == email_attachments(EML)
