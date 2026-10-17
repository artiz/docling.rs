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
    PRIORITY_GENERIC_FILE_FORMAT,
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
        "test_rss.xml",
        "text/xml",
        must_include=[
            # MarkItDown's RSS_TEST_STRINGS: the channel title, an item's
            # heading, its body as Markdown (#678).
            "# The Official Microsoft Blog",
            "## Ignite 2024: Why nearly 70% of the Fortune 500 now use Microsoft 365 Copilot",
            "Published on: ",
        ],
        must_not_include=["<rss", "<item>", "```xml"],
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
    with pytest.raises(TypeError, match="accepts"):
        MarkItDown().register_converter(object())
    with pytest.warns(UserWarning, match="unknown MarkItDown convert"):
        MarkItDown().convert(DATA / "test.xlsx", colour="blue")


class _Result:
    """MarkItDown's DocumentConverterResult, duck-typed (a plugin returns one)."""

    def __init__(self, markdown, title=None):
        self.markdown = markdown
        self.title = title


class _Rtf2Converter:
    """A MarkItDown DocumentConverter for a toy `.rtf2` format."""

    def __init__(self, tag="rtf2"):
        self.tag = tag
        self.calls = []

    def accepts(self, file_stream, stream_info, **kwargs):
        return (stream_info.extension or "") == ".rtf2"

    def convert(self, file_stream, stream_info, **kwargs):
        self.calls.append(sorted(kwargs))
        body = file_stream.read().decode("utf-8", "replace")
        return _Result(f"# {self.tag}\n\n{body}", title=self.tag)


def test_register_converter_runs_markitdown_converters():
    payload = b"\x89\x00\xff\xfe\x81\x82\x83 binary payload"  # neither a signature nor text
    info = StreamInfo(extension=".rtf2", filename="notes.rtf2")

    def convert(md):
        return md.convert_stream(io.BytesIO(payload), stream_info=info)

    md = MarkItDown()
    with pytest.raises((UnsupportedFormatException, FileConversionException)):
        convert(md)

    # A generic converter (priority 10) runs when the engine fails on it …
    generic = _Rtf2Converter("generic")
    md.register_converter(generic, priority=PRIORITY_GENERIC_FILE_FORMAT)
    result = convert(md)
    assert result.markdown.startswith("# generic") and result.title == "generic"
    # … a specific one (priority 0) before it, and before the engine itself,
    # the later registration first among equals.
    first = _Rtf2Converter("first")
    later = _Rtf2Converter("later")
    md.register_converter(first)
    md.register_converter(later)
    assert convert(md).markdown.startswith("# later")
    assert md.convert_stream(io.BytesIO(b"x"), stream_info=StreamInfo(extension=".rtf2")).title == "later"

    # A specific converter takes over a format the engine reads.
    class TakeOverCsv(_Rtf2Converter):
        def accepts(self, file_stream, stream_info, **kwargs):
            return stream_info.extension == ".csv"

    md.register_converter(TakeOverCsv("csv"))
    assert md.convert_stream(io.BytesIO(b"a,b\n1,2\n"), stream_info=StreamInfo(extension=".csv")).title == "csv"
    # Other formats still go to the engine.
    assert "AutoGen" in md.convert(DATA / "test.docx").markdown

    # A converter that raises is recorded; with no other taker it fails.
    class Broken(_Rtf2Converter):
        def convert(self, file_stream, stream_info, **kwargs):
            raise RuntimeError("nope")

    md2 = MarkItDown()
    md2.register_converter(Broken())
    with pytest.raises(FileConversionException) as failure:
        convert(md2)
    assert failure.value.attempts and failure.value.attempts[0].converter.__class__ is Broken


def test_enable_plugins_loads_markitdown_entry_points(monkeypatch):
    import importlib.metadata as metadata

    registered = []

    class Plugin:
        __plugin_interface_version__ = 1

        @staticmethod
        def register_converters(markitdown, **kwargs):
            registered.append(kwargs)
            markitdown.register_converter(_Rtf2Converter("plugin"))

    class EntryPoint:
        def __init__(self, name, target):
            self.name, self._target = name, target

        def load(self):
            if isinstance(self._target, Exception):
                raise self._target
            return self._target

    class EntryPoints(list):
        def select(self, group):
            return self if group == "markitdown.plugin" else []

    monkeypatch.setattr(
        metadata, "entry_points",
        lambda: EntryPoints([EntryPoint("good", Plugin), EntryPoint("bad", ImportError("missing dep"))]),
    )
    with pytest.warns(UserWarning, match="'bad' failed to load: missing dep"):
        md = MarkItDown(enable_plugins=True)
    assert registered == [{}]
    out = md.convert_stream(io.BytesIO(b"body"), stream_info=StreamInfo(extension=".rtf2"))
    assert out.markdown == "# plugin\n\nbody"
    with pytest.warns(UserWarning, match="already enabled"):
        md.enable_plugins()


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


