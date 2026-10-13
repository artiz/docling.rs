#!/usr/bin/env python3
"""docling.rs vs Microsoft MarkItDown — the numbers in README's
"Conformance with Microsoft MarkItDown".

Run from the repo root with a release CLI (`cargo build --release -p
docling-cli`), the models (`scripts/install/download_dependencies.sh`) and
MarkItDown installed in the running Python (`pip install "markitdown[all]"`):

    python3 scripts/conformance/markitdown_compare.py corpus [FORMAT ...]
    python3 scripts/conformance/markitdown_compare.py ocr OCR_TEST_DATA [--llm]

`corpus` converts this repo's test corpus (tests/data/<fmt>/sources with a
docling groundtruth) with both tools: warm time (MarkItDown in-process after
one warm-up conversion; docling.rs batch mode, `--jobs 1`), cold CLI time on
the median-size file, and accuracy against docling's groundtruth Markdown —
word F1, table-cell recall, heading recall. The reference is Python
docling's output, which docling.rs is conformance-tested against, so the
accuracy columns favour docling.rs by construction.

`ocr` reads MarkItDown's own OCR test files
(packages/markitdown-ocr/tests/ocr_test_data in the MarkItDown repo) and
scores the share of the words drawn into their images / scans
(markitdown_ocr_truth.json, transcribed by hand) found in the output.
MarkItDown runs without an LLM; `--llm` adds its markitdown-ocr plugin with
gpt-4o (needs `pip install markitdown-ocr openai` and OPENAI_API_KEY), with
token usage and cost. docling.rs runs `--picture-ocr --force-full-page-ocr`
in batch mode.
"""
import collections
import glob
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CLI = os.path.join(ROOT, "target", "release", "docling-rs")
TRUTH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "markitdown_ocr_truth.json")
FORMATS = ["docx", "pptx", "xlsx", "html", "csv", "epub", "pdf"]
GPT4O_PRICE = (2.50, 10.00)  # USD per 1M input / output tokens


# ------------------------------------------------------------------ metrics

def strip_md(text):
    text = re.sub(r"<!--.*?-->", " ", text, flags=re.S)
    text = re.sub(r"!\[[^\]]*\]\([^)]*\)", " ", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"^\s*\|?\s*:?-{3,}.*$", " ", text, flags=re.M)
    return re.sub(r"[#*_`|>]", " ", text)


def words(text, markdown=True):
    return collections.Counter(re.findall(r"\w+", (strip_md(text) if markdown else text).lower()))


def f1(gt, got):
    inter = sum((gt & got).values())
    p, r = inter / max(1, sum(got.values())), inter / max(1, sum(gt.values()))
    return 0.0 if p + r == 0 else 2 * p * r / (p + r)


def table_cells(text):
    cells = collections.Counter()
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("|") and not re.match(r"^\|?\s*:?-{3,}", s):
            for c in s.strip("|").split("|"):
                c = " ".join(re.findall(r"\w+", c.lower()))
                if c:
                    cells[c] += 1
    return cells


def headings(text):
    return collections.Counter(
        " ".join(re.findall(r"\w+", h.lower())) for h in re.findall(r"^#{1,6}\s+(.*)$", text, flags=re.M)
    )


def recall(gt, got):
    total = sum(gt.values())
    return None if total == 0 else sum((gt & got).values()) / total


def mean(values):
    values = [v for v in values if v is not None]
    return round(statistics.mean(values), 3) if values else None


# ------------------------------------------------------------------ runners

def markitdown_warm(files, **kwargs):
    from markitdown import MarkItDown

    md = MarkItDown(**kwargs)
    md.convert(files[0])
    out = {}
    for f in files:
        t = time.perf_counter()
        try:
            text = md.convert(f).text_content
        except Exception as e:  # noqa: BLE001 — a failed file is a result
            text = f"ERROR: {type(e).__name__}: {e}"
        out[f] = (time.perf_counter() - t, text)
    return out


def docling_batch(files, extra, jobs=1):
    """One batch run over `files`; returns (seconds, {file: markdown})."""
    with tempfile.TemporaryDirectory() as tmp:
        src, dst = os.path.join(tmp, "src"), os.path.join(tmp, "out")
        os.makedirs(src)
        for f in files:
            os.symlink(f, os.path.join(src, os.path.basename(f)))
        t = time.perf_counter()
        subprocess.run(
            [CLI, *extra, "--jobs", str(jobs), "--input", f"{src}/*", "--output", dst, "--to", "md"],
            cwd=ROOT, capture_output=True,
        )
        secs = time.perf_counter() - t
        out = {}
        for f in files:
            stem = os.path.splitext(os.path.basename(f))[0]
            hits = glob.glob(f"{dst}/**/{glob.escape(stem)}.md", recursive=True)
            out[f] = open(hits[0], encoding="utf-8").read() if hits else "ERROR: no output"
        return secs, out


