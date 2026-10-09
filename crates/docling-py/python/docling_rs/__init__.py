"""docling.rs — Rust docling port, Python bindings.

A strangler-fig drop-in for Python docling's common path::

    from docling_rs import DocumentConverter          # was: from docling.document_converter import ...

    result = DocumentConverter().convert("document.pdf")
    print(result.document.export_to_markdown())
    data = result.document.export_to_dict()            # docling JSON wire format

Only the *document processor* is Rust. The Rust engine parses the input and
returns docling-core's JSON wire format; this module loads that into the genuine
``docling_core.types.doc.DoclingDocument``, so every downstream capability —
``export_to_markdown()`` / ``export_to_dict()`` / ``export_to_doctags()``, the
serializers, and the chunkers — is docling's own Python code, unchanged.

Configuration follows docling's shape — ``PdfPipelineOptions`` / ``PdfFormatOption``
and per-call kwargs::

    from docling_rs import DocumentConverter, InputFormat, PdfFormatOption, PdfPipelineOptions

    opts = PdfPipelineOptions(do_ocr=False, do_table_structure=True)
    conv = DocumentConverter(format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)})

One-time model setup (mirrors docling's artifact download; ~700 MB into
``~/.cache/docling.rs``)::

    import docling_rs; docling_rs.download_models()

Declarative formats (DOCX/HTML/XLSX/…) need no models at all.
"""

from __future__ import annotations

import enum
import io
import os
import warnings
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, Iterable, Iterator, List, Optional, Tuple, Union

from docling_core.types.doc import DoclingDocument, ImageRefMode

from . import models
from .models import cache_dir, download_models, ensure_env
from .options import (
    AcceleratorDevice,
    AcceleratorOptions,
    DocumentStream,
    HeadingHierarchyOptions,
    InputFormat,
    PdfFormatOption,
    PdfPipelineOptions,
    TableFormerMode,
    TableStructureOptions,
)
from . import chunking

# GPU wheels (PyPI ``docling-rs-cuda``) bundle ONNX Runtime's CUDA provider
# libraries next to `_native` in this package directory; the native module is
# linked with an `$ORIGIN` rpath, so ONNX Runtime's dlopen-by-name finds them
# there with no Python-side setup — the same mechanism the native CLI uses.
# (Deliberately NO ctypes preload here: loading the provider libraries with
# RTLD_GLOBAL before the static ORT inside `_native` initializes duplicates
# ORT symbols process-wide and segfaulted at session creation in testing.)
from ._native import (
    ConversionError,
    EncryptionError,
    PasswordRequiredError,
    WrongPasswordError,
    __version__,
)
from ._native import DocumentConverter as _NativeDocumentConverter
from ._native import compiled_providers as _compiled_providers
from ._native import email_attachments as _email_attachments

__all__ = [
    "DocumentConverter",
    "ConversionResult",
    "ArchiveItem",
    "ConversionStatus",
    "ErrorItem",
    "ConversionError",
    "EncryptionError",
    "PasswordRequiredError",
    "WrongPasswordError",
    "InputDocument",
    "DoclingDocument",
    "ImageRefMode",
    # email attachment payloads (.eml / .msg)
    "EmailAttachment",
    "email_attachments",
    # docling-shaped configuration
    "InputFormat",
    "DocumentStream",
    "PdfPipelineOptions",
    "PdfFormatOption",
    "TableStructureOptions",
    "TableFormerMode",
    "AcceleratorOptions",
    "AcceleratorDevice",
    # Rust-native chunkers (docling_rs.chunking.HierarchicalChunker / HybridChunker)
    "chunking",
    # model / env helpers
    "download_models",
    "ensure_env",
    "cache_dir",
    "models",
    "__version__",
]


class ConversionStatus(str, enum.Enum):
    """docling's ``ConversionStatus`` (a subset). A ``str`` enum, so both
    ``result.status == "success"`` and ``result.status == ConversionStatus.SUCCESS``
    hold — matching how docling callers branch on the result."""

    SUCCESS = "success"
    PARTIAL_SUCCESS = "partial_success"
    FAILURE = "failure"


@dataclass(frozen=True)
class ArchiveItem:
    """One entry of a ZIP archive converted with
    :meth:`DocumentConverter.convert_archive` (#557). ``outcome`` is
    ``"converted"`` (``result`` holds the :class:`ConversionResult`),
    ``"skipped"`` (``error`` says why: unsupported type, nested archive,
    unsafe path, over a ``DOCLING_RS_ZIP_MAX_*`` limit) or ``"failed"``
    (``error`` is the conversion error — the other entries are unaffected)."""

    path: str
    outcome: str
    result: Optional["ConversionResult"] = None
    error: Optional[str] = None