def _timedtext_server(vtt_by_lang):
    """YouTube's `timedtext` endpoint on 127.0.0.1 (#681): the WebVTT for
    `lang=`, 404 otherwise."""
    import http.server
    import threading
    from urllib.parse import parse_qs, urlparse

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            query = parse_qs(urlparse(self.path).query)
            body = None
            if urlparse(self.path).path == "/api/timedtext" and query.get("fmt") == ["vtt"]:
                body = vtt_by_lang.get(query.get("lang", [""])[0])
            if body is None:
                self.send_response(404)
                self.end_headers()
                return
            data = body.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/vtt; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}"


YOUTUBE_DATA = Path(__file__).resolve().parents[2] / "docling" / "tests" / "data" / "youtube"


def test_youtube_url_is_title_metadata_and_transcript(md, monkeypatch):
    """#681, MarkItDown's `_youtube_converter` shape: a YouTube watch URL
    whose HTML is a watch page gives `# YouTube`, the title, metadata,
    description and the caption transcript; the title is the video's."""
    monkeypatch.setenv("DOCLING_RS_ALLOW_PRIVATE_IP_FETCH", "1")
    server, base = _timedtext_server({
        "en": (YOUTUBE_DATA / "captions_en.vtt").read_text(),
        "de": (YOUTUBE_DATA / "captions_de.vtt").read_text(),
    })
    try:
        page = (YOUTUBE_DATA / "watch.html").read_text().replace(
            "https://www.youtube.com/api/timedtext", base + "/api/timedtext"
        ).encode("utf-8")
        info = StreamInfo(mimetype="text/html", charset="utf-8", url="https://www.youtube.com/watch?v=x7Zq9KfR2bE")
        result = md.convert_stream(io.BytesIO(page), stream_info=info)
        assert result.title == "Introducing docling.rs"
        assert result.markdown.startswith(
            "# YouTube\n\n## Introducing docling.rs\n\n### Video Metadata\n\n- **Views:** 1234\n"
            "- **Keywords:** docling, rust, document conversion\n- **Runtime:** PT1M5S\n\n### Description\n"
        ), result.markdown
        assert "### Transcript\n\ndocling.rs is one binary that converts\n" in result.markdown
        assert "ytInitialPlayerResponse" not in result.markdown
        # The language from docling_options, as MarkItDown's youtube_transcript_languages.
        german = MarkItDown(docling_options={"asr_lang": "de"}).convert_stream(io.BytesIO(page), stream_info=info)
        assert "### Transcript\n\ndocling.rs ist eine Binärdatei" in german.markdown, german.markdown
        # No captions: a clear error (MarkItDown would return the metadata alone).
        silent = (YOUTUBE_DATA / "watch_no_captions.html").read_bytes()
        with pytest.raises(FileConversionException, match="has no captions"):
            md.convert_stream(io.BytesIO(silent), stream_info=StreamInfo(mimetype="text/html", url="https://www.youtube.com/watch?v=q2WkY8rPz3M"))
        # The same page saved to disk is HTML, not a transcript.
        plain = md.convert_stream(io.BytesIO(page), stream_info=StreamInfo(mimetype="text/html", extension=".html"))
        assert "### Transcript" not in plain.markdown
    finally:
        server.shutdown()


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
    # MarkItDown's own `test_files.zip` vector (#677): a `## File:` section
    # per entry; `test.jpg` needs the image models and is skipped without them.
    result = md.convert(DATA / "test_files.zip")
    assert result.markdown.startswith("Content from the zip file `")
    for expected in [
        "## File: test.docx",
        "AutoGen: Enabling Next-Gen LLM Applications via Multi-Agent Conversation",
        "314b0a30-5b04-470b-b9f7-eed2c2bec74a",
        "## File: test.xlsx",
        "6ff4173b-42a5-4784-9b19-f49caff4d93d",
        "## File: test_wikipedia.html",
        "Microsoft was founded by [Bill Gates](/wiki/Bill_Gates)",
    ]:
        assert expected in result.markdown, expected


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
