"""Marker-compatible facade — datalab's Marker (``marker-pdf``) Python API over
the docling.rs engine::

    # was: from marker.converters.pdf import PdfConverter
    #      from marker.models import create_model_dict
    #      from marker.output import text_from_rendered
    from docling_rs.marker.converters.pdf import PdfConverter
    from docling_rs.marker.models import create_model_dict
    from docling_rs.marker.output import text_from_rendered

    converter = PdfConverter(artifact_dict=create_model_dict())
    rendered = converter("paper.pdf")            # MarkdownOutput
    text, _, images = text_from_rendered(rendered)

``PdfConverter`` / ``TableConverter`` / ``OCRConverter``, ``create_model_dict``,
``ConfigParser`` (``docling_rs.marker.config.parser``), the renderers and their
output models (``MarkdownOutput``, ``HTMLOutput``, ``JSONOutput`` /
``JSONBlockOutput``, ``ChunkOutput`` / ``FlatBlockOutput``, ``OCRJSONOutput``),
``text_from_rendered`` / ``save_output`` / ``output_exists``, ``BlockTypes``
and ``settings`` follow Marker 2.0's module layout, so code written against
Marker runs unchanged after the import swap. What differs, deliberately:

* **The engine is docling.rs**: layout analysis, TableFormer and PP-OCR on
  ONNX Runtime — no PyTorch, no Surya inference server, no LLM. ``mode``
  (``balanced`` / ``fast``), ``use_llm``, ``llm_service``,
  ``block_correction_prompt``, ``redo_inline_math``, ``processor_list`` and
  ``processors`` are accepted and ignored (one warning each when they would
  have changed Marker's behaviour); ``create_model_dict`` fetches
  docling.rs's models into ``~/.cache/docling.rs`` on first use, as Marker's
  fetches Surya's.
* **Every format docling.rs reads** goes through ``PdfConverter`` — Marker
  renders DOCX/PPTX/XLSX/HTML/EPUB to PDF first (``marker-pdf[full]``,
  WeasyPrint) and runs the vision models on the render; docling.rs parses
  them natively, so those formats keep their real headings, tables and
  lists and need no models.
* **The Markdown is docling's** (docling-core's export): ``![](<image>)``
  references to the ``images`` dict keyed ``_page_<p>_Picture_<n>.jpeg``, as
  Marker names them; ``paginate_output`` inserts Marker's
  ``{<page>}----…`` separators. Marker's own markup — its ``$$`` math
  spacing, escaped underscores, ``<sup>`` footnote marks — is not
  reproduced byte for byte.
* **The block tree** (JSON / chunks): one page block per page, children in
  reading order with Marker's ids (``/page/0/Text/3``), ``polygon`` and
  ``bbox`` in PDF points from the top-left corner, ``section_hierarchy``
  and per-block ``html``. docling's labels map onto Marker's
  ``BlockTypes``; a picture or table with a caption becomes Marker's
  ``PictureGroup`` / ``TableGroup``, a list its ``ListGroup``.
  ``page_stats.text_extraction_method`` says ``pdftext`` unless OCR was
  forced (``ocr``) — docling.rs does not report per page which pages it
  OCR'd.
* **``OCRConverter``** emits one ``Line`` per docling text item with its
  text and box, not Surya's lines and characters (no ``children``).
* **``TableConverter``** keeps the ``Table`` / ``Form`` /
  ``TableOfContents`` blocks of every page, OCR off, as Marker's does;
  ``force_layout_block`` is accepted and ignored (the layout model always
  runs).

``PdfConverter(config={"docling_options": {...}})`` (a docling.rs extension)
forwards keyword arguments to :class:`docling_rs.DocumentConverter` — e.g.
``{"text_layer_only": True}`` for Marker's ``--disable_ocr`` speed class,
``{"ocr_lang": "ch"}``. The converted ``DoclingDocument`` is on
``rendered.document``.

``python -m docling_rs.marker FILE|FOLDER`` mirrors ``marker_single`` and
``marker``.
"""

from __future__ import annotations

import base64
import collections
import enum
import html as _html
import io
import json
import os
import re
import tempfile
import warnings
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple, Union

from pydantic import BaseModel

from .. import DocumentConverter
from .. import models as _models

__all__ = [
    "BlockTypes",
    "BlockId",
    "PdfConverter",
    "TableConverter",
    "OCRConverter",
    "create_model_dict",
    "ConfigParser",
    "MarkdownOutput",
    "HTMLOutput",
    "JSONOutput",
    "JSONBlockOutput",
    "ChunkOutput",
    "FlatBlockOutput",
    "OCRJSONOutput",
    "OCRJSONPageOutput",
    "OCRJSONLineOutput",
    "MarkdownRenderer",
    "HTMLRenderer",
    "JSONRenderer",
    "ChunkRenderer",
    "OCRJSONRenderer",
    "text_from_rendered",
    "save_output",
    "output_exists",
    "parse_range_str",
    "settings",
]


# ---- Marker's settings ----------------------------------------------------


class _Settings:
    """Marker's ``settings`` object — the fields its CLI and ``save_output``
    read, from the same environment variables Marker's pydantic-settings
    reads (``OUTPUT_DIR``, ``OUTPUT_IMAGE_FORMAT``, ``OUTPUT_ENCODING``,
    ``TORCH_DEVICE``). ``OUTPUT_DIR`` defaults to ``./conversion_results``
    where Marker's is ``conversion_results`` next to its package."""

    def __init__(self) -> None:
        self.OUTPUT_DIR = os.environ.get("OUTPUT_DIR", os.path.join(os.getcwd(), "conversion_results"))
        self.OUTPUT_IMAGE_FORMAT = os.environ.get("OUTPUT_IMAGE_FORMAT", "JPEG")
        self.OUTPUT_ENCODING = os.environ.get("OUTPUT_ENCODING", "utf-8")
        self.TORCH_DEVICE: Optional[str] = os.environ.get("TORCH_DEVICE")
        self.LOGLEVEL = os.environ.get("LOGLEVEL", "INFO")
        self.GOOGLE_API_KEY: Optional[str] = os.environ.get("GOOGLE_API_KEY", "")

    @property
    def TORCH_DEVICE_MODEL(self) -> str:
        return self.TORCH_DEVICE or "cpu"


