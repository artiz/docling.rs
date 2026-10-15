"""docling_rs.markitdown — the MarkItDown-compatible facade (#674).

The vectors are MarkItDown's own (``packages/markitdown/tests/_test_vectors.py``,
microsoft/markitdown@4cc9fa1, MIT — the files in ``data/markitdown/`` come from
the same commit), ported for docling.rs: every content check is kept; checks of
MarkItDown's *markup* (its email header block, ``*`` bullets, ``>`` quotes, its
truncated ``data:image/png;base64...`` placeholder) are dropped, each with the
reason, because byte-identical Markdown is a non-goal. The source modes are
MarkItDown's ``test_module_vectors.py`` ones, minus the live-HTTP test (a fake
response / session stands in).
"""

import base64
import io
import warnings
import zipfile
from dataclasses import FrozenInstanceError, dataclass, field
from pathlib import Path
from typing import List

import pytest

from docling_rs.markitdown import (
    DocumentConverterResult,
    FileConversionException,
    MarkItDown,
    StreamInfo,
    UnsupportedFormatException,
    main,
    sniff_extension,
)

DATA = Path(__file__).parent / "data" / "markitdown"


@dataclass
class Vector:
    filename: str
    mimetype: str
    charset: str = None
    must_include: List[str] = field(default_factory=list)
    must_not_include: List[str] = field(default_factory=list)


VECTORS = [
    Vector(
        "test.docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        must_include=[
            "314b0a30-5b04-470b-b9f7-eed2c2bec74a",
            "49e168b7-d2ae-407f-a055-2167576f39a1",
            "## d666f1f7-46cb-42bd-9a39-9a39cf2a509f",
            "# Abstract",
            "# Introduction",
            "AutoGen: Enabling Next-Gen LLM Applications via Multi-Agent Conversation",
            # dropped: "data:image/png;base64..." — MarkItDown's truncated image
            # URI; docling writes a <!-- image --> placeholder.
        ],
        must_not_include=["data:image/png;base64,iVBORw0KGgoAAAANSU"],
    ),
    Vector(
        "test.pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        must_include=[
            "2cdda5c8-e50e-4db4-b5f0-9722a649f455",
            "04191ea8-5c73-4215-a1d3-1cfb43aaaf12",
            "1b92870d-e3b5-4e65-8153-919f4ff45592",
            "AutoGen: Enabling Next-Gen LLM Applications via Multi-Agent Conversation",
            "a3f6004b-6f4f-4ea8-bee3-3741f4dc385f",  # chart title
            "2003",  # chart value
            # Picture alt text (#676): MarkItDown's `![alt](Picture4.jpg)`
            # is the placeholder's text line here (the file name is not kept).
            "This phrase of the caption is Human-written.",
            "44bf7d06-5e7a-4a",
        ],
        must_not_include=["data:image/jpeg;base64,/9j/4AAQSkZJRgABAQE"],
    ),
    Vector(
        "test.xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        must_include=[
            "## 09060124-b5e7-4717-9d07-3c046eb",  # the sheet-name heading
            "6ff4173b-42a5-4784-9b19-f49caff4d93d",
            "affc7dad-52dc-4b98-9b5d-51e65d8a8ad0",
        ],
    ),
    Vector(
        "test.xls",
        "application/vnd.ms-excel",
        must_include=[
            # dropped: "## 09060124-…" — the XLS backend's document carries
            # no sheet groups to name (the XLSX one does).
            "6ff4173b-42a5-4784-9b19-f49caff4d93d",
            "affc7dad-52dc-4b98-9b5d-51e65d8a8ad0",
        ],
    ),
    Vector(
        "test.pdf",
        "application/pdf",
        must_include=["While there is contemporaneous exploration of multi-agent approaches"],
    ),
    Vector(
        "test_outlook_msg.msg",
        "application/vnd.ms-outlook",
        must_include=[
            # dropped: MarkItDown's "# Email Message" / "**From:** …" /
            # "## Content" block — docling titles the message with its
            # subject and writes plain "From: …" lines; the content is kept.
            "test.sender@example.com",
            "test.recipient@example.com",
            "# Test Email Message",
            "This is the body of the test email message",
        ],
    ),
    Vector(
        "test_mskanji.csv",
        "text/csv",
        charset="cp932",
        must_include=[
            "| 名前 | 年齢 | 住所 |",
            "| 佐藤太郎 | 30 | 東京 |",
            "| 三木英子 | 25 | 大阪 |",
            "| 髙橋淳 | 35 | 名古屋 |",
            # dropped: "| --- | --- | --- |" — docling-core's delimiter row
            # is "| - | - | - |" (equally valid GFM).
        ],
    ),
    Vector(
        "test.epub",
        "application/epub+zip",
        must_include=[
            "**Title:** Test EPUB Document",
            "**Authors:** Test Author",
            "**Description:** A test EPUB document for MarkItDown testing",
            "# Chapter 1: Test Content",
            "This is a **test** paragraph with some formatting",
            "A bullet point",  # MarkItDown: "* A bullet point"; docling: "- "
            "Another point",
            "# Chapter 2: More Content",
            "*different* style",
            "This is a blockquote for testing",  # MarkItDown prefixes "> "
        ],
    ),
]


