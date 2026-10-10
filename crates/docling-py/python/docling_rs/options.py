"""docling-shaped configuration objects.

These mirror the names and fields of docling's ``InputFormat`` /
``PdfPipelineOptions`` / ``PdfFormatOption`` / ``DocumentStream`` so existing
docling code reads unchanged, but they are plain, dependency-free dataclasses:
the Rust engine acts on the subset it supports (``do_ocr``,
``do_table_structure``, and ``accelerator_options.num_threads``), and the rest
are accepted for API compatibility. See the crate README for the support matrix.
"""

from __future__ import annotations

import enum
from dataclasses import dataclass, field
from typing import Any, BinaryIO, Optional


class InputFormat(str, enum.Enum):
    """docling's ``InputFormat``. The first block matches docling's members
    one-for-one (same string values); the second lists formats docling.rs also
    handles. A ``str`` enum, so ``InputFormat.PDF == "pdf"``."""

    DOCX = "docx"
    PPTX = "pptx"
    HTML = "html"
    IMAGE = "image"
    PDF = "pdf"
    ASCIIDOC = "asciidoc"
    MD = "md"
    CSV = "csv"
    XLSX = "xlsx"
    # Legacy binary Office (docling converts via LibreOffice; docling.rs
    # parses natively):
    DOC = "doc"
    XLS = "xls"
    PPT = "ppt"
    XML_USPTO = "xml_uspto"
    XML_JATS = "xml_jats"
    JSON_DOCLING = "json_docling"
    AUDIO = "audio"
    # docling.rs also supports:
    ODT = "odt"
    ODS = "ods"
    ODP = "odp"
    EPUB = "epub"
    VTT = "vtt"
    EMAIL = "email"
    LATEX = "latex"
    MHTML = "mhtml"
    XML_XBRL = "xml_xbrl"
    XML_DOCLANG = "xml_doclang"
    METS_GBS = "mets_gbs"


class AcceleratorDevice(str, enum.Enum):
    """docling's ``AcceleratorDevice``, mapped to the engine's
    ``DOCLING_RS_EP``. ``AUTO`` (the default) leaves the engine's own default
    in place: GPU-when-usable with CPU fallback on the ``docling-rs-cuda``
    wheel, plain CPU on the CPU wheel. ``CUDA`` requires the GPU wheel and
    fails loudly when the GPU can't initialize; ``CPU`` forces CPU. ``MPS``
    selects the CoreML provider (opt-in in the engine, #602) in a wheel built
    with it (``maturin build --features coreml`` on macOS) and warns
    elsewhere — the PyPI macOS wheels are CPU-only."""

    AUTO = "auto"
    CPU = "cpu"
    CUDA = "cuda"
    MPS = "mps"


class TableFormerMode(str, enum.Enum):
    """docling's ``TableFormerMode`` (accepted; the Rust TableFormer runs a
    single accurate mode)."""

    FAST = "fast"
    ACCURATE = "accurate"


@dataclass
class AcceleratorOptions:
    """docling's ``AcceleratorOptions``. ``num_threads`` maps to the engine's
    ONNX Runtime intra-op thread count (via ``DOCLING_RS_PDF_THREADS``)."""

    num_threads: int = 4
    device: AcceleratorDevice = AcceleratorDevice.AUTO


@dataclass
class TableStructureOptions:
    """docling's ``TableStructureOptions`` (accepted for API compatibility)."""

    do_cell_matching: bool = True
    mode: TableFormerMode = TableFormerMode.ACCURATE


@dataclass
class HeadingHierarchyOptions:
    """docling's ``HeadingHierarchyOptions`` (#302): infer PDF/image
    section-header levels after assembly — PDF bookmarks are authoritative,
    legal/outline numbering covers headings without a bookmark match, font
    style ranks the rest. The Rust engine acts on ``enabled``; the remaining
    fields are accepted so docling code constructs unchanged (the engine runs
    docling's default sub-options)."""

    enabled: bool = False
    use_bookmarks: bool = True
    use_numbering: bool = True
    use_style: bool = True
    use_font_style: bool = True
    style_size_tolerance: float = 0.05
    max_level: int = 6
    bookmark_match_threshold: float = 0.8
    numbering_schemes: Optional[list] = None