@dataclass(frozen=True)
class InputDocument:
    """docling's ``ConversionResult.input`` shim: the source's file name/path."""

    file: Path


@dataclass(frozen=True)
class ErrorItem:
    """docling's ``ErrorItem``: one recorded problem of a conversion that still
    produced a document (``component_type``, ``module_name``,
    ``error_message``). Today the one recorded problem is a spent
    ``document_timeout`` (#497)."""

    component_type: str
    module_name: str
    error_message: str


class ConversionResult:
    """docling's ``ConversionResult``: ``.document`` (a genuine
    :class:`~docling_core.types.doc.DoclingDocument`), ``.status``,
    ``.input`` and ``.errors`` (non-empty exactly when the status is
    ``PARTIAL_SUCCESS``)."""

    def __init__(
        self,
        status: str,
        input_name: str,
        document: DoclingDocument,
        errors: Iterable[ErrorItem] = (),
    ):
        self.status = ConversionStatus(status)
        self.document = document
        self.input = InputDocument(file=Path(input_name))
        self.errors = list(errors)


class DocumentConverter:
    """docling-shaped converter whose processor is Rust.

    Parameters mirror docling's converter and ``PdfPipelineOptions``:

    * ``format_options`` — ``{InputFormat.PDF: PdfFormatOption(pipeline_options=...)}``,
      as in docling. The PDF/image pipeline options ``do_ocr``,
      ``do_table_structure`` and ``accelerator_options.num_threads`` take effect.
    * ``do_ocr`` / ``do_table_structure`` — a shorthand for the same, used when no
      ``format_options`` is given.
    * ``no_text_panels`` — PDF/image: keep every detected picture as a picture
      (disable the demotion of uncaptioned dense-text "picture" regions into
      paragraphs — the image-extraction escape hatch, #174).
    * ``heading_hierarchy`` — PDF/image: infer section-header levels after
      assembly (docling's ``HeadingHierarchyModel``, #302 — bookmarks >
      numbering > font style). Also accepted docling-shaped, via
      ``pipeline_options.heading_hierarchy_options.enabled``.
    * ``fetch_images`` — resolve remote/local ``<img src>`` for HTML/EPUB.
    * ``use_web_browser`` — render HTML via headless Chrome before parsing.
    * ``ocr_mode`` / ``ocr_scale`` — docling 2.116's ``OcrMode`` (which regions
      feed the OCR; ``"full_page"``/``"layout_regions"`` discard the embedded
      text layer) and ``OcrOptions.scale`` (OCR input resolution in px per PDF
      point), #254. Also accepted docling-shaped, via
      ``pipeline_options.ocr_options.mode`` / ``.scale``.
    * ``ocr_engine`` — which OCR engine reads scanned pages (#460):
      ``"ppocr"`` (default, the built-in PP-OCRv3 recognizer) or
      ``"tesseract"`` (the system ``tesseract`` binary, docling's
      ``TesseractCliOcrOptions``). Under Tesseract ``ocr_lang`` is its
      language list — tessdata stems (``"deu+fra"``) or BCP-47 tags. Also
      accepted docling-shaped: a ``TesseractCliOcrOptions`` /
      ``TesseractOcrOptions`` as ``pipeline_options.ocr_options`` selects
      the engine, its ``lang`` list joins with ``+``, and its
      ``tesseract_cmd`` / ``path`` / ``psm`` seed ``DOCLING_TESSERACT`` /
      ``DOCLING_RS_TESSDATA_DIR`` / ``DOCLING_RS_TESSERACT_PSM`` (process-wide,
      an explicit environment override wins).
    * ``skip_empty_cells`` / ``compact_tables`` — sparse-spreadsheet output
      controls (docling.rs extensions, #271): omit empty cells from XLSX/XLS
      table rows; render Markdown tables unpadded (all formats). Note
      ``compact_tables`` shapes the *engine's* Markdown serializer only —
      this wrapper's ``document.export_to_markdown()`` runs upstream Python
      docling-core, whose padded table style is untouched;
      ``skip_empty_cells`` is structural and carries through everywhere.
      Likewise page breaks: ``document.export_to_markdown(page_break_placeholder=…)``
      is upstream docling-core's own option here and works off the items'
      ``prov.page_no`` in the engine's JSON (PDF and DjVu pages, slides, sheets); the
      engine-side ``--page-break-placeholder`` of the CLI / serve / Node is
      not a kwarg of this wrapper.
    * ``text_layer_only`` — skip the whole PDF ML stack and read the embedded
      text layer only (the docling.rs fast path behind the CLI's ``--text-layer-only``).
    * ``list_attachments`` — email (.eml/.msg): append an Attachments section
      with names and content types (#251; the payload is never embedded).
    * ``ebcdic_layout`` — EBCDIC (#252): the copybook layout, inline
      ``EbcdicLayout`` JSON or a file path (default: the ``<stem>.layout.json``
      sidecar next to the source).
    * ``video_frames`` — max frames sampled from a video (0 = transcript only;
      needs the ``ffmpeg`` binary); default 8.
    * ``xbrl_taxonomy`` — XBRL: the directory the instance's taxonomy is read
      from (docling's ``XBRLBackendOptions.taxonomy``); default: the instance's
      own directory.
      These five reached the native class before but not this wrapper; every
      surface now shares one option set (#577, ``docs/OPTIONS.md``).
    * ``allowed_formats`` — restrict conversion to these :class:`InputFormat`\\ s
      (docling's converter arg); a source of any other format raises.
    * ``asr_model`` — the speech-recognition model for audio/video, by preset
      name (docling's ``asr_model_specs``): ``None`` / ``"whisper_tiny"``
      (default, multilingual Whisper tiny), ``"whisper_tiny_en"``,
      ``"whisper_base_en"``, ``"whisper_small_en"``,
      ``"whisper_distil_small_en"``, or ``"parakeet_tdt_0.6b_v3"`` (NVIDIA
      Parakeet TDT 0.6B v3: 25 European languages detected per utterance,
      ``asr_lang`` does not apply). Fetch the files once with
      ``download_models(asr_model=…)``; an unknown name, or a preset whose
      files are missing, fails the conversion with a message naming them.
    * ``asr_lang`` — transcription language for audio/video: a Whisper code
      (``"en"``, ``"de"``, …) or ``"auto"`` (default) to detect it from the
      first 30 seconds (docling 2.116 parity).
    * ``encoding`` — character encoding of text inputs (Markdown, CSV,
      AsciiDoc, WebVTT, XML, …): docling's ``TextBackendOptions.encoding``
      (``MarkdownBackendOptions(encoding="shift_jis")``), a WHATWG label or
      Python codec name. ``None`` (default) detects — BOM, UTF-8, then
      windows-1252; bytes the requested encoding cannot decode raise.
    * ``pipeline`` — ``"standard"`` (default) or ``"vlm"`` (#304): convert
      PDF / image inputs by sending each page to a remote OpenAI-compatible
      vision model instead of the local ML stack (no models needed). The
      ``vlm_*`` kwargs mirror the Node bindings' options and fall back to the
      ``DOCLING_RS_VLM_*`` environment: ``vlm_endpoint`` and ``vlm_model``
      are required (a missing one raises ``ValueError`` here, at
      construction); ``vlm_api_key`` (Bearer token), ``vlm_prompt`` and
      ``vlm_max_tokens`` (default 8192) are optional. With
      ``pipeline="standard"`` the ``vlm_*`` kwargs are ignored, not rejected.
    * ``document_timeout`` — docling's ``PipelineOptions.document_timeout``
      (#497): a per-document budget in seconds for the PDF pipeline, checked
      between pages. Once spent, the pages done so far are the document and
      the result is a ``PARTIAL_SUCCESS`` whose ``.errors`` says why. ``None``
      (default) is unlimited. Also accepted docling-shaped, via
      ``pipeline_options.document_timeout``.
    * ``password`` — the password of an encrypted PDF (#611; docling's
      ``--pdf-password``) or Office document — ``.docx``/``.xlsx``/
      ``.pptx``/``.doc``/``.xls``/``.ppt`` (#625, beyond docling, which
      reads none). ``pdf_password``, its earlier name, still works. Also
      accepted docling-shaped, as the PDF ``PdfFormatOption``'s
      ``backend_options.password`` (a plain string or a pydantic
      ``SecretStr``).
    * ``page_range`` — ``(first, last)``, a 1-based inclusive PDF page window
      for every conversion (#518). docling takes it per call —
      ``convert(source, page_range=(a, b))`` — which works here too and wins
      over this default.
    * ``images_scale`` / ``generate_page_images`` — docling's
      ``PdfPipelineOptions`` fields of the same names (#520): picture crops
      (and page images) at ``images_scale`` px per PDF point, and each page's
      render kept as ``document.pages[n].image`` so docling-core's
      ``TableItem.get_image`` / ``FormulaItem.get_image`` work. Also accepted
      docling-shaped via ``pipeline_options``, where ``images_scale`` applies
      once ``generate_picture_images`` or ``generate_page_images`` is set
      (docling only renders images then; without either, crops keep the
      engine's 2.0 px/pt render). Every picture's ``image.dpi`` is
      72·scale (#519).
    * ``artifacts_path`` — override the model cache dir (docling's
      ``artifacts_path``); defaults to ``~/.cache/docling.rs``.
    """

    def __init__(
        self,
        format_options: Optional[Dict[InputFormat, PdfFormatOption]] = None,
        *,
        allowed_formats: Optional[Iterable[InputFormat]] = None,
        do_ocr: bool = True,
        do_table_structure: bool = True,
        force_full_page_ocr: bool = False,
        no_text_panels: bool = False,
        heading_hierarchy: bool = False,
        do_picture_classification: bool = False,
        do_code_enrichment: bool = False,
        do_formula_enrichment: bool = False,
        do_picture_ocr: bool = False,
        picture_ocr_classes: Optional[Union[str, Iterable[str]]] = None,
        picture_ocr_min_side: Optional[int] = None,
        keep_picture_images: bool = True,
        fetch_images: bool = False,
        use_web_browser: bool = False,
        ocr_lang: Optional[str] = None,
        ocr_mode: Optional[str] = None,
        ocr_scale: Optional[float] = None,
        ocr_engine: Optional[str] = None,
        skip_empty_cells: bool = False,
        compact_tables: bool = False,
        asr_model: Optional[str] = None,
        asr_lang: Optional[str] = None,
        encoding: Optional[str] = None,
        pipeline: Optional[str] = None,
        vlm_endpoint: Optional[str] = None,
        vlm_model: Optional[str] = None,
        vlm_api_key: Optional[str] = None,
        vlm_prompt: Optional[str] = None,
        vlm_max_tokens: Optional[int] = None,
        document_timeout: Optional[float] = None,
        password: Optional[str] = None,
        pdf_password: Optional[str] = None,
        page_range: Optional[Tuple[int, int]] = None,
        images_scale: Optional[float] = None,
        generate_page_images: bool = False,
        text_layer_only: bool = False,
        list_attachments: bool = False,
        ebcdic_layout: Optional[str] = None,
        video_frames: Optional[int] = None,
        xbrl_taxonomy: Optional[Union[str, "os.PathLike[str]"]] = None,
        artifacts_path=None,
    ):
        ensure_env(artifacts_path)

        if password is None:
            password = pdf_password
        if password is None:
            password = _pdf_backend_password(format_options)
        # A PDF/IMAGE PdfFormatOption overrides the shorthand kwargs.
        pdf_opts = _pdf_pipeline_options(format_options)
        if pdf_opts is not None:
            do_ocr = pdf_opts.do_ocr
            do_table_structure = pdf_opts.do_table_structure
            # docling proper carries the flag on ocr_options; accept both the
            # direct field and docling-shaped ocr_options.force_full_page_ocr.
            force_full_page_ocr = getattr(
                pdf_opts, "force_full_page_ocr", force_full_page_ocr
            ) or getattr(
                getattr(pdf_opts, "ocr_options", None), "force_full_page_ocr", False
            )
            no_text_panels = getattr(pdf_opts, "no_text_panels", no_text_panels)
            hh = getattr(pdf_opts, "heading_hierarchy_options", None)
            if hh is not None:
                heading_hierarchy = bool(getattr(hh, "enabled", heading_hierarchy))
            do_picture_classification = getattr(
                pdf_opts, "do_picture_classification", do_picture_classification
            )
            do_code_enrichment = getattr(
                pdf_opts, "do_code_enrichment", do_code_enrichment
            )
            do_formula_enrichment = getattr(
                pdf_opts, "do_formula_enrichment", do_formula_enrichment
            )
            # The picture-OCR enrichment (#645) and its filters, when set on
            # the pipeline options (a docling.rs extension of
            # PdfPipelineOptions).
            do_picture_ocr = getattr(pdf_opts, "do_picture_ocr", do_picture_ocr)
            if getattr(pdf_opts, "picture_ocr_classes", None) is not None:
                picture_ocr_classes = pdf_opts.picture_ocr_classes
            if getattr(pdf_opts, "picture_ocr_min_side", None) is not None:
                picture_ocr_min_side = pdf_opts.picture_ocr_min_side
            keep_picture_images = getattr(
                pdf_opts, "keep_picture_images", keep_picture_images
            )
            # docling's PipelineOptions.document_timeout (#497), when set on
            # the pipeline options, wins over the shorthand kwarg.
            dt = getattr(pdf_opts, "document_timeout", None)
            if dt is not None:
                document_timeout = float(dt)
            # docling's image outputs (#520): page images on request, and
            # images_scale whenever docling would render images at all.
            generate_page_images = bool(
                getattr(pdf_opts, "generate_page_images", generate_page_images)
            )
            if generate_page_images or getattr(pdf_opts, "generate_picture_images", False):
                scale = getattr(pdf_opts, "images_scale", None)
                if scale is not None:
                    images_scale = float(scale)
            # Map docling's ocr_options.lang (a list of language ids) onto the
            # engine's en/ch recognition-model switch. First entry wins;
            # anything that isn't recognisably English/Chinese is ignored with
            # a warning (the engine default — English — applies).
            ocr_opts = getattr(pdf_opts, "ocr_options", None)
            # docling 2.116's OcrMode / OcrOptions.scale (#254). An enum mode
            # collapses to its string value; the direct kwargs stay the
            # fallback, matching the pipeline-overrides-shorthand rule above.
            mode = getattr(ocr_opts, "mode", None)
            if mode is not None:
                ocr_mode = getattr(mode, "value", mode)
            scale = getattr(ocr_opts, "scale", None)
            if scale is not None:
                ocr_scale = float(scale)
            # docling's Tesseract kinds (#460) select the engine; the CLI
            # options' binary, tessdata path and page segmentation mode are
            # process-wide knobs here, seeded like the accelerator device.
            kind = getattr(ocr_opts, "kind", None)
            if kind in ("tesseract", "tesserocr"):
                ocr_engine = "tesseract"
                cmd = getattr(ocr_opts, "tesseract_cmd", None)
                if cmd:
                    os.environ.setdefault("DOCLING_TESSERACT", str(cmd))
                tessdata = getattr(ocr_opts, "path", None)
                if tessdata:
                    os.environ.setdefault("DOCLING_RS_TESSDATA_DIR", str(tessdata))
                psm = getattr(ocr_opts, "psm", None)
                if psm is not None:
                    os.environ.setdefault("DOCLING_RS_TESSERACT_PSM", str(int(psm)))
            langs = list(getattr(ocr_opts, "lang", None) or [])
            if langs and ocr_engine == "tesseract":
                # Tesseract's own vocabulary: stems and `iso:` tags pass
                # through, the engine maps and validates them (#460).
                ocr_lang = "+".join(str(lang).strip() for lang in langs)
            elif langs:
                # The same spellings the engine's own `ocr_lang` accepts (#388):
                # engine codes, docling's legacy names, EasyOCR's, and BCP-47
                # tags for English / Chinese with or without docling's `iso:`
                # prefix; script and region subtags are ignored. Passed through
                # as written — the engine canonicalizes and would reject
                # anything else, so the unrecognized case warns here instead.
                head = str(langs[0]).strip().lower()
                if head.startswith("iso:"):
                    head = head[4:].strip()
                # `ch_sim`, `ch_tra`, `chinese_cht`, `en_GB` all reduce to
                # their first subtag.
                primary = head.replace("_", "-").split("-", 1)[0]
                if primary in ("en", "eng", "english"):
                    ocr_lang = "en"
                elif primary in ("ch", "zh", "zho", "chi", "cmn", "chinese"):
                    ocr_lang = "ch"
                else:
                    warnings.warn(
                        f"docling.rs OCR ships English and Chinese recognition "
                        f"models only (en | ch, or a BCP-47 tag for either); "
                        f"ocr_options.lang={langs!r} is ignored",
                        stacklevel=2,
                    )
            acc = getattr(pdf_opts, "accelerator_options", None)
            if acc is not None:
                # Map docling's device to the engine's DOCLING_RS_EP (resolved
                # once per process, so this must run before the first
                # conversion; an explicit environment override always wins).
                # AUTO maps to nothing: the engine's own default already is
                # "auto" in a GPU build (docling-rs-cuda) and CPU otherwise.
                if acc.device == AcceleratorDevice.CUDA:
                    os.environ.setdefault("DOCLING_RS_EP", "cuda")
                elif acc.device == AcceleratorDevice.CPU:
                    os.environ.setdefault("DOCLING_RS_EP", "cpu")
                elif acc.device == AcceleratorDevice.MPS:
                    # docling's Apple-GPU device → the CoreML provider, which
                    # the engine keeps opt-in (#602): this is how
                    # docling-shaped code asks for it. Only where the build
                    # has it — the PyPI macOS wheels are CPU-only.
                    if "coreml" in _compiled_providers():
                        os.environ.setdefault("DOCLING_RS_EP", "coreml")
                    else:
                        warnings.warn(
                            "docling.rs has no MPS execution provider and this "
                            "build has no CoreML; device 'mps' is ignored (build "
                            "the wheel on macOS with `maturin build --features "
                            "coreml` for CoreML).",
                            stacklevel=2,
                        )
                if acc.num_threads:
                    # Process-wide ONNX Runtime intra-op threads; don't clobber an
                    # explicit environment override.
                    os.environ.setdefault("DOCLING_RS_PDF_THREADS", str(acc.num_threads))

        self._inner = _NativeDocumentConverter(
            fetch_images=fetch_images,
            do_ocr=do_ocr,
            force_full_page_ocr=force_full_page_ocr,
            no_text_panels=no_text_panels,
            heading_hierarchy=heading_hierarchy,
            do_table_structure=do_table_structure,
            use_web_browser=use_web_browser,
            do_picture_classification=do_picture_classification,
            do_code_enrichment=do_code_enrichment,
            do_formula_enrichment=do_formula_enrichment,
            do_picture_ocr=do_picture_ocr,
            picture_ocr_classes=_label_list(picture_ocr_classes),
            picture_ocr_min_side=picture_ocr_min_side,
            keep_picture_images=keep_picture_images,
            ocr_lang=ocr_lang,
            ocr_mode=ocr_mode,
            ocr_scale=ocr_scale,
            ocr_engine=ocr_engine,
            skip_empty_cells=skip_empty_cells,
            compact_tables=compact_tables,
            asr_model=asr_model,
            asr_lang=asr_lang,
            encoding=encoding,
            pipeline=pipeline,
            vlm_endpoint=vlm_endpoint,
            vlm_model=vlm_model,
            vlm_api_key=vlm_api_key,
            vlm_prompt=vlm_prompt,
            vlm_max_tokens=vlm_max_tokens,
            document_timeout=document_timeout,
            password=password,
            page_range=_page_range(page_range),
            images_scale=images_scale,
            generate_page_images=generate_page_images,
            text_layer_only=text_layer_only,
            list_attachments=list_attachments,
            ebcdic_layout=ebcdic_layout,
            video_frames=video_frames,
            xbrl_taxonomy=xbrl_taxonomy,
            allowed_formats=(
                [InputFormat(f).value for f in allowed_formats]
                if allowed_formats is not None
                else None
            ),
        )
        # docling's do_picture_description enrichment, run on the converted
        # document (picture_description.py): the engine embeds each picture's
        # crop, so any Python-side describer — e.g. a LangChain chat model via
        # docling_rs.langchain — works without an engine change.
        self._picture_description = None
        if pdf_opts is not None and getattr(pdf_opts, "do_picture_description", False):
            pd = getattr(pdf_opts, "picture_description_options", None)
            if pd is None or not hasattr(pd, "_annotate_images"):
                # docling's default (a local SmolVLM) has no counterpart here;
                # degrade like a missing enrichment model.
                warnings.warn(
                    "do_picture_description needs picture_description_options with a "
                    "describer (e.g. docling_rs.langchain.PictureDescriptionLangChainOptions); "
                    "pictures are left undescribed",
                    stacklevel=2,
                )
            else:
                self._picture_description = pd

    def initialize_pipeline(self, format: Optional[InputFormat] = None) -> None:
        """Eagerly load the ML models for ``format`` (docling's
        ``initialize_pipeline``), so the first PDF conversion doesn't pay the
        model-load cost and later ones reuse the warm pipeline. Only ``PDF`` /
        ``IMAGE`` have models; other formats are a no-op. Uses the converter's
        configured ``do_ocr`` / ``do_table_structure`` (and needs the models
        available — see :func:`download_models`)."""
        self._inner.initialize_pipeline(
            InputFormat(format).value if format is not None else None
        )

    def convert(
        self,
        source: Union[str, os.PathLike, DocumentStream],
        page_range: Optional[Tuple[int, int]] = None,
    ) -> ConversionResult:
        """Convert a filesystem path (str / pathlib.Path), an ``http(s)://``
        URL (downloaded first, as docling does) or an in-memory
        :class:`DocumentStream`. ``page_range=(first, last)`` converts only
        that 1-based inclusive PDF page window, as docling's
        ``convert(source, page_range=…)`` does (#518); ``None`` keeps the
        constructor's window (all pages by default)."""
        return self._finish(_wrap(self._convert_native(source, page_range)))

    def convert_all(
        self,
        sources: Iterable[Union[str, os.PathLike, DocumentStream]],
        raises_on_error: bool = True,
        page_range: Optional[Tuple[int, int]] = None,
    ) -> Iterator[ConversionResult]:
        """Convert many sources, yielding a :class:`ConversionResult` each
        (docling's ``convert_all``). With ``raises_on_error=False`` a failing
        source yields a ``failure`` result (empty document) instead of raising.
        ``page_range`` as in :meth:`convert`."""
        for source in sources:
            try:
                yield self._finish(_wrap(self._convert_native(source, page_range)))
            except Exception:
                if raises_on_error:
                    raise
                name = source.name if isinstance(source, DocumentStream) else str(source)
                yield ConversionResult("failure", name, DoclingDocument(name=Path(name).name))

    def convert_bytes(
        self, name: str, data: bytes, page_range: Optional[Tuple[int, int]] = None
    ) -> ConversionResult:
        """Convert in-memory bytes; ``name``'s extension drives format detection
        (docling's ``DocumentStream`` counterpart). ``page_range`` as in
        :meth:`convert`."""
        native = self._inner.convert_bytes(name, data, page_range=_page_range(page_range))
        return self._finish(_wrap(native))

    def convert_archive(
        self, source: Union[str, os.PathLike, bytes, DocumentStream]
    ) -> Iterator[ArchiveItem]:
        """Convert every document inside a ZIP archive (#557) — a path,
        the archive's ``bytes`` or a :class:`DocumentStream` — yielding an
        :class:`ArchiveItem` per entry in archive order. A broken document
        fails only its own item; raises only when ``source`` is not a
        readable ZIP archive. Nothing is extracted to disk, and the
        ``DOCLING_RS_ZIP_MAX_*`` limits bound what is inflated."""
        if isinstance(source, DocumentStream):
            items = self._inner.convert_archive_bytes(_stream_bytes(source.stream))
        elif isinstance(source, (bytes, bytearray)):
            items = self._inner.convert_archive_bytes(bytes(source))
        elif isinstance(source, str) and _is_url(source):
            _, data = _fetch_url(source)
            items = self._inner.convert_archive_bytes(data)
        else:
            items = self._inner.convert_archive(source)
        for item in items:
            result = None
            if item.result is not None:
                result = self._finish(_wrap(item.result))
            yield ArchiveItem(item.path, item.outcome, result, item.error)

    def _convert_native(self, source, page_range=None):
        page_range = _page_range(page_range)
        if isinstance(source, DocumentStream):
            fmt = getattr(source, "format", None)
            return self._inner.convert_bytes(
                source.name,
                _stream_bytes(source.stream),
                page_range=page_range,
                format=InputFormat(fmt).value if fmt is not None else None,
            )
        if isinstance(source, str) and _is_url(source):
            name, data = _fetch_url(source)
            return self._inner.convert_bytes(name, data, page_range=page_range)
        return self._inner.convert(source, page_range=page_range)

    def _finish(self, result: ConversionResult) -> ConversionResult:
        if self._picture_description is not None and result.status != "failure":
            from .picture_description import describe_pictures

            describe_pictures(result.document, self._picture_description)
        return result