def cold_ms(cmd, f):
    runs = []
    for _ in range(3):
        t = time.perf_counter()
        subprocess.run(cmd + [f], cwd=ROOT, capture_output=True)
        runs.append(time.perf_counter() - t)
    return round(1000 * statistics.median(runs), 1)


# ------------------------------------------------------------------ corpus

def groundtruth(src):
    d = os.path.join(os.path.dirname(os.path.dirname(src)), "groundtruth")
    full = os.path.join(d, os.path.basename(src) + ".md")
    return full if os.path.exists(full) else os.path.join(d, os.path.splitext(os.path.basename(src))[0] + ".md")


def corpus(formats):
    report = {}
    for fmt in formats:
        files = sorted(
            f for f in glob.glob(f"{ROOT}/tests/data/{fmt}/sources/*")
            if os.path.isfile(f) and os.path.exists(groundtruth(f))
        )
        if not files:
            continue
        md = markitdown_warm(files, enable_plugins=False)
        runs = {"markitdown": (sum(t for t, _ in md.values()), {f: x for f, (_, x) in md.items()})}
        runs["docling.rs"] = docling_batch(files, [])
        if fmt == "pdf":
            runs["docling.rs --text-layer-only"] = docling_batch(files, ["--text-layer-only"])
        row = {}
        for name, (secs, texts) in runs.items():
            gts = {f: open(groundtruth(f), encoding="utf-8").read() for f in files}
            row[name] = {
                "files": len(files),
                "seconds": round(secs, 3),
                "errors": sum(texts[f].startswith("ERROR") for f in files),
                "word_f1": mean(f1(words(gts[f]), words(texts[f])) for f in files),
                "table_cell_recall": mean(recall(table_cells(gts[f]), table_cells(texts[f])) for f in files),
                "heading_recall": mean(recall(headings(gts[f]), headings(texts[f])) for f in files),
            }
        sample = sorted(files, key=os.path.getsize)[len(files) // 2]
        markitdown_cli = shutil.which("markitdown") or "markitdown"
        row["cold_cli_ms"] = {
            "file": os.path.basename(sample),
            "markitdown": cold_ms([markitdown_cli], sample),
            "docling.rs": cold_ms([CLI], sample),
        }
        report[fmt] = row
        print(fmt, json.dumps(row), file=sys.stderr)
    return report


# ------------------------------------------------------------------ ocr

def ocr(data, llm):
    truth = json.load(open(TRUTH, encoding="utf-8"))
    files = sorted(os.path.join(data, n) for n in truth)
    group = lambda n: ("Scanned PDF" if "scanned" in n else "PDF with images") if n.startswith("pdf_") else "Office pictures"

    def score(texts):
        g = collections.defaultdict(lambda: [0, 0])
        for f in files:
            n = os.path.basename(f)
            gt = words(" ".join(truth[n]), markdown=False)
            g[group(n)][0] += sum((gt & words(texts[f], markdown=False)).values())
            g[group(n)][1] += sum(gt.values())
        return {k: round(h / t, 3) for k, (h, t) in sorted(g.items())}

    report = {}
    md = markitdown_warm(files, enable_plugins=False)
    report["markitdown (no LLM)"] = {
        "seconds": round(sum(t for t, _ in md.values()), 2),
        **score({f: x for f, (_, x) in md.items()}),
    }
    if llm:
        from openai import OpenAI

        client = OpenAI()
        usage = collections.Counter()
        create = client.chat.completions.create

        def counted(*a, **kw):
            r = create(*a, **kw)
            usage["calls"] += 1
            if getattr(r, "usage", None):
                usage["in"] += r.usage.prompt_tokens or 0
                usage["out"] += r.usage.completion_tokens or 0
            return r

        client.chat.completions.create = counted
        md = markitdown_warm(files, enable_plugins=True, llm_client=client, llm_model="gpt-4o")
        report["markitdown + markitdown-ocr (gpt-4o)"] = {
            "seconds": round(sum(t for t, _ in md.values()), 2),
            "api_calls": usage["calls"],
            "tokens": usage["in"] + usage["out"],
            "usd": round(usage["in"] / 1e6 * GPT4O_PRICE[0] + usage["out"] / 1e6 * GPT4O_PRICE[1], 4),
            **score({f: x for f, (_, x) in md.items()}),
        }
    for jobs in (1, 4):
        secs, texts = docling_batch(files, ["--picture-ocr", "--force-full-page-ocr"], jobs)
        report[f"docling.rs --jobs {jobs}"] = {"seconds": round(secs, 2), **score(texts)}
    return report


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ("corpus", "ocr"):
        sys.exit(__doc__)
    if sys.argv[1] == "corpus":
        report = corpus(sys.argv[2:] or FORMATS)
    else:
        if len(sys.argv) < 3:
            sys.exit("usage: markitdown_compare.py ocr OCR_TEST_DATA [--llm]")
        report = ocr(sys.argv[2], "--llm" in sys.argv[3:])
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