settings = _Settings()


# ---- Marker's block vocabulary ---------------------------------------------


class BlockTypes(str, enum.Enum):
    """Marker's ``BlockTypes`` (``marker.schema``), the same names in the same
    order; ``str(BlockTypes.Text) == "Text"`` as in Marker."""

    Line = "Line"
    Span = "Span"
    Char = "Char"
    FigureGroup = "FigureGroup"
    TableGroup = "TableGroup"
    ListGroup = "ListGroup"
    PictureGroup = "PictureGroup"
    Page = "Page"
    Caption = "Caption"
    Code = "Code"
    Figure = "Figure"
    Footnote = "Footnote"
    Form = "Form"
    Equation = "Equation"
    Handwriting = "Handwriting"
    TextInlineMath = "TextInlineMath"
    ListItem = "ListItem"
    PageFooter = "PageFooter"
    PageHeader = "PageHeader"
    Picture = "Picture"
    SectionHeader = "SectionHeader"
    Table = "Table"
    Text = "Text"
    TableOfContents = "TableOfContents"
    Document = "Document"
    ComplexRegion = "ComplexRegion"
    TableCell = "TableCell"
    Reference = "Reference"
    Bibliography = "Bibliography"
    ChemicalBlock = "ChemicalBlock"
    Diagram = "Diagram"

    def __str__(self) -> str:
        return self.name


class BlockId:
    """Marker's ``BlockId``: ``/page/<page>/<Type>/<n>`` (``/page/<page>`` for
    a page), ``to_path()`` with ``_`` for ``/`` — the image file stems."""

    def __init__(self, page_id: int, block_id: Optional[int] = None, block_type: Optional[BlockTypes] = None):
        self.page_id = page_id
        self.block_id = block_id
        self.block_type = block_type

    def __str__(self) -> str:
        if self.block_type is None or self.block_id is None:
            return f"/page/{self.page_id}"
        return f"/page/{self.page_id}/{self.block_type.name}/{self.block_id}"

    def to_path(self) -> str:
        return str(self).replace("/", "_")

    def __eq__(self, other: Any) -> bool:
        return str(self) == (str(other) if isinstance(other, BlockId) else other)

    def __hash__(self) -> int:
        return hash(str(self))

    def __repr__(self) -> str:
        return f"BlockId({str(self)!r})"


# docling label → Marker block type. Labels docling.rs emits that Marker has
# no block for go to the nearest reading-order equivalent.
_LABELS = {
    "text": BlockTypes.Text,
    "paragraph": BlockTypes.Text,
    "title": BlockTypes.SectionHeader,
    "section_header": BlockTypes.SectionHeader,
    "caption": BlockTypes.Caption,
    "footnote": BlockTypes.Footnote,
    "page_header": BlockTypes.PageHeader,
    "page_footer": BlockTypes.PageFooter,
    "list_item": BlockTypes.ListItem,
    "code": BlockTypes.Code,
    "formula": BlockTypes.Equation,
    "table": BlockTypes.Table,
    "picture": BlockTypes.Picture,
    "chart": BlockTypes.Figure,
    "document_index": BlockTypes.TableOfContents,
    "reference": BlockTypes.Reference,
    "checkbox_selected": BlockTypes.Form,
    "checkbox_unselected": BlockTypes.Form,
    "key_value_region": BlockTypes.Form,
    "form": BlockTypes.Form,
    "handwritten_text": BlockTypes.Handwriting,
    "empty_value": BlockTypes.Text,
    "grading_scale": BlockTypes.Form,
}
_GROUP_LABELS = {
    "list": BlockTypes.ListGroup,
    "ordered_list": BlockTypes.ListGroup,
    "picture_area": BlockTypes.PictureGroup,
    "form_area": BlockTypes.Form,
    "key_value_area": BlockTypes.Form,
}
_IMAGE_TYPES = (BlockTypes.Picture, BlockTypes.Figure, BlockTypes.Diagram)
_TABLE_TYPES = (BlockTypes.Table, BlockTypes.Form, BlockTypes.TableOfContents)


def _label(item: Any) -> str:
    label = getattr(item, "label", None)
    return getattr(label, "value", label) or ""


# ---- the rendered document -------------------------------------------------


class _Block:
    """One node of the Marker-shaped tree built from a ``DoclingDocument``."""

    __slots__ = ("id", "type", "item", "children", "polygon", "bbox", "section_hierarchy", "image_name")

    def __init__(self, block_id: BlockId, block_type: BlockTypes, item: Any = None):
        self.id = block_id
        self.type = block_type
        self.item = item
        self.children: List[_Block] = []
        self.polygon: List[List[float]] = [[0.0, 0.0]] * 4
        self.bbox: List[float] = [0.0, 0.0, 0.0, 0.0]
        self.section_hierarchy: Dict[int, str] = {}
        self.image_name: Optional[str] = None


class _Page:
    __slots__ = ("page_id", "width", "height", "blocks", "counter")

    def __init__(self, page_id: int, width: float, height: float):
        self.page_id = page_id
        self.width = width
        self.height = height
        self.blocks: List[_Block] = []
        self.counter = 0

    @property
    def polygon(self) -> List[List[float]]:
        return [[0.0, 0.0], [self.width, 0.0], [self.width, self.height], [0.0, self.height]]

    @property
    def bbox(self) -> List[float]:
        return [0.0, 0.0, self.width, self.height]