@dataclass
class EmailAttachment:
    """One attachment of an ``.eml`` / Outlook ``.msg``, from
    :func:`email_attachments`. ``data`` is the payload when it was kept
    (``None`` for an attachment over a limit, or one without a payload — a
    reference, an OLE object); ``format`` is the :class:`InputFormat` it
    converts as, ``None`` with ``skipped`` saying why it will not. A
    forwarded message is an ``.eml`` entry whose ``data`` is the nested
    message."""

    index: int
    name: str
    content_type: Optional[str]
    format: Optional[InputFormat]
    size: int
    inline: bool
    skipped: Optional[str]
    data: Optional[bytes]

    def as_stream(self) -> DocumentStream:
        """The payload as a :class:`DocumentStream` named after the attachment
        and carrying its detected ``format`` (#564: a ``scan.bin`` sent as
        ``application/pdf``, or a nameless sniffed part, converts as what it
        is, not what its extension says), ready for
        :meth:`DocumentConverter.convert`. Raises ``ValueError`` for an
        attachment without a payload."""
        if self.data is None:
            raise ValueError(f"attachment {self.name!r} has no payload ({self.skipped})")
        return DocumentStream(name=self.name, stream=io.BytesIO(self.data), format=self.format)


def email_attachments(
    source: Union[str, os.PathLike, bytes, DocumentStream],
    *,
    max_entries: Optional[int] = None,
    max_entry_size: Optional[int] = None,
    max_total_size: Optional[int] = None,
) -> List[EmailAttachment]:
    """List the attachments of an ``.eml`` / ``.msg`` with their payloads — a
    path, the message bytes, or a :class:`DocumentStream`. The limits are byte
    counts bounding what is kept (defaults: 10 000 attachments, 256 MiB each,
    1 GiB in all); file names are reduced to a safe base name, so ``name``
    can be written to a directory as it is. Convert one with
    ``converter.convert(attachment.as_stream())``."""
    if isinstance(source, DocumentStream):
        data = _stream_bytes(source.stream)
    elif isinstance(source, (bytes, bytearray, memoryview)):
        data = bytes(source)
    else:
        data = Path(source).read_bytes()
    rows = _email_attachments(data, max_entries, max_entry_size, max_total_size)
    out = []
    for index, name, content_type, fmt, size, inline, skipped, payload in rows:
        try:
            fmt = InputFormat(fmt) if fmt is not None else None
        except ValueError:
            pass
        out.append(EmailAttachment(index, name, content_type, fmt, size, inline, skipped, payload))
    return out


