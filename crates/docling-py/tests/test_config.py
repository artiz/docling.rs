"""Tests for the docling-shaped configuration surface and re-exports
(declarative path only — no ML models required)."""

import io
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
HTML = REPO / "tests/data/html/sources/hyperlink_03.html"

docling_rs = pytest.importorskip("docling_rs")


def test_input_format_matches_docling_values():
    from docling_rs import InputFormat

    # docling's own members carry these exact string values.
    assert InputFormat.PDF == "pdf"
    assert InputFormat.DOCX == "docx"
    assert InputFormat.XML_JATS == "xml_jats"
    assert InputFormat.JSON_DOCLING == "json_docling"


def test_reexports_are_importable():
    import docling_rs as d

    for name in (
        "DocumentConverter",
        "ConversionResult",
        "ConversionStatus",
        "DoclingDocument",
        "ImageRefMode",
        "InputFormat",
        "DocumentStream",
        "PdfPipelineOptions",
        "PdfFormatOption",
        "AcceleratorOptions",
        "AcceleratorDevice",
        "TableFormerMode",
    ):
        assert hasattr(d, name), name


def test_pipeline_options_via_format_options_convert():
    from docling_rs import (
        DocumentConverter,
        InputFormat,
        PdfFormatOption,
        PdfPipelineOptions,
        AcceleratorOptions,
    )

    opts = PdfPipelineOptions(
        do_ocr=False,
        do_table_structure=False,
        accelerator_options=AcceleratorOptions(num_threads=2),
    )
    conv = DocumentConverter(
        format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)}
    )
    # Options are PDF-pipeline knobs; a declarative HTML convert still works.
    res = conv.convert(HTML)
    assert res.status == "success"
    assert res.document.export_to_markdown()


def test_shorthand_flags_convert():
    from docling_rs import DocumentConverter

    res = DocumentConverter(do_ocr=False, do_table_structure=True).convert(HTML)
    assert res.status == "success"


def test_document_stream_source():
    from docling_rs import DocumentConverter, DocumentStream

    stream = DocumentStream(name="hyperlink_03.html", stream=io.BytesIO(HTML.read_bytes()))
    res = DocumentConverter().convert(stream)
    assert res.status == "success"
    assert res.document.texts


def test_image_ref_mode_reexport_drives_export():
    from docling_rs import DocumentConverter, ImageRefMode

    doc = DocumentConverter().convert(HTML).document
    # docling-core's own export honours the re-exported enum.
    md = doc.export_to_markdown(image_mode=ImageRefMode.EMBEDDED)
    assert isinstance(md, str)


def test_allowed_formats_restricts_conversion():
    from docling_rs import DocumentConverter, InputFormat

    # HTML allowed → converts.
    ok = DocumentConverter(allowed_formats=[InputFormat.HTML]).convert(HTML)
    assert ok.status == "success"

    # HTML not in the allowed set → the engine refuses it.
    conv = DocumentConverter(allowed_formats=[InputFormat.PDF, InputFormat.DOCX])
    with pytest.raises(Exception):
        conv.convert(HTML)


def test_unknown_allowed_format_raises():
    from docling_rs import DocumentConverter

    with pytest.raises(Exception):
        DocumentConverter(allowed_formats=["not_a_format"])


def test_convert_all_yields_results():
    from docling_rs import DocumentConverter

    results = list(DocumentConverter().convert_all([HTML, HTML]))
    assert len(results) == 2
    assert all(r.status == "success" for r in results)


def test_convert_all_raises_on_error_false():
    from docling_rs import DocumentConverter

    missing = REPO / "tests/data/html/sources/__does_not_exist__.html"
    out = list(DocumentConverter().convert_all([HTML, missing], raises_on_error=False))
    assert len(out) == 2
    assert out[0].status == "success"
    assert out[1].status == "failure"


def test_conversion_error_type():
    from docling_rs import DocumentConverter, ConversionError

    missing = REPO / "tests/data/html/sources/__nope__.html"
    with pytest.raises(ConversionError):
        DocumentConverter().convert(missing)