@pytest.fixture(scope="module")
def md():
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")  # the PDF text-layer notice, if models are absent
        yield MarkItDown()


def _check(result, v):
    assert isinstance(result, DocumentConverterResult)
    for s in v.must_include:
        assert s in result.markdown, f"{v.filename}: missing {s!r}"
    for s in v.must_not_include:
        assert s not in result.markdown, f"{v.filename}: unexpected {s!r}"
    assert result.text_content == result.markdown == str(result)


class FakeResponse:
    """The ``requests.Response`` surface ``convert_response`` reads."""

    def __init__(self, url, content, headers):
        self.url = url
        self.content = content
        self.headers = headers

    def raise_for_status(self):
        pass


class FakeSession:
    def __init__(self, response):
        self.response = response
        self.urls = []

    def get(self, url, stream=False):
        self.urls.append(url)
        return self.response


MODES = ["local", "path", "stream_hints", "stream_no_hints", "file_uri", "data_uri", "response", "session"]


@pytest.mark.parametrize("mode", MODES)
@pytest.mark.parametrize("v", VECTORS, ids=lambda v: v.filename)
def test_vectors(md, v, mode):
    local = DATA / v.filename
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        if mode == "local":
            result = md.convert(str(local))
        elif mode == "path":
            result = md.convert(local)
        elif mode == "stream_hints":
            info = StreamInfo(extension=local.suffix, mimetype=v.mimetype, charset=v.charset)
            with open(local, "rb") as fh:
                result = md.convert(fh, stream_info=info)
        elif mode == "stream_no_hints":
            # #675: the engine guesses a non-Latin charset itself, so a
            # cp932 stream converts with or without charset_normalizer.
            with open(local, "rb") as fh:
                result = md.convert(fh)
        elif mode == "file_uri":
            result = md.convert(local.as_uri())
        elif mode == "data_uri":
            data = base64.b64encode(local.read_bytes()).decode()
            charset = f";charset={v.charset}" if v.charset else ""
            result = md.convert(f"data:{v.mimetype}{charset};base64,{data}")
        elif mode == "response":
            ctype = v.mimetype + (f"; charset={v.charset}" if v.charset else "")
            resp = FakeResponse(
                "https://example.com/download?id=1",
                local.read_bytes(),
                {"content-type": ctype, "content-disposition": f'attachment; filename="{v.filename}"'},
            )
            result = md.convert_response(resp)
        else:  # session: convert_uri goes through the requests_session
            ctype = v.mimetype + (f"; charset={v.charset}" if v.charset else "")
            session = FakeSession(FakeResponse(f"https://example.com/files/{v.filename}", local.read_bytes(), {"content-type": ctype}))
            with warnings.catch_warnings():
                warnings.simplefilter("ignore")
                result = MarkItDown(requests_session=session).convert(f"https://example.com/files/{v.filename}")
            assert session.urls == [f"https://example.com/files/{v.filename}"]
    _check(result, v)


def test_result_api_and_title(md):
    result = md.convert(DATA / "test.pptx")
    assert result.title == "AutoGen: Enabling Next-Gen LLM Applications via Multi-Agent Conversation"
    assert result.document is not None  # docling.rs extra: the DoclingDocument
    result.text_content = "replaced"
    assert result.markdown == "replaced" == str(result)
    plain = DocumentConverterResult("x")
    assert plain.title is None and plain.document is None


def test_stream_info_copy_and_update():
    base = StreamInfo(extension=".pdf", filename="a.pdf")
    updated = base.copy_and_update(StreamInfo(mimetype="application/pdf", extension=None), charset="utf-8")
    assert updated == StreamInfo(extension=".pdf", filename="a.pdf", mimetype="application/pdf", charset="utf-8")
    with pytest.raises(FrozenInstanceError):
        base.extension = ".docx"  # frozen, as MarkItDown's


def test_keep_data_uris(md):
    result = md.convert(DATA / "test.docx", keep_data_uris=True)
    assert "data:image/png;base64,iVBORw0KGgoAAAANSU" in result.markdown
    assert "<!-- image -->" in md.convert(DATA / "test.docx").markdown


def test_sources_and_errors(md):
    with pytest.raises(UnsupportedFormatException):
        md.convert_stream(io.BytesIO(bytes(range(256)) * 8))
    with pytest.raises(TypeError):
        md.convert(io.StringIO("text stream"))
    with pytest.raises(TypeError):
        md.convert(42)
    with pytest.raises(ValueError):
        md.convert_uri("ftp://example.com/file.pdf")
    # A wrong hint falls back to the content.
    with open(DATA / "test.docx", "rb") as fh:
        result = md.convert_stream(fh, stream_info=StreamInfo(extension=".bin"))
    assert "314b0a30-5b04-470b-b9f7-eed2c2bec74a" in result.markdown
    # A failing conversion of a supported format is FileConversionException.
    with pytest.raises(FileConversionException):
        md.convert_stream(io.BytesIO(b"PK\x03\x04 not really a docx"), stream_info=StreamInfo(extension=".docx"))