def _pdf_pipeline_options(
    format_options: Optional[Dict[InputFormat, PdfFormatOption]],
) -> Optional[PdfPipelineOptions]:
    """The PDF (or image) pipeline options from a docling-style ``format_options``
    mapping, if any."""
    if not format_options:
        return None
    for fmt in (InputFormat.PDF, InputFormat.IMAGE):
        fo = format_options.get(fmt)
        if fo is not None and getattr(fo, "pipeline_options", None) is not None:
            return fo.pipeline_options
    return None


def _pdf_backend_password(
    format_options: Optional[Dict[InputFormat, PdfFormatOption]],
) -> Optional[str]:
    """The PDF password from a docling-style ``format_options`` mapping —
    ``PdfFormatOption(backend_options=PdfBackendOptions(password=...))``, a
    plain string or a pydantic ``SecretStr`` — if any (#611)."""
    if not format_options:
        return None
    fo = format_options.get(InputFormat.PDF)
    password = getattr(getattr(fo, "backend_options", None), "password", None)
    if password is None:
        return None
    if hasattr(password, "get_secret_value"):
        password = password.get_secret_value()
    return str(password)


def _stream_bytes(stream) -> bytes:
    """A :class:`DocumentStream`'s whole content: rewound first when it can
    be, as docling's backends do (``path_or_stream.seek(0)``), so a stream
    already read once — or handed over after a peek — still converts (#564)."""
    if hasattr(stream, "seekable") and stream.seekable():
        stream.seek(0)
    return stream.read()