def test_accelerator_device_maps_to_ep_env(monkeypatch):
    # device=cuda/cpu maps to DOCLING_RS_EP (setdefault — an explicit env
    # override wins); AUTO leaves the engine default alone (auto on the GPU
    # wheel, CPU otherwise); MPS has no provider here and warns.
    import os

    from docling_rs import (
        DocumentConverter,
        InputFormat,
        PdfFormatOption,
        PdfPipelineOptions,
        AcceleratorOptions,
        AcceleratorDevice,
    )

    def convert_with(device):
        opts = PdfPipelineOptions(accelerator_options=AcceleratorOptions(device=device))
        DocumentConverter(
            format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)}
        )

    monkeypatch.delenv("DOCLING_RS_EP", raising=False)
    convert_with(AcceleratorDevice.CUDA)
    assert os.environ["DOCLING_RS_EP"] == "cuda"

    monkeypatch.setenv("DOCLING_RS_EP", "cpu")
    convert_with(AcceleratorDevice.CUDA)  # explicit env wins over the option
    assert os.environ["DOCLING_RS_EP"] == "cpu"

    monkeypatch.delenv("DOCLING_RS_EP", raising=False)
    convert_with(AcceleratorDevice.CPU)
    assert os.environ["DOCLING_RS_EP"] == "cpu"

    monkeypatch.delenv("DOCLING_RS_EP", raising=False)
    convert_with(AcceleratorDevice.AUTO)
    assert "DOCLING_RS_EP" not in os.environ

    with pytest.warns(UserWarning, match="mps"):
        convert_with(AcceleratorDevice.MPS)


def test_initialize_pipeline_noop_for_non_ml_format():
    from docling_rs import DocumentConverter, InputFormat

    conv = DocumentConverter()
    # No models needed for a declarative format → clean no-op, and conversion
    # still works afterwards.
    conv.initialize_pipeline(InputFormat.MD)
    assert conv.convert(HTML).status == "success"


def test_no_text_panels_reaches_the_native_converter():
    """#197: the facade must accept ``no_text_panels`` — both as a direct
    kwarg and via docling-shaped ``PdfPipelineOptions`` — and still convert
    (declarative path; the flag only alters the ML pipeline, so constructing
    without a TypeError and forwarding to the native converter is the bug
    surface)."""
    from docling_rs import (
        DocumentConverter,
        InputFormat,
        PdfFormatOption,
        PdfPipelineOptions,
    )

    for converter in (
        DocumentConverter(no_text_panels=True),
        DocumentConverter(
            format_options={
                InputFormat.PDF: PdfFormatOption(
                    pipeline_options=PdfPipelineOptions(no_text_panels=True)
                )
            }
        ),
    ):
        result = converter.convert(HTML)
        assert "homepage" in result.document.export_to_markdown().lower()


def test_sparse_sheet_kwargs_forward_to_engine():
    """#274: the wrapper forwards skip_empty_cells / compact_tables to the
    native converter (they were native-only in v1.14.0). skip_empty_cells is
    structural, so it must show up in the docling-core document the wrapper
    hands back: fewer table cells on a gappy sheet."""
    from docling_rs import DocumentConverter

    xlsx = REPO / "tests/data/xlsx/sources/xlsx_07_gap_tolerance_.xlsx"

    def cell_count(converter):
        doc = converter.convert(xlsx).document
        return sum(len(t.data.table_cells) for t in doc.tables)

    dense = cell_count(DocumentConverter())
    sparse = cell_count(
        DocumentConverter(skip_empty_cells=True, compact_tables=True)
    )
    assert sparse < dense


def test_ocr_kwargs_forward_and_validate():
    """#274 follow-through for #254: ocr_mode / ocr_scale reach the native
    converter (which validates them), directly and docling-shaped via
    ocr_options.mode / .scale."""
    from types import SimpleNamespace

    from docling_rs import DocumentConverter, InputFormat, PdfFormatOption

    # Valid values construct; the native layer rejects bad ones — the error
    # surfacing at all is proof of forwarding.
    DocumentConverter(ocr_mode="full_page", ocr_scale=3.0)
    with pytest.raises(Exception):
        DocumentConverter(ocr_mode="not_a_mode")
    with pytest.raises(Exception):
        DocumentConverter(ocr_scale=-1.0)

    # docling-shaped: OcrOptions.mode may be an enum (collapses to .value).
    shaped = SimpleNamespace(
        do_ocr=True,
        do_table_structure=True,
        ocr_options=SimpleNamespace(
            mode=SimpleNamespace(value="layout_regions"), scale=2.5, lang=[]
        ),
    )
    DocumentConverter(
        format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=shaped)}
    )
    shaped.ocr_options.mode = "bogus"
    with pytest.raises(Exception):
        DocumentConverter(
            format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=shaped)}
        )


