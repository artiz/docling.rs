#!/usr/bin/env python3
"""Summarize an ONNX Runtime per-operator profile written under
DOCLING_RS_ORT_PROFILE (docling-onnx asks ORT for one per session).

    DOCLING_RS_ORT_PROFILE=/tmp/prof docling-rs paper.pdf --output out
    python3 scripts/dev/ort_profile_summary.py /tmp/prof/decoder_kv_*.json

Prints, per file: the run count and total `model_run` time, the kernel time
by operator type (share of the kernel total, calls, mean per call), the
heaviest individual nodes with their output shapes, and the run-duration
percentiles — enough to tell weight-streaming GEMMs from shape plumbing from
executor overhead (the difference between `model_run` and the kernel sum).
Profiling itself inflates small nodes (tens of µs each), so compare shares,
not absolute times, against an unprofiled `DOCLING_RS_TIMING=1` run.
"""
import collections
import json
import sys


def summarize(path, top=12):
    events = json.load(open(path, encoding="utf-8"))
    runs = [e for e in events if e.get("cat") == "Session" and e.get("name") == "model_run"]
    nodes = [e for e in events if e.get("cat") == "Node" and e.get("name", "").endswith("_kernel_time")]
    if not runs:
        print(f"{path}: no model_run events")
        return
    run_total = sum(r["dur"] for r in runs) / 1000
    kernel_total = sum(n["dur"] for n in nodes) / 1000
    print(f"== {path}")
    print(f"runs {len(runs)}  model_run {run_total:.1f} ms  kernels {kernel_total:.1f} ms "
          f"({100 * kernel_total / max(run_total, 1e-9):.0f} %)  nodes/run {len(nodes) / len(runs):.0f}")
    by_op = collections.Counter()
    calls = collections.Counter()
    for n in nodes:
        op = n["args"].get("op_name", "?")
        by_op[op] += n["dur"]
        calls[op] += 1
    print("  by operator:")
    for op, dur in by_op.most_common(top):
        print(f"    {op:24s} {dur / 1000:9.1f} ms {100 * dur / max(kernel_total * 1000, 1):5.1f} % "
              f"{calls[op]:8d} calls {dur / calls[op]:7.1f} µs/call")
    by_node = collections.Counter()
    shape = {}
    for n in nodes:
        by_node[n["name"]] += n["dur"]
        shape[n["name"]] = (n["args"].get("op_name", "?"), n["args"].get("output_type_shape"))
    print("  heaviest nodes:")
    for name, dur in by_node.most_common(top):
        op, out = shape[name]
        print(f"    {op:12s} {name[:48]:48s} {dur / 1000:8.1f} ms  out={json.dumps(out)[:60]}")
    durs = sorted(r["dur"] / 1000 for r in runs)
    pct = lambda p: durs[min(len(durs) - 1, int(p * len(durs)))]
    print(f"  run ms: min {durs[0]:.2f}  p25 {pct(0.25):.2f}  median {pct(0.5):.2f}  "
          f"p75 {pct(0.75):.2f}  max {durs[-1]:.2f}")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for p in sys.argv[1:]:
        summarize(p)
