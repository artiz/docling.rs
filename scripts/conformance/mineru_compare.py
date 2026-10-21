#!/usr/bin/env python3
"""docling.rs vs OpenDataLab's MinerU — the numbers in README's
"Conformance with MinerU".

Run from the repo root with a release CLI (`cargo build --release -p
docling-cli`), the models (`scripts/install/download_dependencies.sh`) and
MinerU 4 installed in the running Python (`pip install "mineru>=4,<5"`; its
`basic` tier fetches its ONNX models on first use, `standard` its VLM and
llama.cpp):

    python3 scripts/conformance/mineru_compare.py corpus [--tiers flash,basic] [FORMAT ...]

`corpus` converts this repo's test corpus (tests/data/<fmt>/sources with a
docling groundtruth) with both tools: warm time (MinerU in-process through
`mineru.parser.parse` after a warm-up conversion; docling.rs batch mode,
`--jobs 1`), peak RSS of the conversion process tree, cold CLI time on the
median-size file (`mineru-kit parse` vs `docling-rs`), and accuracy against
docling's groundtruth Markdown — word F1, table-cell recall (MinerU writes
HTML tables: their `<td>` cells count), heading recall. The reference is
Python docling's output, which docling.rs is conformance-tested against, so
the accuracy columns favour docling.rs by construction; the timings and
memory do not depend on it.

MinerU runs one row per tier: `flash` (native text, no models — its only
tier for Office / HTML / EPUB) and `basic` (its ONNX layout, table and OCR
models, CPU) by default; `--tiers flash,basic,standard` adds the VLM tier.
docling.rs runs its default full pipeline (layout, TableFormer, OCR) and,
for PDFs, `--text-layer-only` as the like-for-like row for `flash`.
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
FORMATS = ["pdf", "docx", "pptx", "xlsx", "html", "epub"]
FLASH_ONLY = {"docx", "pptx", "xlsx", "html", "epub"}

_MEASURE = r"""
import json, resource, subprocess, sys, time
t = time.perf_counter()
p = subprocess.run(sys.argv[1:])
r = resource.getrusage(resource.RUSAGE_CHILDREN)
print(json.dumps({"seconds": time.perf_counter() - t, "maxrss_mb": r.ru_maxrss // 1024, "returncode": p.returncode}), file=sys.stderr)
"""

# MinerU in-process: a warm-up conversion, then every file timed; the
# Markdown next to a JSON report. Guarded for the spawn start method of
# MinerU's render workers.
_MINERU_WORKER = r"""
import glob, json, os, sys, time


def servers_mb():
    total = 0
    for status in glob.glob("/proc/[0-9]*/status"):
        try:
            with open(status) as fh:
                lines = fh.read().splitlines()
        except OSError:
            continue
        name = next((l.split(None, 1)[1] for l in lines if l.startswith("Name:")), "")
        if "llama" not in name:
            continue
        total += next((int(l.split()[1]) // 1024 for l in lines if l.startswith("VmHWM:")), 0)
    return total


def main():
    files, out, tier = json.loads(sys.argv[1]), sys.argv[2], sys.argv[3]
    from mineru.parser import parse
    t = time.perf_counter()
    try:
        parse(files[0], tier=tier, ocr_mode="auto")
    except Exception:
        pass
    load = time.perf_counter() - t
    times = {}
    for f in files:
        t = time.perf_counter()
        try:
            text = parse(f, tier=tier, ocr_mode="auto").markdown()
        except Exception as e:
            text = f"ERROR: {type(e).__name__}: {e}"
        times[f] = time.perf_counter() - t
        with open(os.path.join(out, os.path.splitext(os.path.basename(f))[0] + ".md"), "w", encoding="utf-8") as fh:
            fh.write(text)
    json.dump({"load": load, "times": times, "servers_mb": servers_mb()}, open(os.path.join(out, "_report.json"), "w"))


if __name__ == "__main__":
    main()
"""


# ------------------------------------------------------------------ metrics

def strip_md(text):
    text = re.sub(r"<!--.*?-->", " ", text, flags=re.S)
    text = re.sub(r"!\[[^\]]*\]\([^)]*\)", " ", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"^\s*\|?\s*:?-{3,}.*$", " ", text, flags=re.M)
    text = re.sub(r"<[^>]+>", " ", text)
    return re.sub(r"[#*_`|>\\]", " ", text)


def words(text):
    return collections.Counter(re.findall(r"\w+", strip_md(text).lower()))


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
    for cell in re.findall(r"<t[dh][^>]*>(.*?)</t[dh]>", text, flags=re.S):
        c = " ".join(re.findall(r"\w+", re.sub(r"<[^>]+>", " ", cell).lower()))
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

def measured(cmd, cwd=ROOT):
    p = subprocess.run([sys.executable, "-c", _MEASURE, *cmd], cwd=cwd, capture_output=True, text=True)
    line = p.stderr.strip().splitlines()[-1] if p.stderr.strip() else "{}"
    try:
        return json.loads(line)
    except json.JSONDecodeError:
        return {"seconds": None, "maxrss_mb": None, "returncode": p.returncode, "stderr": p.stderr[-2000:]}


def mineru_warm(files, tier):
    """One in-process MinerU run over `files`; a dead worker (an OOM kill
    leaves no traceback) is rerun one worker per file."""
    with tempfile.TemporaryDirectory() as out:
        script = os.path.join(out, "_worker.py")
        open(script, "w").write(_MINERU_WORKER)
        m = measured([sys.executable, script, json.dumps(files), out, tier])
        rss = {"client_mb": m.get("maxrss_mb"), "servers_mb": None}
        report_path = os.path.join(out, "_report.json")
        if os.path.exists(report_path):
            report = json.load(open(report_path))
            times, load = report["times"], report["load"]
            rss["servers_mb"] = report.get("servers_mb") or None
        else:
            times, load = {}, None
            for f in files:
                one = os.path.join(out, "one")
                os.makedirs(one, exist_ok=True)
                mf = measured([sys.executable, script, json.dumps([f]), one, tier])
                rss["client_mb"] = max(rss["client_mb"] or 0, mf.get("maxrss_mb") or 0)
                r = os.path.join(one, "_report.json")
                stem = os.path.splitext(os.path.basename(f))[0] + ".md"
                if os.path.exists(r):
                    rep = json.load(open(r))
                    times[f] = rep["times"][f]
                    load = load or rep["load"]
                    rss["servers_mb"] = max(rss["servers_mb"] or 0, rep.get("servers_mb") or 0) or None
                    os.replace(os.path.join(one, stem), os.path.join(out, stem))
                    os.remove(r)
                else:
                    with open(os.path.join(out, stem), "w", encoding="utf-8") as fh:
                        fh.write(f"ERROR: mineru worker died (exit {mf.get('returncode')}, peak RSS {mf.get('maxrss_mb')} MB)")
        texts = {}
        for f in files:
            path = os.path.join(out, os.path.splitext(os.path.basename(f))[0] + ".md")
            texts[f] = open(path, encoding="utf-8").read() if os.path.exists(path) else "ERROR: no output"
        return sum(times.values()), rss, load, texts


def docling_batch(files, extra, jobs=1):
    with tempfile.TemporaryDirectory() as tmp:
        src, dst = os.path.join(tmp, "src"), os.path.join(tmp, "out")
        os.makedirs(src)
        for f in files:
            os.symlink(f, os.path.join(src, os.path.basename(f)))
        m = measured([CLI, *extra, "--jobs", str(jobs), "--input", f"{src}/*", "--output", dst, "--to", "md"])
        out = {}
        for f in files:
            stem = os.path.splitext(os.path.basename(f))[0]
            hits = glob.glob(f"{dst}/**/{glob.escape(stem)}.md", recursive=True)
            out[f] = open(hits[0], encoding="utf-8").read() if hits else "ERROR: no output"
        return m.get("seconds"), m.get("maxrss_mb"), out


def cold_ms(cmd, f, tier=None, runs=3):
    times = []
    for _ in range(runs):
        with tempfile.TemporaryDirectory() as out:
            args = [*cmd, f] + (["-o", os.path.join(out, "x.md"), "--tier", tier] if tier else [])
            m = measured(args)
        times.append(m.get("seconds") or 0.0)
    return round(1000 * statistics.median(times), 1)


# ------------------------------------------------------------------ corpus

def groundtruth(src):
    d = os.path.join(os.path.dirname(os.path.dirname(src)), "groundtruth")
    full = os.path.join(d, os.path.basename(src) + ".md")
    return full if os.path.exists(full) else os.path.join(d, os.path.splitext(os.path.basename(src))[0] + ".md")


def score(files, texts):
    gts = {f: open(groundtruth(f), encoding="utf-8").read() for f in files}
    return {
        "files": len(files),
        "errors": sum(texts[f].startswith("ERROR") for f in files),
        "word_f1": mean(f1(words(gts[f]), words(texts[f])) for f in files),
        "table_cell_recall": mean(recall(table_cells(gts[f]), table_cells(texts[f])) for f in files),
        "heading_recall": mean(recall(headings(gts[f]), headings(texts[f])) for f in files),
    }


def corpus(formats, tiers, keep=None):
    report = {}
    for fmt in formats:
        files = sorted(
            f for f in glob.glob(f"{ROOT}/tests/data/{fmt}/sources/*")
            if os.path.isfile(f) and os.path.exists(groundtruth(f))
        )
        if not files:
            continue
        row = {}
        for tier in (["flash"] if fmt in FLASH_ONLY else tiers):
            secs, rss, load, texts = mineru_warm(files, tier)
            row[f"mineru {tier}"] = {"seconds": None if secs is None else round(secs, 3), "maxrss": rss,
                                     "model_load_seconds": None if load is None else round(load, 2), **score(files, texts)}
            if keep:
                for f, t in texts.items():
                    open(os.path.join(keep, f"{fmt}.mineru-{tier}.{os.path.basename(f)}.md"), "w", encoding="utf-8").write(t)
        secs, rss, texts = docling_batch(files, [])
        row["docling.rs"] = {"seconds": round(secs, 3), "maxrss": {"client_mb": rss}, **score(files, texts)}
        if keep:
            for f, t in texts.items():
                open(os.path.join(keep, f"{fmt}.docling-rs.{os.path.basename(f)}.md"), "w", encoding="utf-8").write(t)
        if fmt == "pdf":
            secs, rss, texts = docling_batch(files, ["--text-layer-only"])
            row["docling.rs --text-layer-only"] = {"seconds": round(secs, 3), "maxrss": {"client_mb": rss}, **score(files, texts)}
        sample = sorted(files, key=os.path.getsize)[len(files) // 2]
        kit = shutil.which("mineru-kit") or os.path.join(os.path.dirname(sys.executable), "mineru-kit")
        cold = {"file": os.path.basename(sample), "docling.rs": cold_ms([CLI], sample)}
        for tier in (["flash"] if fmt in FLASH_ONLY else tiers):
            cold[f"mineru-kit parse --tier {tier}"] = cold_ms([kit, "parse"], sample, tier=tier)
        row["cold_cli_ms"] = cold
        report[fmt] = row
        print(fmt, json.dumps(row), file=sys.stderr, flush=True)
    return report


def main():
    args = sys.argv[1:]
    if not args or args[0] != "corpus":
        sys.exit(__doc__)
    tiers = ["flash", "basic"]
    keep = None
    rest = []
    it = iter(args[1:])
    for a in it:
        if a == "--tiers":
            tiers = [t.strip() for t in next(it).split(",") if t.strip()]
        elif a == "--keep":
            keep = next(it)
            os.makedirs(keep, exist_ok=True)
        else:
            rest.append(a)
    print(json.dumps(corpus(rest or FORMATS, tiers, keep), indent=1))


if __name__ == "__main__":
    main()
