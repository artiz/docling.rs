#!/usr/bin/env python3
"""Full declarative conformance sweep: Markdown byte-for-byte *and* JSON
structurally, every format at once, against the docling installed in the
running Python.

`conformance.sh <fmt>` scores one format's Markdown against live docling. This
runs the whole `tests/data` corpus in one go and also compares the JSON export
the way the backends' own tests do: every key and value except `origin` /
`version` (hashes and schema stamps) and an image's `uri` (PIL re-encodes the
bytes). Upstream is driven backend-directly like `docling_convert.py`, so a
slim install without torch is enough:

    uv venv .venv-compare
    uv pip install --python .venv-compare/bin/python \\
        "docling-slim[format-docx,format-email,format-html,format-iwork,format-latex,\\
format-markdown,format-office,format-opendocument,format-pptx,format-xlsx,\\
format-xml-jats,format-xml-uspto,format-xml-xbrl]" pypdfium2 pylatexenc
    cargo build -p docling-cli
    .venv-compare/bin/python scripts/conformance/full_conformance.py [fmt ...]

Prints a per-format summary (files upstream converts, Markdown exact,
Markdown exact after whitespace normalization, JSON identical, upstream
errors) and every file that is not exact on both, then writes the raw rows to
`target/full_conformance.json`. Formats upstream cannot convert without the ML
pipeline (PDF, images, audio) or that are docling.rs extensions (`.numbers`,
`.key`, `.msg`, RTF, …) are skipped; a file upstream itself fails on counts as
an error, not a miss.
"""

import difflib
import glob
import json
import logging
import os
import re
import subprocess
import sys
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
import docling_convert as dc  # noqa: E402

from docling.datamodel.base_models import InputFormat  # noqa: E402
from docling.datamodel.document import InputDocument  # noqa: E402

CLI = os.environ.get("DOCLING_RS_BIN", str(ROOT / "target/debug/docling-rs"))

DEFAULT_FORMATS = [
    "asciidoc", "csv", "docx", "html", "xlsx", "pptx", "md", "md_deepseek",
    "latex", "jats", "uspto", "xbrl", "webvtt", "odf", "epub", "email", "iwork",
    "doclang", "ebcdic",
]

for ext, name in [
    ("tex", "LATEX"), ("latex", "LATEX"), ("pages", "IWORK_PAGES"), ("ebc", "EBCDIC"),
    ("xhtml", "HTML"), ("pptm", "PPTX"), ("xltx", "XLSX"), ("xltm", "XLSX"),
]:
    if hasattr(InputFormat, name):
        dc.EXT_TO_FORMAT.setdefault(ext, getattr(InputFormat, name))


def backend_for(fmt):
    try:
        return dc.backend_for(fmt)
    except SystemExit:
        pass
    if fmt.name == "LATEX":
        from docling.backend.latex_backend import LatexDocumentBackend

        return LatexDocumentBackend
    if fmt.name == "IWORK_PAGES":
        from docling.backend.iwork_backend import IWorkPagesDocumentBackend

        return IWorkPagesDocumentBackend
    if fmt.name == "EBCDIC":
        from docling.backend.ebcdic_backend import EbcdicDocumentBackend

        return EbcdicDocumentBackend
    if fmt.name == "XML_XBRL":
        from docling.backend.xml.xbrl_backend import XBRLDocumentBackend

        return XBRLDocumentBackend
    raise SystemExit(f"no declarative backend wired for format: {fmt}")