class _Rendered:
    """The document as Marker's renderers see it: pages of blocks in reading
    order, the extracted images and the metadata. Built once per conversion,
    shared by every output format."""

    def __init__(self, document: Any, options: Dict[str, Any]):
        self.document = document
        self.options = options
        self.pages: List[_Page] = []
        self.images: Dict[str, Any] = {}
        self.toc: List[Dict[str, Any]] = []
        self._hier: Dict[int, str] = {}
        self._build()

    # -- tree -------------------------------------------------------------

    def _build(self) -> None:
        doc = self.document
        pages: Dict[int, _Page] = {}
        for no, page in sorted(getattr(doc, "pages", {}).items()):
            size = getattr(page, "size", None)
            pages[int(no) - 1] = _Page(int(no) - 1, float(getattr(size, "width", 0.0) or 0.0), float(getattr(size, "height", 0.0) or 0.0))
        if not pages:  # a declarative document: no pages, one virtual page 0
            pages[0] = _Page(0, 0.0, 0.0)
        self._pages = pages
        keep = self.options.get("keep_pages")
        layers = self._layers()
        for ref in getattr(doc.body, "children", []):
            self._walk(ref.resolve(doc), layers, None)
        self.pages = [p for n, p in sorted(pages.items()) if keep is None or n in keep]

    def _layers(self) -> set:
        from docling_core.types.doc import ContentLayer

        layers = {ContentLayer.BODY}
        if self.options.get("keep_pageheader_in_output") or self.options.get("keep_pagefooter_in_output"):
            layers.add(ContentLayer.FURNITURE)
        return layers

    def _page_of(self, item: Any) -> int:
        prov = getattr(item, "prov", None)
        if prov:
            return int(prov[0].page_no) - 1
        return 0 if 0 in self._pages else min(self._pages)

    def _box(self, item: Any, page: _Page) -> Tuple[List[List[float]], List[float]]:
        prov = getattr(item, "prov", None)
        if not prov:
            return [[0.0, 0.0]] * 4, [0.0, 0.0, 0.0, 0.0]
        b = prov[0].bbox
        left, right = float(b.l), float(b.r)
        if getattr(b.coord_origin, "value", str(b.coord_origin)).upper().endswith("BOTTOMLEFT"):
            top, bottom = page.height - float(b.t), page.height - float(b.b)
        else:
            top, bottom = float(b.t), float(b.b)
        top, bottom = min(top, bottom), max(top, bottom)
        return [[left, top], [right, top], [right, bottom], [left, bottom]], [left, top, right, bottom]

    def _new_block(self, page: _Page, block_type: BlockTypes, item: Any = None) -> _Block:
        block = _Block(BlockId(page.page_id, page.counter, block_type), block_type, item)
        page.counter += 1
        if item is not None:
            block.polygon, block.bbox = self._box(item, page)
        return block

    def _attach(self, block: _Block, parent: Optional[_Block], page: _Page) -> None:
        (parent.children if parent is not None else page.blocks).append(block)

    def _walk(self, item: Any, layers: set, parent: Optional[_Block]) -> None:
        doc = self.document
        label = _label(item)
        children = [c.resolve(doc) for c in getattr(item, "children", [])]
        if hasattr(item, "text") or label in ("table", "picture", "chart"):
            layer = getattr(item, "content_layer", None)
            if layer is not None and layer not in layers:
                return
            if label == "page_header" and not self.options.get("keep_pageheader_in_output"):
                return
            if label == "page_footer" and not self.options.get("keep_pagefooter_in_output"):
                return
            block_type = _LABELS.get(label, BlockTypes.Text)
            page = self._pages.setdefault(self._page_of(item), _Page(self._page_of(item), 0.0, 0.0))
            captions = [c for c in children if _label(c) == "caption"]
            rest = [c for c in children if _label(c) != "caption"]
            if captions and block_type in _IMAGE_TYPES + (BlockTypes.Table,):
                group = self._new_block(page, BlockTypes.PictureGroup if block_type in _IMAGE_TYPES else BlockTypes.TableGroup)
                self._attach(group, parent, page)
                block = self._new_block(page, block_type, item)
                self._finish(block, page)
                group.children.append(block)
                for cap in captions:
                    self._walk(cap, layers, group)
                if block_type is BlockTypes.Table:  # a table's footnotes; a figure's inner text stays inside it
                    for child in rest:
                        self._walk(child, layers, group)
                self._union(group)
                return
            block = self._new_block(page, block_type, item)
            self._finish(block, page)
            self._attach(block, parent, page)
            if block_type in _IMAGE_TYPES:  # the text inside a figure stays inside it, as in Marker
                return
            for child in children:  # footnotes nested under a table or paragraph
                self._walk(child, layers, block)
            return
        # a group
        group_type = _GROUP_LABELS.get(label)
        if group_type is None:  # chapter / section / inline / sheet …: flatten
            for child in children:
                self._walk(child, layers, parent)
            return
        first = next((c for c in children if getattr(c, "prov", None)), None)
        page = self._pages.setdefault(self._page_of(first) if first else 0, _Page(0, 0.0, 0.0))
        group = self._new_block(page, group_type, item)
        self._attach(group, parent, page)
        for child in children:
            self._walk(child, layers, group)
        self._union(group)

    def _finish(self, block: _Block, page: _Page) -> None:
        """Section hierarchy, the table of contents and the picture images."""
        item = block.item
        if block.type is BlockTypes.SectionHeader:
            level = 1 if _label(item) == "title" else int(getattr(item, "level", 1) or 1) + 1
            self._hier = {k: v for k, v in self._hier.items() if k < level}
            self._hier[level] = str(block.id)
            self.toc.append({
                "title": getattr(item, "text", ""),
                "heading_level": level,
                "page_id": page.page_id,
                "polygon": block.polygon,
            })
        block.section_hierarchy = dict(self._hier)
        if block.type in _IMAGE_TYPES and self.options.get("extract_images", True):
            image = None
            try:
                image = item.get_image(self.document)
            except Exception:  # noqa: BLE001 — a picture without data is a placeholder
                image = None
            if image is not None:
                name = f"{block.id.to_path()}.{settings.OUTPUT_IMAGE_FORMAT.lower()}"
                self.images[name] = image
                block.image_name = name
                ref = getattr(item, "image", None)
                if ref is not None:
                    try:
                        ref.uri = Path(name)
                    except Exception:  # noqa: BLE001 — export then keeps the data URI
                        pass

    def _union(self, group: _Block) -> None:
        boxes = [c.bbox for c in group.children if any(c.bbox)]
        if not boxes:
            return
        left, top = min(b[0] for b in boxes), min(b[1] for b in boxes)
        right, bottom = max(b[2] for b in boxes), max(b[3] for b in boxes)
        group.bbox = [left, top, right, bottom]
        group.polygon = [[left, top], [right, top], [right, bottom], [left, bottom]]

    # -- per-block HTML -----------------------------------------------------

    def block_html(self, block: _Block) -> str:
        """Marker's ``html`` of a block: a container carries
        ``<content-ref src='…'>`` placeholders for its children, a leaf its
        own markup."""
        if block.children:
            refs = "".join(f"<content-ref src='{c.id}'></content-ref>" for c in block.children)
            if block.type is BlockTypes.ListGroup:
                tag = "ol" if _label(block.item) == "ordered_list" else "ul"
                return f"<{tag}>{refs}</{tag}>"
            return refs
        return self.leaf_html(block)

    def leaf_html(self, block: _Block) -> str:
        item = block.item
        if item is None:
            return ""
        if block.type in _IMAGE_TYPES:
            return f"<p><img src='{block.image_name}'></p>" if block.image_name else ""
        if block.type is BlockTypes.Table and hasattr(item, "export_to_html"):
            try:
                return item.export_to_html(self.document, add_caption=False)
            except TypeError:
                return item.export_to_html(self.document)
        if block.type is BlockTypes.Equation:
            return f"<math display=\"block\">{_html.escape(getattr(item, 'text', ''))}</math>"
        if block.type is BlockTypes.Code:
            return f"<pre>{_html.escape(getattr(item, 'text', ''))}</pre>"
        try:
            from docling_core.transforms.serializer.html import HTMLDocSerializer

            text = HTMLDocSerializer(doc=self.document).serialize(item=item).text.strip()
            if text:
                return text
        except Exception:  # noqa: BLE001 — fall back to plain markup
            pass
        text = _html.escape(getattr(item, "text", ""))
        if block.type is BlockTypes.SectionHeader:
            level = min(6, max(1, 1 if _label(item) == "title" else int(getattr(item, "level", 1) or 1) + 1))
            return f"<h{level}>{text}</h{level}>"
        if block.type is BlockTypes.ListItem:
            return f"<li>{text}</li>"
        return f"<p>{text}</p>"

    def spliced_html(self, block: _Block) -> str:
        """The block's HTML with its children spliced in (Marker's
        ``json_to_html`` / ``assemble_html_with_images``)."""
        html = self.block_html(block)
        for child in block.children:
            html = html.replace(f"<content-ref src='{child.id}'></content-ref>", self.spliced_html(child))
        return html

    # -- document-level exports -----------------------------------------------

    def _export_kwargs(self) -> Dict[str, Any]:
        from docling_core.types.doc import ImageRefMode

        kwargs: Dict[str, Any] = {
            "image_mode": ImageRefMode.REFERENCED if self.options.get("extract_images", True) else ImageRefMode.PLACEHOLDER,
            "included_content_layers": self._layers(),
        }
        labels = self._labels()
        if labels is not None:
            kwargs["labels"] = labels
        return kwargs

    def _labels(self) -> Optional[set]:
        """The docling labels a block-type filter (``TableConverter``) keeps,
        for docling-core's ``labels=`` export filter; ``None`` = everything."""
        keep = self.options.get("block_types")
        if not keep:
            return None
        from docling_core.types.doc import DocItemLabel

        labels = {DocItemLabel(name) for name, t in _LABELS.items() if t in keep and name in DocItemLabel._value2member_map_}
        return labels

    def markdown(self) -> str:
        doc = self.document
        kwargs = self._export_kwargs()
        paginate = self.options.get("paginate_output", False)
        sep = self.options.get("page_separator", "-" * 48)
        has_pages = bool(getattr(doc, "pages", None))
        if not has_pages or (not paginate and self.options.get("keep_pages") is None):
            parts = [doc.export_to_markdown(**kwargs)]
            if paginate:
                parts = [f"{{{0}}}{sep}\n\n" + parts[0]]
        else:
            parts = []
            for page in self.pages:
                body = doc.export_to_markdown(page_no=page.page_id + 1, **kwargs).strip()
                parts.append(f"{{{page.page_id}}}{sep}\n\n{body}" if paginate else body)
        text = "\n\n".join(parts)
        if not self.options.get("extract_images", True):
            text = re.sub(r"^<!-- image -->\n?", "", text, flags=re.M)
        return self._fix_image_links(text).strip() + "\n" if text.strip() else ""

    def _fix_image_links(self, text: str) -> str:
        # docling-core writes `![Image](name)`; Marker writes `![](name)`.
        return re.sub(r"!\[Image\]\((_page_\d+_[A-Za-z]+_\d+\.[a-z]+)\)", r"![](\1)", text)

    def html(self) -> str:
        from docling_core.types.doc import ImageRefMode

        doc = self.document
        kwargs = self._export_kwargs()
        kwargs["image_mode"] = ImageRefMode.REFERENCED if self.options.get("extract_images", True) else ImageRefMode.PLACEHOLDER
        head = '<head><meta charset="utf-8" /></head>'
        has_pages = bool(getattr(doc, "pages", None))
        paginate = self.options.get("paginate_output", False)
        if not has_pages or (not paginate and self.options.get("keep_pages") is None):
            return doc.export_to_html(html_head=head, **kwargs)
        bodies = []
        for page in self.pages:
            full = doc.export_to_html(html_head=head, page_no=page.page_id + 1, **kwargs)
            m = re.search(r"<body[^>]*>(.*)</body>", full, flags=re.S)
            body = (m.group(1) if m else full).strip()
            # docling-core wraps a single page in its own page div; Marker's
            # carries the page id.
            m = re.fullmatch(r"<div class='page'>(.*)</div>", body, flags=re.S)
            if m:
                body = m.group(1).strip()
            bodies.append(f"<div class='page' data-page-id='{page.page_id}'>{body}</div>" if paginate else body)
        return f"<!DOCTYPE html>\n<html lang=\"en\">\n{head}\n<body>\n" + "\n".join(bodies) + "\n</body>\n</html>"

    def metadata(self) -> Dict[str, Any]:
        stats = []
        forced = self.options.get("force_ocr") or self.options.get("strip_existing_ocr")
        for page in self.pages:
            counts: collections.Counter = collections.Counter()
            stack = list(page.blocks)
            while stack:
                b = stack.pop()
                counts[str(b.type)] += 1
                stack.extend(b.children)
            stats.append({
                "page_id": page.page_id,
                "text_extraction_method": "ocr" if forced else "pdftext",
                "block_counts": [list(x) for x in counts.most_common()],
                "block_metadata": {"llm_request_count": 0, "llm_error_count": 0, "llm_tokens_used": 0},
            })
        keep = {p.page_id for p in self.pages}
        return {
            "table_of_contents": [t for t in self.toc if t["page_id"] in keep],
            "page_stats": stats,
        }


