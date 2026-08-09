#!/usr/bin/env python3
"""Compare docling.rs against an Amazon Bedrock model on the PDF corpus.

Driven by bedrock_conformance.sh (which validates env, pins the conformance
models and builds the release CLI). For every groundtruth-scored corpus PDF:

- docling.rs converts the whole set in ONE warm batch run (the #205 CLI batch
  mode); per-file wall seconds are parsed from its ``ok: … (X.Xs`` stderr
  lines, so the model load is paid once, exactly like a real batch user.
- Bedrock gets one Converse call per PDF: a ``document`` content block with
  the raw bytes plus an extraction prompt; the call is timed around the API
  round-trip and the reply's text blocks are the extracted Markdown.

Both outputs score against the same committed groundtruth with a normalized
line-similarity percentage (whitespace runs collapsed, empty lines dropped,
``difflib.SequenceMatcher`` over the line lists) — the byte-diff metric of
pdf_groundtruth.sh is meaningless for an LLM, so both sides get the same
fuzzy metric; docling.rs sits near 100 % on it.
"""

import difflib
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import boto3

ROOT = Path(__file__).resolve().parent.parent.parent
SOURCES = ROOT / "tests/data/pdf/sources"
GROUNDTRUTH = ROOT / "tests/data/pdf/groundtruth"
CLI = ROOT / "target/release/docling-rs"

MODEL_ID = os.environ.get("AWS_BEDROCK_MODEL_ID", "eu.amazon.nova-lite-v1:0")
MAX_TOKENS = int(os.environ.get("AWS_BEDROCK_MAX_TOKENS", "5000"))
PROMPT = os.environ.get(
    "AWS_BEDROCK_PROMPT",
    "Extract all the possible info from this PDF as markdown text.",
)


def normalize(text: str) -> list[str]:
    """Whitespace-collapsed non-empty lines — the shared fuzzy-metric input."""
    lines = []
    for raw in text.splitlines():
        line = re.sub(r"\s+", " ", raw).strip()
        if line:
            lines.append(line)
    return lines


def similarity(groundtruth: str, output: str) -> float:
    return 100.0 * difflib.SequenceMatcher(
        None, normalize(groundtruth), normalize(output)
    ).ratio()


def docling_batch(stems: list[str]) -> dict[str, tuple[float, str]]:
    """Convert all corpus PDFs in one warm batch run.

    Returns stem -> (seconds, markdown). Only the groundtruth-scored PDFs are
    linked into the batch directory, so the timing loop matches the scored set.
    """
    with tempfile.TemporaryDirectory(prefix="bedrock-dl-") as tmp:
        indir = Path(tmp) / "in"
        outdir = Path(tmp) / "out"
        indir.mkdir()
        for stem in stems:
            os.symlink(SOURCES / f"{stem}.pdf", indir / f"{stem}.pdf")
        proc = subprocess.run(
            [str(CLI), "--input", f"{indir}/*.pdf", "--output", str(outdir)],
            capture_output=True,
            text=True,
            cwd=ROOT,
        )
        results: dict[str, tuple[float, str]] = {}
        # stderr lines: "ok: <in> -> <out> (6.6s, 254 ms/page)"
        for line in proc.stderr.splitlines():
            m = re.match(r"ok: (.+?) -> .+ \(([0-9.]+)s", line)
            if not m:
                continue
            stem = Path(m.group(1)).stem
            out = outdir / f"{stem}.md"
            if out.exists():
                results[stem] = (float(m.group(2)), out.read_text())
        for line in proc.stderr.splitlines():
            if line.startswith("error:"):
                print(f"  docling {line}", file=sys.stderr)
        return results


def bedrock_client():
    return boto3.client(
        "bedrock-runtime",
        region_name=os.environ["AWS_BEDROCK_REGION"],
        aws_access_key_id=os.environ["AWS_BEDROCK_ACCESS_KEY_ID"],
        aws_secret_access_key=os.environ["AWS_BEDROCK_SECRET_ACCESS_KEY"],
    )


