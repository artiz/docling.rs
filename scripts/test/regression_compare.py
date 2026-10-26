#!/usr/bin/env python3
"""Regression check: this checkout's release CLI against an earlier build.

    python3 scripts/test/regression_compare.py [--base REF] [--formats md,json]
                                                [--runs N] [--threshold PCT]

Builds `--base` (default: the newest `vX.Y.Z` tag) in a throwaway worktree
with its own target dir, builds HEAD, then:

1. **Output**: converts every corpus source (`tests/data/*/sources`,
   `crates/docling/tests/data/*/sources`; PDFs only the README's fixture,
   scans/audio skipped) with both CLIs in each `--formats` and lists the
   files whose bytes differ — an intended change shows up here as much as a
   regression, so read the list.
2. **Cost**: the README's performance fixtures, `--runs` runs each: mean
   wall time and peak RSS (wait4's rusage), plus callgrind's instruction
   count when valgrind is installed — the number that does not move with
   machine noise. A fixture more than `--threshold` % (default 10) worse on
   instructions (or on wall time without valgrind) is flagged.

Exit status 1 when anything is flagged or differs, so a scheduled session
can tell "nothing to report" from "look at this". The models resolve through
`DOCLING_RS_MODELS_DIR` (the repo's `.models/`), as `performance.sh` does.
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = [
    "tests/data/csv/sources/csv-comma.csv",
    "tests/data/md/sources/wiki.md",
    "tests/data/pptx/sources/powerpoint_with_image.pptx",
    "tests/data/xlsx/sources/xlsx_04_inflated.xlsx",
    "tests/data/jats/sources/elife-56337.nxml",
    "tests/data/html/sources/wiki_duck.html",
    "tests/data/docx/sources/docx_rich_tables_01.docx",
    "tests/data/pdf/sources/picture_classification.pdf",
]
SKIP_DIRS = ("/pdf/", "/scanned/", "/audio/", "/tiff/", "/heif/", "/webp/", "pdf_password")


def sh(*args, **kw):
    return subprocess.run(args, check=True, **kw)


def build(cwd: Path, target: Path) -> Path:
    env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_INCREMENTAL="0")
    sh("cargo", "build", "--release", "--quiet", "-p", "docling-cli", cwd=cwd, env=env)
    return target / "release" / "docling-rs"


def newest_tag() -> str:
    out = sh("git", "tag", "--list", "v*", "--sort=-v:refname", cwd=ROOT,
             capture_output=True, text=True).stdout.split()
    return out[0]


GNU_TIME = "/usr/bin/time" if os.access("/usr/bin/time", os.X_OK) else None


def run(binary: Path, args, env):
    """Output, exit code, wall seconds — and the peak RSS in MB when GNU time
    is there (else None): a child's `ru_maxrss` counts the image it was
    forked from, so read from this script, whose own heap grows through the
    corpus pass, it would report the script's size, not the CLI's."""
    cmd = [str(binary), *args]
    with tempfile.NamedTemporaryFile(mode="r") as rss:
        if GNU_TIME:
            cmd = [GNU_TIME, "-f", "%M", "-o", rss.name, *cmd]
        t = time.perf_counter()
        p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
        wall = time.perf_counter() - t
        peak = None
        if GNU_TIME:
            lines = rss.read().split()
            peak = int(lines[-1]) / 1024 if lines and lines[-1].isdigit() else None
    return p.stdout, p.returncode, wall, peak


def instructions(binary: Path, src: str, env):
    if not shutil.which("valgrind"):
        return None
    with tempfile.NamedTemporaryFile(suffix=".cg") as cg:
        r = subprocess.run(["valgrind", "--tool=callgrind", f"--callgrind-out-file={cg.name}",
                            str(binary), src], capture_output=True, text=True, env=env)
        for line in r.stderr.splitlines():
            if "Collected :" in line:
                return int(line.split(":")[-1].strip())
    return None


def corpus():
    files = []
    for base in ("tests/data", "crates/docling/tests/data"):
        for p in sorted((ROOT / base).glob("*/sources/**/*")):
            s = str(p)
            if p.is_file() and not any(d in s for d in SKIP_DIRS):
                files.append(s)
    return files + [str(ROOT / FIXTURES[-1])]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base")
    ap.add_argument("--formats", default="md,json")
    ap.add_argument("--runs", type=int, default=10)
    ap.add_argument("--threshold", type=float, default=10.0)
    ap.add_argument("--keep", action="store_true", help="keep the base worktree")
    a = ap.parse_args()
    base = a.base or newest_tag()
    env = dict(os.environ, DOCLING_RS_MODELS_DIR=str(ROOT / ".models"))

    work = Path(tempfile.mkdtemp(prefix="docling-regress-"))
    tree = work / "base"
    sh("git", "worktree", "add", "--quiet", "--detach", str(tree), base, cwd=ROOT)
    try:
        print(f">> building {base} and HEAD", flush=True)
        old = build(tree, work / "target")
        new = build(ROOT, ROOT / "target")
        flagged = 0

        for fmt in a.formats.split(","):
            diffs = []
            files = corpus()
            for src in files:
                o = run(old, ["--to", fmt, src], env)
                n = run(new, ["--to", fmt, src], env)
                if o[0] != n[0] or o[1] != n[1]:
                    diffs.append(os.path.relpath(src, ROOT))
            print(f"\n== output --to {fmt}: {len(diffs)} of {len(files)} differ from {base}")
            for d in diffs:
                print(f"   {d}")
            flagged += bool(diffs)

        print(f"\n== cost vs {base} ({a.runs} runs; instructions from callgrind)")
        print(f"{'fixture':32} {'wall old→new':>20} {'peak RSS old→new':>22} {'instr Δ':>9}")
        for f in FIXTURES:
            src = str(ROOT / f)
            stats = {}
            for v, b in (("old", old), ("new", new)):
                run(b, [src], env)  # warm-up
                rs = [run(b, [src], env) for _ in range(a.runs)]
                peaks = [r[3] for r in rs if r[3] is not None]
                stats[v] = (sum(r[2] for r in rs) / a.runs, max(peaks) if peaks else float("nan"),
                            instructions(b, src, env) if not f.endswith(".pdf") else None)
            (wo, mo, io), (wn, mn, inn) = stats["old"], stats["new"]
            di = (inn - io) / io * 100 if io and inn else None
            dw = (wn - wo) / wo * 100
            bad = (di if di is not None else dw) > a.threshold
            flagged += bad
            dis = f"{di:+.1f}%" if di is not None else "—"
            print(f"{Path(f).name:32} {wo*1000:8.1f}→{wn*1000:7.1f} ms {mo:9.1f}→{mn:7.1f} MB"
                  f" {dis:>9}{'  ⚠' if bad else ''}")
        sys.exit(1 if flagged else 0)
    finally:
        if not a.keep:
            subprocess.run(["git", "worktree", "remove", "--force", str(tree)], cwd=ROOT)
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