# ---- output models (Marker's pydantic models) --------------------------------


class _Output(BaseModel):
    model_config = {"arbitrary_types_allowed": True, "extra": "allow"}

    @property
    def document(self) -> Any:
        """docling.rs extension: the converted ``DoclingDocument``."""
        return getattr(self, "_document", None)


class MarkdownOutput(_Output):
    markdown: str
    images: dict
    metadata: dict


class HTMLOutput(_Output):
    html: str
    images: dict
    metadata: dict


class JSONBlockOutput(BaseModel):
    id: str
    block_type: str
    html: str
    polygon: List[List[float]]
    bbox: List[float]
    children: Optional[List["JSONBlockOutput"]] = None
    section_hierarchy: Optional[Dict[int, str]] = None
    images: Optional[dict] = None


class JSONOutput(_Output):
    children: List[JSONBlockOutput]
    block_type: str = str(BlockTypes.Document)
    metadata: dict


class FlatBlockOutput(BaseModel):
    id: str
    block_type: str
    html: str
    page: int
    polygon: List[List[float]]
    bbox: List[float]
    section_hierarchy: Optional[Dict[int, str]] = None
    images: Optional[dict] = None


class ChunkOutput(_Output):
    blocks: List[FlatBlockOutput]
    page_info: Dict[int, dict]
    metadata: dict