def bedrock_convert(client, pdf: Path) -> tuple[float, str]:
    """One timed Converse call: PDF document block + the extraction prompt."""
    # Document names allow alphanumerics/spaces/hyphens only.
    name = re.sub(r"[^A-Za-z0-9-]+", "-", pdf.stem).strip("-") or "document"
    started = time.monotonic()
    resp = client.converse(
        modelId=MODEL_ID,
        messages=[
            {
                "role": "user",
                "content": [
                    {
                        "document": {
                            "format": "pdf",
                            "name": name,
                            "source": {"bytes": pdf.read_bytes()},
                        }
                    },
                    {"text": PROMPT},
                ],
            }
        ],
        inferenceConfig={"maxTokens": MAX_TOKENS},
    )
    secs = time.monotonic() - started
    parts = resp["output"]["message"]["content"]
    return secs, "".join(p.get("text", "") for p in parts)


def main() -> int:
    stems = sorted(p.stem for p in GROUNDTRUTH.glob("*.md"))
    if not stems:
        print("error: no groundtruth files found", file=sys.stderr)
        return 2

    ep = os.environ.get("DOCLING_RS_EP", "cpu") or "cpu"
    print(f"docling.rs: warm batch over {len(stems)} PDFs (EP: {ep}) …", file=sys.stderr)
    docling = docling_batch(stems)

    print(f"bedrock: {MODEL_ID} in {os.environ['AWS_BEDROCK_REGION']} …", file=sys.stderr)
    client = bedrock_client()
    nova: dict[str, tuple[float, str] | str] = {}
    for stem in stems:
        try:
            nova[stem] = bedrock_convert(client, SOURCES / f"{stem}.pdf")
            print(f"  {stem}: {nova[stem][0]:.1f}s", file=sys.stderr)
        except Exception as e:  # noqa: BLE001 — per-file API errors are data
            nova[stem] = f"{type(e).__name__}: {e}"
            print(f"  {stem}: FAILED ({nova[stem]})", file=sys.stderr)

    print()
    print(f"{'PDF':34} {'docling s':>9} {'docling %':>9} {'nova s':>8} {'nova %':>7}")
    print(f"{'---':34} {'---------':>9} {'---------':>9} {'------':>8} {'------':>7}")
    dl_times, dl_sims, nv_times, nv_sims = [], [], [], []
    for stem in stems:
        gt = (GROUNDTRUTH / f"{stem}.md").read_text()
        if stem in docling:
            secs, md = docling[stem]
            sim = similarity(gt, md)
            dl_times.append(secs)
            dl_sims.append(sim)
            dl_cols = f"{secs:>9.1f} {sim:>8.1f}%"
        else:
            dl_cols = f"{'—':>9} {'—':>9}"
        entry = nova[stem]
        if isinstance(entry, tuple):
            secs, md = entry
            sim = similarity(gt, md)
            nv_times.append(secs)
            nv_sims.append(sim)
            nv_cols = f"{secs:>8.1f} {sim:>6.1f}%"
        else:
            nv_cols = f"{'—':>8} {'—':>7}"
        print(f"{stem:34} {dl_cols} {nv_cols}")

    def mean(xs: list[float]) -> float:
        return sum(xs) / len(xs) if xs else 0.0

    print()
    print(
        f"docling.rs ({ep}): {sum(dl_times):.1f}s total, "
        f"{mean(dl_times):.1f}s/doc, {mean(dl_sims):.1f}% mean similarity "
        f"({len(dl_times)}/{len(stems)} converted)"
    )
    print(
        f"{MODEL_ID}: {sum(nv_times):.1f}s total, "
        f"{mean(nv_times):.1f}s/doc, {mean(nv_sims):.1f}% mean similarity "
        f"({len(nv_times)}/{len(stems)} converted)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