def test_ensure_env_leaves_ocr_lang_selectable(tmp_path, monkeypatch):
    """#285: ensure_env must NOT pin DOCLING_OCR_REC_ONNX/DOCLING_OCR_DICT —
    those per-file overrides beat the engine's en/ch pair selection and made
    the ocr_lang kwarg silently inert. The cache is handed over as
    DOCLING_RS_MODELS_DIR instead, and download_models() knows the English
    pair."""
    from docling_rs import models as m

    # A fake cache with the layout the downloader produces.
    cache = tmp_path / "cache"
    for rel in (
        "models/ocr_rec.onnx",
        "models/ppocr_keys_v1.txt",
        "models/ocr_rec_en.onnx",
        "models/en_dict.txt",
    ):
        p = cache / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(b"stub")

    # Run from a directory without a local .models/ (a checkout's own assets
    # outrank the cache by design and would short-circuit the test).
    monkeypatch.chdir(tmp_path)
    for var in ("DOCLING_RS_MODELS_DIR", "DOCLING_OCR_REC_ONNX", "DOCLING_OCR_DICT"):
        monkeypatch.delenv(var, raising=False)

    m.ensure_env(cache)

    import os

    assert os.environ.get("DOCLING_RS_MODELS_DIR") == str(cache / "models")
    assert "DOCLING_OCR_REC_ONNX" not in os.environ
    assert "DOCLING_OCR_DICT" not in os.environ

    # The download manifest carries the English pair (with upstream
    # fallbacks for release tags that don't host it).
    assert m._OPTIONAL["ocr_rec_en.onnx"] == "models/ocr_rec_en.onnx"
    assert m._OPTIONAL["en_dict.txt"] == "models/en_dict.txt"
    assert "models/ocr_rec_en.onnx" in m._FALLBACK_URLS
    assert "models/en_dict.txt" in m._FALLBACK_URLS


def test_ensure_env_still_respects_explicit_ocr_pins(tmp_path, monkeypatch):
    """The per-file vars remain the pin-any-model hatch (#285 workaround 2):
    ensure_env never overwrites caller-set values."""
    from docling_rs import models as m

    cache = tmp_path / "cache"
    (cache / "models").mkdir(parents=True)
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("DOCLING_OCR_REC_ONNX", "/custom/rec.onnx")
    monkeypatch.setenv("DOCLING_OCR_DICT", "/custom/dict.txt")
    monkeypatch.delenv("DOCLING_RS_MODELS_DIR", raising=False)

    m.ensure_env(cache)

    import os

    assert os.environ["DOCLING_OCR_REC_ONNX"] == "/custom/rec.onnx"
    assert os.environ["DOCLING_OCR_DICT"] == "/custom/dict.txt"


def test_pdfium_plan_selects_by_platform():
    """#299: pdfium is selected by host platform, mirroring #298's shell fix —
    Linux x64 keeps the pinned release build, everything else takes the
    matching bblanchon prebuilt with the *platform's* library name, unknown
    hosts skip."""
    from docling_rs import models as m

    assert m._pdfium_plan("Linux", "x86_64") == ("release", "libpdfium.so")
    assert m._pdfium_plan("Linux", "amd64") == ("release", "libpdfium.so")
    assert m._pdfium_plan("Linux", "aarch64") == (
        "tarball",
        f"{m._PDFIUM_TGZ_BASE}/pdfium-linux-arm64.tgz",
        "libpdfium.so",
    )
    assert m._pdfium_plan("Darwin", "arm64") == (
        "tarball",
        f"{m._PDFIUM_TGZ_BASE}/pdfium-mac-arm64.tgz",
        "libpdfium.dylib",
    )
    assert m._pdfium_plan("Darwin", "x86_64") == (
        "tarball",
        f"{m._PDFIUM_TGZ_BASE}/pdfium-mac-x64.tgz",
        "libpdfium.dylib",
    )
    assert m._pdfium_plan("Linux", "riscv64") is None
    assert m._pdfium_plan("Windows", "AMD64") is None


def test_fetch_pdfium_extracts_the_platform_member(tmp_path, monkeypatch):
    """The tarball path extracts exactly `lib/<platform lib>` into the cache
    (#299) — exercised with an in-memory tgz and a mocked Darwin host, so no
    network and no real platform dependence."""
    import io
    import tarfile

    from docling_rs import models as m

    payload = b"mach-o bytes"
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        info = tarfile.TarInfo("lib/libpdfium.dylib")
        info.size = len(payload)
        tar.addfile(info, io.BytesIO(payload))

    class FakeResponse(io.BytesIO):
        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    monkeypatch.setattr(
        m, "_pdfium_plan", lambda *a: ("tarball", "https://example.invalid/p.tgz", "libpdfium.dylib")
    )
    monkeypatch.setattr(
        m.urllib.request, "urlopen", lambda url: FakeResponse(buf.getvalue())
    )
    m._fetch_pdfium(tmp_path, progress=False, force=False)
    dest = tmp_path / ".pdfium/lib/libpdfium.dylib"
    assert dest.read_bytes() == payload

    # Idempotent: a second run must not re-download (urlopen would explode).
    monkeypatch.setattr(m.urllib.request, "urlopen", None)
    m._fetch_pdfium(tmp_path, progress=False, force=False)