class OCRJSONCharOutput(BaseModel):
    id: str
    block_type: str
    text: str
    polygon: List[List[float]]
    bbox: List[float]


class OCRJSONLineOutput(BaseModel):
    id: str
    block_type: str
    html: str
    polygon: List[List[float]]
    bbox: List[float]
    children: Optional[List[OCRJSONCharOutput]] = None


class OCRJSONPageOutput(BaseModel):
    id: str
    block_type: str
    polygon: List[List[float]]
    bbox: List[float]
    children: Optional[List[OCRJSONLineOutput]] = None


class OCRJSONOutput(_Output):
    children: List[OCRJSONPageOutput]
    block_type: str = str(BlockTypes.Document)
    metadata: Optional[dict] = None


def _image_b64(image: Any) -> str:
    buf = io.BytesIO()
    if image.mode != "RGB":
        image = image.convert("RGB")
    image.save(buf, format=settings.OUTPUT_IMAGE_FORMAT)
    return base64.b64encode(buf.getvalue()).decode("ascii")


# ---- renderers ---------------------------------------------------------------


class BaseRenderer:
    """Marker's ``BaseRenderer`` shape: constructed with a ``config`` dict,
    called with the document. Here the document is the facade's rendered
    tree."""

    extract_images: bool = True
    image_extraction_mode: str = "highres"
    keep_pageheader_in_output: bool = False
    keep_pagefooter_in_output: bool = False
    add_block_ids: bool = False
    paginate_output: bool = False

    def __init__(self, config: Optional[Union[dict, BaseModel]] = None):
        _assign_config(self, config)

    def __call__(self, document: _Rendered) -> Any:
        raise NotImplementedError

    def _with_document(self, output: _Output, document: _Rendered) -> _Output:
        object.__setattr__(output, "_document", document.document)
        return output


class MarkdownRenderer(BaseRenderer):
    page_separator: str = "-" * 48
    inline_math_delimiters: Tuple[str, str] = ("$", "$")
    block_math_delimiters: Tuple[str, str] = ("$$", "$$")
    html_tables_in_markdown: bool = False

    def __call__(self, document: _Rendered) -> MarkdownOutput:
        document.options.setdefault("page_separator", self.page_separator)
        out = MarkdownOutput(markdown=document.markdown(), images=dict(document.images), metadata=document.metadata())
        return self._with_document(out, document)


class HTMLRenderer(BaseRenderer):
    def __call__(self, document: _Rendered) -> HTMLOutput:
        out = HTMLOutput(html=document.html(), images=dict(document.images), metadata=document.metadata())
        return self._with_document(out, document)


class JSONRenderer(BaseRenderer):
    def _block(self, document: _Rendered, block: _Block) -> JSONBlockOutput:
        if block.children:
            return JSONBlockOutput(
                id=str(block.id), block_type=str(block.type), html=document.block_html(block),
                polygon=block.polygon, bbox=block.bbox, section_hierarchy=block.section_hierarchy or None,
                children=[self._block(document, c) for c in block.children],
            )
        images = None
        if block.image_name and block.image_name in document.images:
            images = {str(block.id): _image_b64(document.images[block.image_name])}
        return JSONBlockOutput(
            id=str(block.id), block_type=str(block.type), html=document.leaf_html(block),
            polygon=block.polygon, bbox=block.bbox, section_hierarchy=block.section_hierarchy or None, images=images,
        )

    def __call__(self, document: _Rendered) -> JSONOutput:
        pages = []
        for page in document.pages:
            pid = BlockId(page.page_id, page.page_id, BlockTypes.Page)
            pages.append(JSONBlockOutput(
                id=str(pid), block_type=str(BlockTypes.Page),
                html="".join(f"<content-ref src='{b.id}'></content-ref>" for b in page.blocks),
                polygon=page.polygon, bbox=page.bbox,
                children=[self._block(document, b) for b in page.blocks],
            ))
        return self._with_document(JSONOutput(children=pages, metadata=document.metadata()), document)