def _label_list(labels) -> Optional[str]:
    """``picture_ocr_classes`` as the engine's comma-separated string: a
    string passes through, an iterable of labels is joined."""
    if labels is None or isinstance(labels, str):
        return labels
    return ",".join(str(label) for label in labels)


def _page_range(page_range) -> Optional[Tuple[int, int]]:
    """docling's ``page_range`` argument as the engine's ``(first, last)``:
    any two-item sequence of integers; ``None`` stays ``None``. docling's
    default window ``(1, sys.maxsize)`` simply means "all pages"."""
    if page_range is None:
        return None
    try:
        first, last = page_range
        return (int(first), int(last))
    except (TypeError, ValueError):
        raise ValueError(
            f"page_range must be a (first, last) pair of page numbers, got {page_range!r}"
        ) from None


def _is_url(source: str) -> bool:
    return source.startswith(("http://", "https://"))


# Media types whose extension the URL path may not carry (e.g. arXiv's
# https://arxiv.org/pdf/2408.09869) — format detection is by extension.
_URL_EXTENSIONS = {
    "application/pdf": ".pdf",
    "text/html": ".html",
    "application/xhtml+xml": ".html",
    "text/markdown": ".md",
    "text/plain": ".txt",
    "text/csv": ".csv",
    "application/json": ".json",
    "application/xml": ".xml",
    "text/xml": ".xml",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document": ".docx",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation": ".pptx",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": ".xlsx",
    "application/epub+zip": ".epub",
    "image/png": ".png",
    "image/jpeg": ".jpg",
    "image/tiff": ".tiff",
    "image/webp": ".webp",
    "audio/mpeg": ".mp3",
    "audio/wav": ".wav",
    "audio/x-wav": ".wav",
    "video/mp4": ".mp4",
}


