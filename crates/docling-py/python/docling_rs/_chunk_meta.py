"""A native chunk as docling's ``DocChunk``: item refs resolved against the
document, headings and origin, exported the way docling-core's ``DocMeta``
does. Shared by the LangChain loader and the AutoGen indexer, so both hand
out the same ``dl_meta`` and the same page provenance."""

from __future__ import annotations

from typing import Any, Dict, Optional

from .chunking import DocChunk as _NativeChunk


class ResolvedMeta:
    """docling-core's chunk ``DocMeta`` for a native chunk — the chunk's
    items resolved from their refs, its headings and the document origin —
    with docling's ``export_json_dict()``. Built here rather than as
    docling-core's own ``DocMeta`` because importing
    ``docling_core.transforms.chunker`` needs the optional
    ``docling-core[chunking]`` extra (tree-sitter et al.)."""

    schema_name = "docling_core.transforms.chunker.DocMeta"
    version = "1.0.0"

    def __init__(self, doc_items: list, headings: Optional[list], origin: Any):
        self.doc_items = doc_items
        self.headings = headings or None
        self.origin = origin

    def export_json_dict(self) -> Dict[str, Any]:
        """``DocMeta.export_json_dict()``: ``model_dump(mode="json",
        by_alias=True, exclude_none=True)``, the items serialized through
        the field's declared ``DocItem`` type as pydantic does (base fields
        only — ``label``, ``prov``, refs; no ``text``)."""
        from pydantic import TypeAdapter
        from docling_core.types.doc import DocItem

        dump = dict(mode="json", by_alias=True, exclude_none=True)
        out: Dict[str, Any] = {
            "schema_name": self.schema_name,
            "version": self.version,
            "doc_items": [TypeAdapter(DocItem).dump_python(it, **dump) for it in self.doc_items],
        }
        if self.headings:
            out["headings"] = list(self.headings)
        if self.origin is not None:
            out["origin"] = self.origin.model_dump(**dump)
        return out


class ResolvedChunk:
    """A native chunk as docling's ``DocChunk`` shape: ``text`` + resolved
    ``meta``."""

    def __init__(self, text: str, meta: ResolvedMeta):
        self.text = text
        self.meta = meta


def docling_chunk(chunk: Any, dl_doc: Any) -> Any:
    """docling's ``DocChunk`` view of a native chunk: its item refs
    (``"#/texts/3"``) resolved against the document, with the headings and the
    document origin — what docling's own chunkers yield, so a meta extractor
    sees the same ``chunk.meta`` either way. Chunks that are not native (a
    docling chunker's) pass through, as do native ones without items (the
    word-window chunker, which works on Markdown, not the item tree)."""
    if not isinstance(chunk, _NativeChunk) or not chunk.meta.doc_items:
        return chunk
    from docling_core.types.doc.document import RefItem

    items = []
    for ref in chunk.meta.doc_items:
        try:
            items.append(RefItem(cref=ref).resolve(dl_doc))
        except Exception:
            continue
    if not items:
        return chunk
    return ResolvedChunk(chunk.text, ResolvedMeta(items, chunk.meta.headings, dl_doc.origin))