class ChunkRenderer(BaseRenderer):
    def __call__(self, document: _Rendered) -> ChunkOutput:
        blocks = []
        info: Dict[int, dict] = {}
        for page in document.pages:
            info[page.page_id] = {"bbox": page.bbox, "polygon": page.polygon}
            for block in page.blocks:
                images = {}
                stack = [block]
                while stack:
                    b = stack.pop()
                    if b.image_name and b.image_name in document.images:
                        images[str(b.id)] = _image_b64(document.images[b.image_name])
                    stack.extend(b.children)
                blocks.append(FlatBlockOutput(
                    id=str(block.id), block_type=str(block.type), html=document.spliced_html(block),
                    page=page.page_id, polygon=block.polygon, bbox=block.bbox,
                    section_hierarchy=block.section_hierarchy or None, images=images or None,
                ))
        return self._with_document(ChunkOutput(blocks=blocks, page_info=info, metadata=document.metadata()), document)


class OCRJSONRenderer(BaseRenderer):
    def __call__(self, document: _Rendered) -> OCRJSONOutput:
        pages = []
        for page in document.pages:
            lines = []
            stack = list(reversed(page.blocks))
            while stack:
                b = stack.pop()
                if b.children:
                    stack.extend(reversed(b.children))
                    continue
                text = getattr(b.item, "text", None)
                if text is None:
                    continue
                lines.append(OCRJSONLineOutput(
                    id=str(BlockId(page.page_id, b.id.block_id, BlockTypes.Line)), block_type=str(BlockTypes.Line),
                    html=_html.escape(text), polygon=b.polygon, bbox=b.bbox,
                ))
            pid = BlockId(page.page_id, page.page_id, BlockTypes.Page)
            pages.append(OCRJSONPageOutput(id=str(pid), block_type=str(BlockTypes.Page), polygon=page.polygon, bbox=page.bbox, children=lines))
        return self._with_document(OCRJSONOutput(children=pages, metadata=None), document)


_RENDERERS = {
    "markdown": MarkdownRenderer, "html": HTMLRenderer, "json": JSONRenderer, "chunks": ChunkRenderer, "ocr_json": OCRJSONRenderer,
    "MarkdownRenderer": MarkdownRenderer, "HTMLRenderer": HTMLRenderer, "JSONRenderer": JSONRenderer,
    "ChunkRenderer": ChunkRenderer, "OCRJSONRenderer": OCRJSONRenderer,
}


def _resolve_renderer(renderer: Any) -> type:
    if renderer is None:
        return MarkdownRenderer
    if isinstance(renderer, type):
        return renderer
    name = str(renderer).rsplit(".", 1)[-1]
    try:
        return _RENDERERS[name]
    except KeyError:
        raise ValueError(f"unknown renderer {renderer!r}; one of {sorted(set(_RENDERERS))}") from None


# ---- config ------------------------------------------------------------------


def parse_range_str(range_str: str) -> List[int]:
    """Marker's ``parse_range_str``: ``"0,5-10,20"`` → sorted unique ints."""
    pages = set()
    for part in str(range_str).split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            a, b = part.split("-", 1)
            pages.update(range(int(a), int(b) + 1))
        else:
            pages.add(int(part))
    return sorted(pages)


def _assign_config(obj: Any, config: Any) -> None:
    """Marker's ``assign_config``: copy matching keys of a dict / model onto
    the object, including ``<ClassName>_<key>`` prefixed ones."""
    if config is None:
        return
    if isinstance(config, BaseModel):
        config = config.model_dump()
    if not isinstance(config, dict):
        raise ValueError("config must be a dict or a pydantic model")
    prefix = type(obj).__name__ + "_"
    for key, value in config.items():
        if hasattr(obj, key):
            setattr(obj, key, value)
    for key, value in config.items():
        if key.startswith(prefix) and hasattr(obj, key[len(prefix):]):
            setattr(obj, key[len(prefix):], value)


# Marker options the engine has no counterpart for: accepted, one warning
# when set to something other than Marker's default.
_IGNORED = {
    "use_llm": False, "llm_service": None, "block_correction_prompt": None, "redo_inline_math": False,
    "mode": None, "processors": None, "debug": False, "pdftext_workers": None, "disable_multiprocessing": False,
    "force_layout_block": None, "add_block_ids": False, "image_extraction_mode": "highres",
    "html_tables_in_markdown": False, "lowres_image_dpi": 96, "gemini_api_key": None, "keep_chars": False,
    "disable_tqdm": False, "ocr_full_page": True,
}

_warned: set = set()


def _warn_once(key: str, message: str) -> None:
    if key not in _warned:
        _warned.add(key)
        warnings.warn(message, stacklevel=3)


def _engine_kwargs(config: Dict[str, Any]) -> Dict[str, Any]:
    """Marker's config keys → :class:`docling_rs.DocumentConverter` keyword
    arguments."""
    kwargs: Dict[str, Any] = {}
    if config.get("disable_ocr"):
        kwargs["do_ocr"] = False
    if config.get("force_ocr") or config.get("strip_existing_ocr"):
        kwargs["force_full_page_ocr"] = True
    langs = config.get("languages")
    if langs:
        first = (langs if isinstance(langs, str) else list(langs)[0]).split(",")[0].strip().lower()
        kwargs["ocr_lang"] = "ch" if first in ("zh", "ch", "chi_sim", "zh-hans", "zh-cn") else first
    dpi = config.get("highres_image_dpi")
    if dpi:
        kwargs["images_scale"] = max(0.1, min(4.0, float(dpi) / 72.0))
    if config.get("extract_images", True) is False:
        kwargs["keep_picture_images"] = False
    for key, default in _IGNORED.items():
        if key in config and config[key] != default and config[key] not in (None, False, [], ""):
            _warn_once(key, f"docling_rs.marker: {key}={config[key]!r} is accepted and ignored — docling.rs has no counterpart")
    kwargs.update(config.get("docling_options") or {})
    return kwargs


def _page_window(config: Dict[str, Any]) -> Tuple[Optional[Tuple[int, int]], Optional[set]]:
    """Marker's 0-based ``page_range`` list → the engine's 1-based inclusive
    window and the set of pages to keep from it."""
    pages = config.get("page_range")
    if pages is None:
        return None, None
    if isinstance(pages, str):
        pages = parse_range_str(pages)
    pages = sorted({int(p) for p in pages})
    if not pages:
        return None, None
    return (pages[0] + 1, pages[-1] + 1), set(pages)


