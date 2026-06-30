#!/usr/bin/env python3
"""Find text docling-parse sees on the page but our pipeline doesn't emit.

docling-parse's textline cells are the COMPLETE page text (every line, before
layout drops/relabels anything). We diff that bag of line-strings against our
pipeline's Markdown to surface text we silently drop (orphan cells, regions the
layout missed). Reports, per PDF, the docling-parse lines whose (normalized)
text does not appear anywhere in our Markdown output.

Usage: python scripts/parse_vs_ours.py <stem> [<stem> ...]
  reads tests/data/pdf/sources/<stem>.pdf and our output from
  <ours_dir>/<stem>.md (env OURS_DIR, default the scratch gen dir).
"""
import os
import re
import sys
from pathlib import Path

from docling_parse.pdf_parser import DoclingPdfParser


def norm(s):
    return re.sub(r"\s+", " ", s).strip()


def parse_lines(pdf):
    p = DoclingPdfParser()
    doc = p.load(str(pdf))
    lines = []
    for pno, page in doc.iterate_pages():
        d = page.export_to_dict()
        for c in d.get("textline_cells", []):
            t = norm(c.get("text", ""))
            if t:
                r = c.get("rect", {})
                lines.append((pno, t, r.get("r_x0"), r.get("r_y0")))
    return lines


def main():
    ours_dir = Path(os.environ.get("OURS_DIR", "."))
    for stem in sys.argv[1:]:
        pdf = Path(f"tests/data/pdf/sources/{stem}.pdf")
        ours = (ours_dir / f"{stem}.md")
        ours_txt = norm(ours.read_text()) if ours.exists() else ""
        missing = []
        for pno, t, x, y in parse_lines(pdf):
            # consider the line "present" if its text (or a long prefix) is a
            # substring of our normalized markdown
            probe = t if len(t) <= 60 else t[:60]
            if probe not in ours_txt:
                missing.append((pno, round(x or 0, 1), round(y or 0, 1), t))
        print(f"===== {stem}: {len(missing)} parse-lines not found in our output =====")
        for pno, x, y, t in missing[:40]:
            print(f"  p{pno} x={x} y={y} | {t[:90]}")


if __name__ == "__main__":
    main()
