"""Pandoc AST (JSON) export — ``docling-rs --to pandoc`` for Python (#515).

docling's Python ``DoclingDocument`` has no Pandoc exporter; this runs the
Rust serializer on the document's JSON, so the output is byte-identical to the
CLI's and ``pandoc -f json`` reads it::

    from docling_rs import DocumentConverter
    from docling_rs.pandoc import export_to_pandoc, save_as_pandoc

    doc = DocumentConverter().convert("report.pdf").document
    ast = export_to_pandoc(doc)                       # str: Pandoc JSON
    save_as_pandoc(doc, "report.pandoc.json")         # then: pandoc -f json report.pandoc.json -o report.docx

The mapping (headings, lists, tables with spans, figures, code, formulas,
footnotes) and the elements Pandoc has no node for are documented in the
README's "Pandoc AST output" section.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Optional, Union

from ._native import pandoc_from_json as _pandoc_from_json

__all__ = ["PANDOC_API_VERSION", "export_to_pandoc", "save_as_pandoc"]

#: The pandoc-types API version the serializer writes (Pandoc 3.x).
PANDOC_API_VERSION = (1, 23, 1, 1)


def _document_json(doc: Any) -> str:
    if isinstance(doc, str):
        return doc
    if isinstance(doc, dict):
        return json.dumps(doc)
    if hasattr(doc, "export_to_dict"):
        return json.dumps(doc.export_to_dict())
    raise TypeError(
        "expected a DoclingDocument, a docling-JSON dict, or a JSON string; "
        f"got {type(doc).__name__}"
    )


def _mode(image_mode: Any) -> str:
    # docling's ImageRefMode is a str enum ("placeholder", "embedded",
    # "referenced"); plain strings work too.
    return str(getattr(image_mode, "value", image_mode))


def export_to_pandoc(
    doc: Any,
    *,
    image_mode: Any = "embedded",
    api_version: Optional[str] = None,
    heading_depth: str = "docling",
    track_changes: Optional[str] = None,
) -> str:
    """The document as Pandoc's JSON AST.

    ``doc`` is a ``DoclingDocument``, a docling-JSON dict or a JSON string.
    ``image_mode`` is ``"embedded"`` (default: pictures as data URIs, so
    ``pandoc -t docx`` rebuilds them, #537) or ``"placeholder"`` (an
    ``Image`` with no target, classed ``docling-placeholder``); use
    :func:`save_as_pandoc` for ``"referenced"`` images.
    ``api_version`` (e.g. ``"1.23"``) must name the version the serializer
    writes; any other raises ``ValueError``.
    ``heading_depth`` is the ``Header`` depth a section heading gets (#848):
    ``"docling"`` (default: the title is ``Header 1``, a level-*n* section
    heading ``Header n+1`` — docling's Markdown / HTML depth) or ``"source"``
    (``Header n``: a DOCX ``Heading 1`` is ``Header 1``, as ``pandoc -f docx``
    reads it).
    ``track_changes`` is the policy the document was converted under (#851):
    ``"accept"`` or ``"reject"`` leave the reviewer comments out of the AST,
    as Pandoc's ``--track-changes=accept|reject`` do; ``None`` (default) and
    ``"all"`` write them.
    """
    mode = _mode(image_mode)
    if mode == "referenced":
        raise ValueError("image_mode='referenced' writes files; use save_as_pandoc()")
    ast, _ = _pandoc_from_json(
        _document_json(doc), mode, "artifacts", api_version, heading_depth, track_changes
    )
    return ast


def save_as_pandoc(
    doc: Any,
    filename: Union[str, Path],
    *,
    image_mode: Any = "embedded",
    artifacts_dir: Optional[Union[str, Path]] = None,
    api_version: Optional[str] = None,
    heading_depth: str = "docling",
    track_changes: Optional[str] = None,
) -> None:
    """Write the Pandoc JSON AST to ``filename``. With
    ``image_mode="referenced"`` the pictures are written under
    ``artifacts_dir`` (default ``<stem>_artifacts`` next to the file) and
    linked by their path relative to the file, like the CLI's batch output."""
    filename = Path(filename)
    mode = _mode(image_mode)
    if artifacts_dir is None:
        stem = filename.name.removesuffix(".json").removesuffix(".pandoc")
        artifacts_dir = filename.parent / f"{stem}_artifacts"
    artifacts_dir = Path(artifacts_dir)
    try:
        link_dir = artifacts_dir.relative_to(filename.parent)
    except ValueError:
        link_dir = artifacts_dir
    ast, images = _pandoc_from_json(
        _document_json(doc),
        mode,
        link_dir.as_posix(),
        api_version,
        heading_depth,
        track_changes,
    )
    for rel, data in images:
        path = filename.parent / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    filename.write_text(ast, encoding="utf-8")