class ConfigParser:
    """Marker's ``ConfigParser``: CLI options in, the converter's pieces out."""

    def __init__(self, cli_options: dict):
        self.cli_options = dict(cli_options)

    def generate_config_dict(self) -> Dict[str, Any]:
        config: Dict[str, Any] = {}
        output_dir = self.cli_options.get("output_dir", settings.OUTPUT_DIR)
        for key, value in self.cli_options.items():
            if value is None:
                continue
            if key == "debug" and value:
                config["debug_pdf_images"] = True
                config["debug_layout_images"] = True
                config["debug_json"] = True
                config["debug_data_folder"] = output_dir
            elif key == "page_range":
                config["page_range"] = parse_range_str(value) if isinstance(value, str) else value
            elif key == "config_json":
                with open(value, encoding="utf-8") as fh:
                    config.update(json.load(fh))
            elif key == "disable_multiprocessing":
                if value:
                    config["pdftext_workers"] = 1
            elif key == "disable_image_extraction":
                if value:
                    config["extract_images"] = False
            else:
                config[key] = value
        if settings.GOOGLE_API_KEY:
            config["gemini_api_key"] = settings.GOOGLE_API_KEY
        return config

    def get_renderer(self) -> str:
        fmt = self.cli_options.get("output_format") or "markdown"
        names = {"markdown": "MarkdownRenderer", "json": "JSONRenderer", "html": "HTMLRenderer", "chunks": "ChunkRenderer"}
        if fmt not in names:
            raise ValueError(f"Unknown output format: {fmt}")
        return f"docling_rs.marker.renderers.{fmt}.{names[fmt]}"

    def get_processors(self) -> Optional[List[str]]:
        processors = self.cli_options.get("processors")
        return [p.strip() for p in processors.split(",") if p.strip()] if processors else None

    def get_converter_cls(self) -> type:
        name = self.cli_options.get("converter_cls")
        if not name:
            return PdfConverter
        return _resolve_converter(name)

    def get_llm_service(self) -> Optional[str]:
        if not self.cli_options.get("use_llm"):
            return None
        return self.cli_options.get("llm_service") or "marker.services.gemini.GoogleGeminiService"

    def get_output_folder(self, filepath: str) -> str:
        folder = os.path.join(self.cli_options.get("output_dir") or settings.OUTPUT_DIR, self.get_base_filename(filepath))
        os.makedirs(folder, exist_ok=True)
        return folder

    def get_base_filename(self, filepath: str) -> str:
        return os.path.splitext(os.path.basename(str(filepath)))[0]


# ---- models ------------------------------------------------------------------


class _Engine(dict):
    """What ``create_model_dict`` returns: a dict with Marker's keys so code
    that indexes them keeps working, every value the docling.rs engine
    marker. Nothing is loaded here — the ONNX sessions start on the first
    conversion, once per converter."""


def _models_present() -> bool:
    """Whether every required PDF model resolves — the working directory's
    ``.models/``, ``$DOCLING_RS_MODELS_DIR`` or the cache."""
    roots = [Path.cwd() / ".models", _models.cache_dir() / "models"]
    if env := os.environ.get("DOCLING_RS_MODELS_DIR"):
        roots.append(Path(env))
    for rel in _models._REQUIRED.values():
        name = str(rel).split("/", 1)[1] if str(rel).startswith("models/") else str(rel)
        if not any((root / name).exists() for root in roots):
            return False
    return True


def create_model_dict(device=None, dtype=None, attention_implementation: Optional[str] = None,
                      inference_manager: Any = None, inference_backend: Optional[str] = None, **_: Any) -> dict:
    """Marker's ``create_model_dict``: the arguments are accepted for
    call-site compatibility. docling.rs's models are fetched into
    ``~/.cache/docling.rs`` when they are missing (``download_models``), as
    Marker fetches Surya's on first use; a PDF converts from its text layer
    until then."""
    if not _models_present():
        try:
            _models.download_models(enrichment=False)
        except Exception as e:  # noqa: BLE001 — offline: the text layer still converts
            _warn_once("models", f"docling_rs.marker: could not fetch the models ({e}); PDFs convert from their text layer")
    _models.ensure_env()
    return _Engine(
        docling_rs=True, layout_model="docling.rs", fast_layout_model="docling.rs",
        recognition_model="docling.rs", ocr_error_model="docling.rs", inference_manager=None,
    )


# ---- converters --------------------------------------------------------------


class BaseConverter:
    """Marker's ``BaseConverter``: ``config`` in, ``__call__(filepath)`` out."""

    default_processors: Tuple = ()
    default_renderer: type = MarkdownRenderer

    def __init__(self, config: Optional[Union[dict, BaseModel]] = None):
        if isinstance(config, BaseModel):
            config = config.model_dump()
        self.config: Dict[str, Any] = dict(config or {})
        self.llm_service = None
        self.page_count: Optional[int] = None

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        raise NotImplementedError