def upstream(path: Path):
    """(markdown, json dict) from the installed docling, or None to skip."""
    ext = path.suffix.lower().lstrip(".")
    fmt = dc.EXT_TO_FORMAT.get(ext)
    if ext in ("xml", "txt"):
        # The XML families share an extension; sniff like docling's format
        # detection does. A `.txt` is a USPTO `pftaps` patent when it opens
        # with a PATN record, otherwise Markdown/plain text.
        head = path.read_text(encoding="utf-8", errors="ignore")[:4000]
        if ext == "xml" and "xbrl" in head.lower():
            fmt = InputFormat.XML_XBRL
        elif ext == "xml" or "<article" in head:
            fmt = dc._sniff_xml(path)
        elif head.startswith("PATN"):
            fmt = InputFormat.XML_USPTO
    if ext == "pages":
        fmt = InputFormat.IWORK_PAGES
    if fmt is None or fmt in dc._PIPELINE_FORMATS:
        return None
    if fmt == InputFormat.MD and dc._is_deepseek(path.read_text(encoding="utf-8")):
        return dc._convert_deepseek(path), None
    backend_cls = backend_for(fmt)
    kwargs = {}
    if fmt.name == "EBCDIC":
        # The corpus keeps each record layout in a `<stem>.layout.json`
        # sidecar, which is also the Rust CLI's default.
        from docling.backend.ebcdic_backend import EbcdicBackendOptions

        kwargs["options"] = EbcdicBackendOptions(layout_file=path.with_suffix(".layout.json"))
    in_doc = InputDocument(path_or_stream=path, format=fmt, backend=backend_cls, filename=path.name)
    doc = backend_cls(path_or_stream=path, in_doc=in_doc, **kwargs).convert()
    # The JSON first: docling-core's Markdown serializer clamps every
    # provenance box to its page *in place* (`_clamp_bbox_to_page`), so a
    # dict exported after it would carry the clamped boxes, not the
    # backend's.
    as_dict = doc.export_to_dict()
    return doc.export_to_markdown(), as_dict


def strip(o):
    """Drop what a structural comparison never looks at."""
    if isinstance(o, dict):
        return {k: ("<uri>" if k == "uri" else strip(v)) for k, v in o.items() if k not in ("origin", "version")}
    if isinstance(o, list):
        return [strip(x) for x in o]
    return o


def norm(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def diff_lines(a: str, b: str) -> int:
    return sum(
        1
        for line in difflib.unified_diff(a.splitlines(), b.splitlines(), lineterm="", n=0)
        if line[:1] in "+-" and not line.startswith(("+++", "---"))
    )


def run_cli(*args: str) -> str:
    return subprocess.run([CLI, *args], capture_output=True, text=True).stdout


def main() -> None:
    formats = sys.argv[1:] or DEFAULT_FORMATS
    summary, rows = [], []
    for fmt in formats:
        n = md_exact = md_norm = json_ok = errors = 0
        for src in sorted(glob.glob(str(ROOT / "tests/data" / fmt / "sources" / "*"))):
            path = Path(src)
            if path.is_dir() or path.name.endswith(".layout.json"):
                continue
            try:
                ref = upstream(path)
            except (Exception, SystemExit) as exc:  # noqa: BLE001 — upstream's failure is the datum
                rows.append((fmt, path.name, "UPSTREAM-ERR", str(exc)[:90]))
                errors += 1
                continue
            if ref is None:
                continue
            ref_md, ref_json = ref
            n += 1
            ours_md = run_cli(src)
            strict = diff_lines(ours_md.rstrip("\n"), ref_md.rstrip("\n"))
            loose = diff_lines(norm(ours_md), norm(ref_md))
            md_exact += strict == 0
            md_norm += loose == 0
            json_state = "n/a"
            if ref_json is not None:
                ours = strip(json.loads(run_cli("--to", "json", src) or "{}"))
                want = strip(ref_json)
                ours.pop("name", None)
                want.pop("name", None)
                json_state = "ok" if ours == want else "DIFF"
                json_ok += json_state == "ok"
            rows.append((fmt, path.name, f"md={strict}/{loose}", f"json={json_state}"))
        summary.append((fmt, n, md_exact, md_norm, json_ok, errors))

    print(f"{'format':12s} {'files':>5s} {'md-exact':>9s} {'md-norm':>8s} {'json':>5s} {'err':>4s}")
    for fmt, n, mdx, mdn, jx, err in summary:
        print(f"{fmt:12s} {n:5d} {mdx:9d} {mdn:8d} {jx:5d} {err:4d}")
    print()
    for fmt, name, md, js in rows:
        if md == "md=0/0" and js in ("json=ok", "json=n/a"):
            continue
        print(f"{fmt:12s} {name:50s} {md:14s} {js}")
    out = ROOT / "target" / "full_conformance.json"
    out.parent.mkdir(exist_ok=True)
    json.dump({"summary": summary, "rows": rows}, out.open("w"), indent=1)


if __name__ == "__main__":
    main()