def _fetch_url(url: str) -> "tuple[str, bytes]":
    """Download an http(s) source (docling's ``convert(url)``): the file name
    comes from ``Content-Disposition``, else the URL path; when that has no
    extension, the response's media type supplies one. Read whole into memory
    — the engine converts bytes, like ``DocumentStream``."""
    import email.message
    import urllib.parse
    import urllib.request

    req = urllib.request.Request(url, headers={"User-Agent": f"docling-rs/{__version__}"})
    with urllib.request.urlopen(req) as resp:
        data = resp.read()
        disposition = resp.headers.get("Content-Disposition", "")
        media_type = resp.headers.get_content_type()
    name = ""
    if disposition:
        msg = email.message.Message()
        msg["Content-Disposition"] = disposition
        name = msg.get_filename() or ""
    if not name:
        name = Path(urllib.parse.unquote(urllib.parse.urlparse(url).path)).name or "document"
    if not Path(name).suffix and media_type in _URL_EXTENSIONS:
        name += _URL_EXTENSIONS[media_type]
    return name, data


def _wrap(native) -> ConversionResult:
    """Validate the Rust engine's JSON into a real ``DoclingDocument``."""
    document = DoclingDocument.model_validate_json(native.document_json)
    errors = [ErrorItem(*item) for item in getattr(native, "errors", ())]
    return ConversionResult(native.status, native.input_name, document, errors)