def test_ensure_env_requires_the_platform_pdfium_lib(tmp_path, monkeypatch):
    """#299: PDFIUM_DYNAMIC_LIB_PATH is only exported when the cache holds the
    *platform's* library — a cache with just a wrong-platform binary is
    treated as not installed instead of being handed to dlopen."""
    import os

    from docling_rs import models as m

    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("PDFIUM_DYNAMIC_LIB_PATH", raising=False)

    cache = tmp_path / "cache"
    libdir = cache / ".pdfium/lib"
    libdir.mkdir(parents=True)

    # Wrong-platform library only (a dylib on this Linux host): stays unset.
    wrong = "libpdfium.dylib" if m._pdfium_lib_name() == "libpdfium.so" else "libpdfium.so"
    (libdir / wrong).write_bytes(b"x")
    m.ensure_env(cache)
    assert "PDFIUM_DYNAMIC_LIB_PATH" not in os.environ

    # The platform's library appears: the directory is exported.
    (libdir / m._pdfium_lib_name()).write_bytes(b"x")
    m.ensure_env(cache)
    assert os.environ["PDFIUM_DYNAMIC_LIB_PATH"] == str(libdir)


# --- #304: remote VLM pipeline ----------------------------------------------

# 1x1 red PNG: the VLM image leg needs no pdfium and no models.
_PNG = bytes(
    [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00,
        0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
        0x00, 0x00, 0x03, 0x00, 0x01, 0x6E, 0x2C, 0xDC, 0x33, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
)


def test_vlm_requires_an_endpoint_at_construction(monkeypatch):
    from docling_rs import DocumentConverter

    monkeypatch.delenv("DOCLING_RS_VLM_ENDPOINT", raising=False)
    with pytest.raises(ValueError, match="vlm_endpoint"):
        DocumentConverter(pipeline="vlm")


def test_unknown_pipeline_is_a_value_error():
    from docling_rs import DocumentConverter

    with pytest.raises(ValueError, match="unknown pipeline"):
        DocumentConverter(pipeline="magic")


def test_vlm_max_tokens_zero_is_a_value_error():
    from docling_rs import DocumentConverter

    with pytest.raises(ValueError, match="vlm_max_tokens"):
        DocumentConverter(
            pipeline="vlm",
            vlm_endpoint="http://localhost:11434/v1",
            vlm_model="granite-docling",
            vlm_max_tokens=0,
        )


def test_standard_pipeline_ignores_stray_vlm_kwargs(tmp_path):
    """The Node bindings' contract: without pipeline="vlm" the vlm_* kwargs
    are ignored, not rejected — and conversions stay fully local."""
    from docling_rs import DocumentConverter

    conv = DocumentConverter(
        vlm_endpoint="http://localhost:1/v1", vlm_model="nope"
    )
    doc = tmp_path / "d.md"
    doc.write_text("# hi\n")
    result = conv.convert(doc)
    assert "hi" in result.document.export_to_markdown()


def test_vlm_converts_an_image_through_a_stub_endpoint(tmp_path):
    """End-to-end over a local OpenAI-compatible stub (the counterpart of
    crates/docling/tests/vlm.rs::mock_openai): a PNG page goes out, the
    stub's answer comes back as the document text."""
    import json
    import threading
    from http.server import BaseHTTPRequestHandler, HTTPServer

    from docling_rs import DocumentConverter

    seen = {}

    class Stub(BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers["Content-Length"]))
            seen["path"] = self.path
            seen["body"] = json.loads(body)
            payload = json.dumps(
                {"choices": [{"message": {"role": "assistant",
                                          "content": "Hello from the VLM"}}]}
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, *args):  # keep pytest output clean
            pass

    server = HTTPServer(("127.0.0.1", 0), Stub)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        conv = DocumentConverter(
            pipeline="vlm",
            vlm_endpoint=f"http://127.0.0.1:{server.server_port}/v1",
            vlm_model="mock-docling",
            vlm_api_key="sk-test",
            vlm_max_tokens=512,
        )
        png = tmp_path / "page.png"
        png.write_bytes(_PNG)
        result = conv.convert(png)
    finally:
        server.shutdown()
        thread.join()

    assert "Hello from the VLM" in result.document.export_to_markdown()
    assert seen["path"] == "/v1/chat/completions"
    assert seen["body"]["model"] == "mock-docling"
    assert seen["body"]["max_tokens"] == 512