def test_options_warn():
    with pytest.warns(UserWarning, match="style_map"):
        MarkItDown(style_map="p => h1")
    with pytest.warns(UserWarning, match="unknown MarkItDown option 'colour'"):
        MarkItDown(colour="blue")
    with pytest.warns(UserWarning, match="plugins"):
        MarkItDown(enable_plugins=True)
    with pytest.raises(NotImplementedError, match="680"):
        MarkItDown().register_converter(object())
    with pytest.warns(UserWarning, match="unknown MarkItDown convert"):
        MarkItDown().convert(DATA / "test.xlsx", colour="blue")


class FakeLlm:
    """An OpenAI-style client: ``client.chat.completions.create(...)``."""

    def __init__(self):
        self.calls = []
        self.chat = self
        self.completions = self

    def create(self, model, messages):
        self.calls.append((model, messages))

        class _Msg:
            content = f"a caption by {model}"

        class _Choice:
            message = _Msg()

        class _Resp:
            choices = [_Choice()]

        return _Resp()


def test_llm_picture_captions():
    llm = FakeLlm()
    md = MarkItDown(llm_client=llm, llm_model="vision-1", llm_prompt="Describe it.")
    result = md.convert(DATA / "test.docx")
    assert llm.calls, "every picture goes to the model"
    model, messages = llm.calls[0]
    content = messages[0]["content"]
    assert model == "vision-1" and content[0] == {"type": "text", "text": "Describe it."}
    assert content[1]["image_url"]["url"].startswith("data:image/png;base64,")
    assert "a caption by vision-1" in result.markdown
    # Per-call options override the constructor's, as in MarkItDown.
    result = md.convert(DATA / "test.docx", llm_model="vision-2")
    assert "a caption by vision-2" in result.markdown
    # No llm_model, no captions.
    assert "a caption" not in MarkItDown(llm_client=FakeLlm()).convert(DATA / "test.docx").markdown


def test_zip(md, tmp_path):
    archive = tmp_path / "bundle.zip"
    with zipfile.ZipFile(archive, "w") as zf:
        zf.write(DATA / "test.docx", "docs/test.docx")
        zf.write(DATA / "test.xlsx", "sheets/test.xlsx")
        zf.writestr("notes.unknown", b"\x00\x01\x02")
    result = md.convert(archive)
    assert result.markdown.startswith("Content from the zip file `bundle.zip`:")
    assert "## File: docs/test.docx" in result.markdown
    assert "## File: sheets/test.xlsx" in result.markdown
    assert "314b0a30-5b04-470b-b9f7-eed2c2bec74a" in result.markdown
    assert "6ff4173b-42a5-4784-9b19-f49caff4d93d" in result.markdown
    assert "notes.unknown" not in result.markdown


def test_cli(tmp_path, capsys):
    out = tmp_path / "out.md"
    assert main(["-o", str(out), str(DATA / "test.xlsx")]) == 0
    assert "## 09060124-b5e7-4717-9d07-3c046eb" in out.read_text(encoding="utf-8")
    assert main(["-c", "cp932", str(DATA / "test_mskanji.csv")]) == 0
    assert "| 佐藤太郎 | 30 | 東京 |" in capsys.readouterr().out


@pytest.mark.parametrize(
    "name, expected",
    [
        ("test.pdf", ".pdf"),
        ("test.docx", ".docx"),
        ("test.pptx", ".pptx"),
        ("test.xlsx", ".xlsx"),
        ("test.xls", ".xls"),
        ("test.epub", ".epub"),
        ("test_outlook_msg.msg", ".msg"),
    ],
)
def test_sniff_files(name, expected):
    assert sniff_extension((DATA / name).read_bytes()) == expected


def test_sniff_text_and_images():
    assert sniff_extension(b"\x89PNG\r\n\x1a\n" + b"\0" * 32) == ".png"
    assert sniff_extension(b"\xff\xd8\xff\xe0" + b"\0" * 32) == ".jpg"
    assert sniff_extension(b"<!DOCTYPE html><html><body>x</body></html>") == ".html"
    assert sniff_extension(b"a,b,c\n1,2,3\n4,5,6\n") == ".csv"
    assert sniff_extension(b"a\tb\n1\t2\n") == ".tsv"
    assert sniff_extension(b"BMW,Audi\n1,2\n") == ".csv"  # not a bitmap
    assert sniff_extension(b"Hello, world\nfoo\n") == ".txt"
    assert sniff_extension(bytes(range(256)) * 8) is None