@dataclass
class PdfPipelineOptions:
    """docling's ``PdfPipelineOptions``.

    Acted on by the Rust engine: ``do_ocr``, ``do_table_structure``,
    ``force_full_page_ocr`` (docling keeps it on ``ocr_options``; accepted here
    directly too — #187's escape hatch for undecodable text layers),
    ``do_picture_classification`` / ``do_code_enrichment`` /
    ``do_formula_enrichment`` (the opt-in enrichment models),
    ``do_picture_ocr`` + ``picture_ocr_classes`` / ``picture_ocr_min_side`` /
    ``keep_picture_images`` (#645: OCR the pictures of non-PDF documents),
    ``no_text_panels`` (a docling.rs extension, #173/#174: keep every detected
    picture as a picture instead of demoting uncaptioned dense-text panels to
    paragraphs) and
    ``accelerator_options.num_threads``. ``do_picture_description`` with
    ``picture_description_options`` runs in Python on the converted document
    (:mod:`docling_rs.picture_description`). ``generate_page_images`` keeps
    each page's render as ``document.pages[n].image``, and ``images_scale``
    sets the picture-crop / page-image resolution once either
    ``generate_picture_images`` or ``generate_page_images`` is on (#520 —
    docling renders images only then; otherwise crops keep the engine's 2.0
    px/pt render, since picture images are always extracted here). The
    remaining fields are accepted so docling code constructs unchanged, but do
    not alter the pipeline (the export image mode is chosen by docling-core at
    ``export_to_markdown(...)`` time)."""

    do_ocr: bool = True
    do_table_structure: bool = True
    force_full_page_ocr: bool = False
    no_text_panels: bool = False
    heading_hierarchy_options: HeadingHierarchyOptions = field(
        default_factory=HeadingHierarchyOptions
    )
    do_picture_classification: bool = False
    do_code_enrichment: bool = False
    do_formula_enrichment: bool = False
    #: docling.rs extension (#645): OCR the pictures embedded in non-PDF
    #: documents with the engine's OCR models; the text lands in
    #: ``picture.meta.description`` (+ the ``description`` annotation), as
    #: ``do_picture_description`` writes it. ``picture_ocr_classes`` (a
    #: comma-separated string or a list of DocumentFigureClassifier labels)
    #: and ``picture_ocr_min_side`` (px, default 32) filter which pictures
    #: are read; ``keep_picture_images=False`` drops the image bytes after.
    do_picture_ocr: bool = False
    picture_ocr_classes: Optional[Any] = None
    picture_ocr_min_side: Optional[int] = None
    keep_picture_images: bool = True
    table_structure_options: TableStructureOptions = field(
        default_factory=TableStructureOptions
    )
    accelerator_options: AcceleratorOptions = field(default_factory=AcceleratorOptions)
    images_scale: float = 1.0
    generate_page_images: bool = False
    generate_picture_images: bool = False
    #: docling's picture description enrichment: run on the converted
    #: document with ``picture_description_options``' model (a
    #: :class:`docling_rs.picture_description.PictureDescriptionBaseOptions`
    #: subclass such as ``docling_rs.langchain.PictureDescriptionLangChainOptions``).
    do_picture_description: bool = False
    picture_description_options: Optional[Any] = None
    #: Accepted for docling compatibility; no effect (no plugin system, no
    #: remote-service gate — the describer you pass is what runs).
    allow_external_plugins: bool = False
    enable_remote_services: bool = False


@dataclass
class PdfFormatOption:
    """docling's ``PdfFormatOption``: carries ``pipeline_options`` for a format,
    as passed in ``DocumentConverter(format_options={InputFormat.PDF: ...})``."""

    pipeline_options: Optional[PdfPipelineOptions] = None


@dataclass
class DocumentStream:
    """docling's ``DocumentStream``: an in-memory source whose ``name`` (with
    extension) drives format detection. ``stream`` is any binary file-like
    object (e.g. ``io.BytesIO``). ``format`` (a docling.rs extension, #564)
    names the :class:`InputFormat` outright when the name cannot — an email
    attachment called ``scan.bin`` sent as ``application/pdf``; ``None``
    keeps the extension-based detection."""

    name: str
    stream: BinaryIO
    format: Optional["InputFormat"] = None