class PdfConverter(BaseConverter):
    """Marker's ``PdfConverter`` — every format docling.rs reads, not only PDF.

    ``artifact_dict`` is ``create_model_dict()``'s (any dict, or ``None``);
    ``processor_list`` and ``llm_service`` are accepted and ignored;
    ``renderer`` is one of the renderer classes or its Marker / docling.rs
    import path; ``config`` the Marker options (``page_range``,
    ``disable_ocr``, ``force_ocr``, ``paginate_output``, ``extract_images``,
    ``keep_pageheader_in_output``, ``keep_pagefooter_in_output``,
    ``highres_image_dpi``, ``languages`` …) plus ``docling_options``."""

    converter_block_types: Optional[Tuple[BlockTypes, ...]] = None
    _engine_extra: Dict[str, Any] = {}

    def __init__(self, artifact_dict: Optional[dict] = None, processor_list: Optional[List[str]] = None,
                 renderer: Any = None, llm_service: Any = None, config: Optional[Union[dict, BaseModel]] = None):
        super().__init__(config)
        self.artifact_dict = artifact_dict if artifact_dict is not None else {}
        if processor_list:
            _warn_once("processor_list", "docling_rs.marker: processor_list is accepted and ignored — docling.rs runs its own pipeline")
        if llm_service:
            _warn_once("llm_service", "docling_rs.marker: llm_service is accepted and ignored — docling.rs runs no LLM")
        self.processor_list = processor_list
        self.renderer = renderer
        self.llm_service = llm_service
        window, keep = _page_window(self.config)
        self._window = window
        self._keep = keep
        kwargs = _engine_kwargs(self.config)
        kwargs.update(self._engine_extra)
        self._engine = DocumentConverter(**kwargs)

    # Marker calls the result `document` and keeps `page_count` on itself.
    def build_document(self, filepath: Union[str, os.PathLike, io.BytesIO]) -> _Rendered:
        source = filepath
        tmp = None
        try:
            if isinstance(filepath, (io.BytesIO, io.BufferedIOBase)):
                from ..markitdown import sniff_extension

                data = filepath.getvalue() if hasattr(filepath, "getvalue") else filepath.read()
                suffix = sniff_extension(data) or ".pdf"
                fd, tmp = tempfile.mkstemp(suffix=suffix)
                with os.fdopen(fd, "wb") as fh:
                    fh.write(data)
                source = tmp
            result = self._engine.convert(source, page_range=self._window)
        finally:
            if tmp:
                os.unlink(tmp)
        document = result.document
        options = dict(self.config)
        options["keep_pages"] = self._keep
        options.setdefault("extract_images", True)
        if self.converter_block_types:
            options["block_types"] = set(self.converter_block_types)
        rendered = _Rendered(document, options)
        if self.converter_block_types:
            for page in rendered.pages:
                page.blocks = [b for b in page.blocks if b.type in self.converter_block_types
                               or any(c.type in self.converter_block_types for c in b.children)]
        self.page_count = len(document.pages) if getattr(document, "pages", None) else len(rendered.pages)
        return rendered

    def resolve_dependencies(self, cls: type) -> Any:
        return cls(config=self.config)

    def __call__(self, filepath: Union[str, os.PathLike, io.BytesIO]) -> Any:
        rendered = self.build_document(filepath)
        renderer = _resolve_renderer(self.renderer or self.default_renderer)(self.config)
        return renderer(rendered)


class TableConverter(PdfConverter):
    """Marker's ``TableConverter``: only the ``Table`` / ``Form`` /
    ``TableOfContents`` blocks of each page, OCR off."""

    converter_block_types = _TABLE_TYPES
    _engine_extra = {"do_ocr": False}

    def __init__(self, *args: Any, **kwargs: Any):
        super().__init__(*args, **kwargs)
        self.config["disable_ocr"] = True


class OCRConverter(PdfConverter):
    """Marker's ``OCRConverter``: OCR forced on every page, the result as
    ``OCRJSONOutput`` — one ``Line`` per text block."""

    default_renderer = OCRJSONRenderer
    _engine_extra = {"force_full_page_ocr": True}

    def __init__(self, *args: Any, **kwargs: Any):
        super().__init__(*args, **kwargs)
        self.config["force_ocr"] = True
        if self.renderer is None:
            self.renderer = OCRJSONRenderer


_CONVERTERS = {"PdfConverter": PdfConverter, "TableConverter": TableConverter, "OCRConverter": OCRConverter}


def _resolve_converter(name: Any) -> type:
    if isinstance(name, type):
        return name
    short = str(name).rsplit(".", 1)[-1]
    try:
        return _CONVERTERS[short]
    except KeyError:
        raise ValueError(f"unknown converter {name!r}; one of {sorted(_CONVERTERS)}") from None


# ---- output helpers (marker.output) --------------------------------------------


def text_from_rendered(rendered: Any) -> Tuple[str, str, dict]:
    """Marker's ``text_from_rendered``: ``(text, extension, images)``."""
    if isinstance(rendered, MarkdownOutput):
        return rendered.markdown, "md", rendered.images
    if isinstance(rendered, HTMLOutput):
        return rendered.html, "html", rendered.images
    if isinstance(rendered, (JSONOutput, ChunkOutput, OCRJSONOutput)):
        return rendered.model_dump_json(exclude={"metadata"}, indent=2), "json", {}
    raise ValueError("Invalid output type")


def convert_if_not_rgb(image: Any) -> Any:
    return image.convert("RGB") if getattr(image, "mode", "RGB") != "RGB" else image


def save_output(rendered: Any, output_dir: Union[str, os.PathLike], fname_base: str) -> None:
    """Marker's ``save_output``: ``<fname_base>.<ext>``, ``<fname_base>_meta.json``
    and the images, in ``output_dir``."""
    text, ext, images = text_from_rendered(rendered)
    os.makedirs(output_dir, exist_ok=True)
    with open(os.path.join(output_dir, f"{fname_base}.{ext}"), "w+", encoding=settings.OUTPUT_ENCODING) as fh:
        fh.write(text)
    with open(os.path.join(output_dir, f"{fname_base}_meta.json"), "w+", encoding=settings.OUTPUT_ENCODING) as fh:
        fh.write(json.dumps(getattr(rendered, "metadata", None) or {}, indent=2))
    for name, image in images.items():
        convert_if_not_rgb(image).save(os.path.join(output_dir, name), settings.OUTPUT_IMAGE_FORMAT)


def output_exists(output_dir: Union[str, os.PathLike], fname_base: str) -> bool:
    return any(os.path.exists(os.path.join(output_dir, f"{fname_base}.{ext}")) for ext in ("md", "html", "json"))


def unwrap_outer_tag(html: str) -> str:
    m = re.fullmatch(r"\s*<p>(.*)</p>\s*", html, flags=re.S)
    return m.group(1) if m and "<p>" not in m.group(1) else html


def json_to_html(block: Any) -> str:
    """Marker's ``json_to_html``: a JSON block's HTML with its children
    spliced in for the ``<content-ref>`` placeholders."""
    html = block.html
    for child in getattr(block, "children", None) or []:
        html = html.replace(f"<content-ref src='{child.id}'></content-ref>", json_to_html(child))
    return html
