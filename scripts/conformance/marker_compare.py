#!/usr/bin/env python3
"""docling.rs vs datalab's Marker — the numbers in README's
"Conformance with Marker".

Run from the repo root with a release CLI (`cargo build --release -p
docling-cli`), the models (`scripts/install/download_dependencies.sh`) and
Marker installed in the running Python (`pip install marker-pdf`; the Office
and HTML rows need `pip install "marker-pdf[full]"`, which brings WeasyPrint):

    python3 scripts/conformance/marker_compare.py corpus [FORMAT ...]

`corpus` converts this repo's test corpus (tests/data/<fmt>/sources with a
docling groundtruth) with both tools: warm time (Marker in-process after a
warm-up conversion, models loaded once; docling.rs batch mode, `--jobs 1`),
peak RSS of the conversion process tree, cold CLI time on the median-size
file (`marker_single` vs `docling-rs`), and accuracy against docling's
groundtruth Markdown — word F1, table-cell recall, heading recall. The
reference is Python docling's output, which docling.rs is conformance-tested
against, so the accuracy columns favour docling.rs by construction; the
timings and memory do not depend on it.

Marker runs with its defaults on the host it is run on — on a CPU host that
is its `fast` mode (lightweight layout detector, OCR only on garbled blocks);
`--marker-config '{"mode": "balanced"}'` or any other JSON config overrides
them. docling.rs runs its default full pipeline (layout, TableFormer, OCR)
and, for PDFs, `--text-layer-only` as the like-for-like row for Marker's
`--disable_ocr`.
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

# Runs a command and reports its wall time and the peak RSS of its process
# tree (ru_maxrss of the waited-for children, which includes the inference
# workers Marker starts), as JSON on the last stderr line.
_MEASURE = r"""
import json, resource, subprocess, sys, time
t = time.perf_counter()
p = subprocess.run(sys.argv[1:])
r = resource.getrusage(resource.RUSAGE_CHILDREN)
print(json.dumps({"seconds": time.perf_counter() - t, "maxrss_mb": r.ru_maxrss // 1024, "returncode": p.returncode}), file=sys.stderr)
"""

# Marker in-process: models once, a warm-up conversion, then every file
# timed; the Markdown is written next to a JSON report.
_MARKER_WORKER = r"""
import json, os, sys, time
files, out, config = json.loads(sys.argv[1]), sys.argv[2], json.loads(sys.argv[3])
from marker.converters.pdf import PdfConverter
from marker.models import create_model_dict
from marker.output import text_from_rendered
t = time.perf_counter()
converter = PdfConverter(artifact_dict=create_model_dict(), config=config)
load = time.perf_counter() - t
try:
    converter(files[0])
except Exception:
    pass
times = {}
for f in files:
    t = time.perf_counter()
    try:
        text = text_from_rendered(converter(f))[0]
    except Exception as e:
        text = f"ERROR: {type(e).__name__}: {e}"
    times[f] = time.perf_counter() - t
    with open(os.path.join(out, os.path.splitext(os.path.basename(f))[0] + ".md"), "w", encoding="utf-8") as fh:
        fh.write(text)
json.dump({"load": load, "times": times}, open(os.path.join(out, "_report.json"), "w"))
"""


# ------------------------------------------------------------------ metrics

def strip_md(text):
    text = re.sub(r"<!--.*?-->", " ", text, flags=re.S)
    text = re.sub(r"!\[[^\]]*\]\([^)]*\)", " ", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"^\s*\|?\s*:?-{3,}.*$", " ", text, flags=re.M)
    text = re.sub(r"<[^>]+>", " ", text)  # Marker's <sup>, <br>, <span>
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
    # Marker keeps a complex table as HTML inside the Markdown
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

SURYA = os.path.expanduser("~/.cache/datalab/surya")


def marker_servers():
    """Marker 2's inference servers: Surya spawns them once and leaves them
    running (`keep_alive`), registered in ~/.cache/datalab/surya/*_server.json."""
    out = []
    for path in glob.glob(os.path.join(SURYA, "*_server.json")):
        try:
            out.append((path, json.load(open(path))))
        except (OSError, ValueError):
            continue
    return out


def kill_marker_servers():
    """A true cold start: no resident inference server."""
    for path, info in marker_servers():
        try:
            os.kill(int(info["pid"]), 15)
        except (OSError, ValueError, KeyError):
            pass
        for suffix in (".json", ".lock"):
            try:
                os.remove(path[: -len(".json")] + suffix)
            except OSError:
                pass
    time.sleep(1)


def marker_server_peak_mb():
    """Peak RSS (VmHWM) of the resident inference servers, summed — they
    are not children of the measured process, so ru_maxrss misses them."""
    total = 0
    for _, info in marker_servers():
        try:
            with open(f"/proc/{int(info['pid'])}/status") as fh:
                for line in fh:
                    if line.startswith("VmHWM:"):
                        total += int(line.split()[1]) // 1024
        except (OSError, ValueError, KeyError):
            continue
    return total or None


def measured(cmd, cwd=ROOT):
    """(seconds, peak RSS MB, returncode) of `cmd` run through the wrapper."""
    p = subprocess.run([sys.executable, "-c", _MEASURE, *cmd], cwd=cwd, capture_output=True, text=True)
    line = p.stderr.strip().splitlines()[-1] if p.stderr.strip() else "{}"
    try:
        return json.loads(line)
    except json.JSONDecodeError:
        return {"seconds": None, "maxrss_mb": None, "returncode": p.returncode, "stderr": p.stderr[-2000:]}


def marker_warm(files, config):
    """One in-process Marker run over `files`: (seconds summed over the
    files, peak RSS MB, model-load seconds, {file: markdown})."""
    kill_marker_servers()
    with tempfile.TemporaryDirectory() as out:
        m = measured([sys.executable, "-c", _MARKER_WORKER, json.dumps(files), out, json.dumps(config)])
        rss = {"client_mb": m.get("maxrss_mb"), "servers_mb": marker_server_peak_mb()}
        report_path = os.path.join(out, "_report.json")
        if not os.path.exists(report_path):
            return None, rss, None, {f: "ERROR: marker did not finish: " + m.get("stderr", "") for f in files}
        report = json.load(open(report_path))
        texts = {}
        for f in files:
            path = os.path.join(out, os.path.splitext(os.path.basename(f))[0] + ".md")
            texts[f] = open(path, encoding="utf-8").read() if os.path.exists(path) else "ERROR: no output"
        return sum(report["times"].values()), rss, report["load"], texts


def docling_batch(files, extra, jobs=1):
    """One batch run over `files`; (seconds, peak RSS MB, {file: markdown})."""
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


def cold_ms(cmd, f, runs=3):
    """Median wall time of `cmd FILE` over `runs` fresh processes — for
    `marker_single` with its inference server killed first, so every run is
    the first run on a machine."""
    times = []
    marker = cmd[0].endswith("marker_single")
    for _ in range(runs):
        if marker:
            kill_marker_servers()
        with tempfile.TemporaryDirectory() as out:
            m = measured([*cmd, f] + (["--output_dir", out] if marker else []))
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


def corpus(formats, marker_config, keep=None):
    report = {}
    for fmt in formats:
        files = sorted(
            f for f in glob.glob(f"{ROOT}/tests/data/{fmt}/sources/*")
            if os.path.isfile(f) and os.path.exists(groundtruth(f))
        )
        if not files:
            continue
        row = {}
        secs, rss, load, texts = marker_warm(files, marker_config)
        row["marker"] = {"seconds": None if secs is None else round(secs, 3), "maxrss": rss, "model_load_seconds": None if load is None else round(load, 2), **score(files, texts)}
        if keep:
            for f, t in texts.items():
                open(os.path.join(keep, f"{fmt}.marker.{os.path.basename(f)}.md"), "w", encoding="utf-8").write(t)
        secs, rss, texts = docling_batch(files, [])
        row["docling.rs"] = {"seconds": round(secs, 3), "maxrss": {"client_mb": rss}, **score(files, texts)}
        if keep:
            for f, t in texts.items():
                open(os.path.join(keep, f"{fmt}.docling-rs.{os.path.basename(f)}.md"), "w", encoding="utf-8").write(t)
        if fmt == "pdf":
            secs, rss, texts = docling_batch(files, ["--text-layer-only"])
            row["docling.rs --text-layer-only"] = {"seconds": round(secs, 3), "maxrss": {"client_mb": rss}, **score(files, texts)}
        sample = sorted(files, key=os.path.getsize)[len(files) // 2]
        marker_cli = shutil.which("marker_single") or os.path.join(os.path.dirname(sys.executable), "marker_single")
        row["cold_cli_ms"] = {
            "file": os.path.basename(sample),
            "marker_single": cold_ms([marker_cli], sample),
            "docling.rs": cold_ms([CLI], sample),
        }
        report[fmt] = row
        print(fmt, json.dumps(row), file=sys.stderr, flush=True)
    return report


def main():
    args = sys.argv[1:]
    if not args or args[0] != "corpus":
        sys.exit(__doc__)
    config = {}
    keep = None
    rest = []
    it = iter(args[1:])
    for a in it:
        if a == "--marker-config":
            config = json.loads(next(it))
        elif a == "--keep":
            keep = next(it)
            os.makedirs(keep, exist_ok=True)
        else:
            rest.append(a)
    print(json.dumps(corpus(rest or FORMATS, config, keep), indent=1))


if __name__ == "__main__":
    main()
