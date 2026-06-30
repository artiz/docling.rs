#!/usr/bin/env python3
"""Dump EVERYTHING docling extracts for a PDF, for conformance diagnosis.

For each source PDF we emit a JSON with:
  - markdown         : docling's export_to_markdown (the conformance reference)
  - items            : every body item in reading order — label, page, bbox
                       (top-left, page-point coords), and text (raw, untruncated)
  - parse_cells      : docling-parse textline cells per page (text + bbox), the
                       parser layer our pdfium glyph pipeline is trying to match

Usage:
  python scripts/docling_dump_all.py <pdf> [<pdf> ...] --out <dir>
"""
import argparse
import json
import sys
from pathlib import Path


def parse_cells(path):
    """docling-parse textline + word cells per page (text + bbox)."""
    try:
        from docling_parse.pdf_parser import DoclingPdfParser
    except Exception as e:
        return {"error": f"docling_parse import: {e}"}
    p = DoclingPdfParser()
    doc = p.load(str(path))
    pages = {}
    for pno, page in doc.iterate_pages():
        d = page.export_to_dict()
        dim = d.get("dimension", {})
        def cell(c):
            r = c.get("rect", {})
            return {"text": c.get("text", ""),
                    "x0": r.get("r_x0"), "y0": r.get("r_y0"),
                    "x1": r.get("r_x1"), "y1": r.get("r_y1"),
                    "x2": r.get("r_x2"), "y2": r.get("r_y2"),
                    "x3": r.get("r_x3"), "y3": r.get("r_y3")}
        pages[pno] = {
            "dimension": dim,
            "textline_cells": [cell(c) for c in d.get("textline_cells", [])],
        }
    return pages


def docling_items(path):
    """Full docling pipeline: markdown + every body item with label/page/bbox/text."""
    from docling.document_converter import DocumentConverter, PdfFormatOption
    from docling.datamodel.base_models import InputFormat
    from docling.datamodel.pipeline_options import PdfPipelineOptions
    opts = PdfPipelineOptions()
    opts.do_ocr = False  # digital text-layer corpus; avoids easyocr model download
    opts.do_table_structure = True
    conv = DocumentConverter(
        format_options={InputFormat.PDF: PdfFormatOption(pipeline_options=opts)}
    )
    res = conv.convert(str(path))
    dd = res.document
    md = dd.export_to_markdown()
    items = []
    # iterate_items yields (item, level) in reading order
    for idx, (item, _level) in enumerate(dd.iterate_items()):
        label = getattr(item, "label", None)
        label = getattr(label, "value", label)
        text = getattr(item, "text", None)
        prov = getattr(item, "prov", None) or []
        bbox = None
        page_no = None
        if prov:
            pr = prov[0]
            page_no = getattr(pr, "page_no", None)
            bb = getattr(pr, "bbox", None)
            if bb is not None:
                # docling bbox: l,t,r,b with a coord origin; normalize to a dict
                bbox = {"l": getattr(bb, "l", None), "t": getattr(bb, "t", None),
                        "r": getattr(bb, "r", None), "b": getattr(bb, "b", None),
                        "origin": str(getattr(bb, "coord_origin", ""))}
        # tables: capture grid text too
        extra = None
        if label == "table" and hasattr(item, "data") and item.data is not None:
            try:
                extra = [[c.text for c in row] for row in item.data.grid]
            except Exception:
                extra = None
        items.append({"i": idx, "label": label, "page": page_no,
                      "bbox": bbox, "text": text, "grid": extra})
    return md, items


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pdfs", nargs="+")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    for pdf in args.pdfs:
        pdf = Path(pdf)
        stem = pdf.stem
        rec = {"source": str(pdf)}
        try:
            md, items = docling_items(pdf)
            rec["markdown"] = md
            rec["items"] = items
        except Exception as e:
            rec["items_error"] = repr(e)
        try:
            rec["parse_cells"] = parse_cells(pdf)
        except Exception as e:
            rec["parse_cells_error"] = repr(e)
        (out / f"{stem}.json").write_text(json.dumps(rec, ensure_ascii=False, indent=1))
        n = len(rec.get("items", []))
        print(f"{stem}: {n} items", file=sys.stderr)


if __name__ == "__main__":
    main()
