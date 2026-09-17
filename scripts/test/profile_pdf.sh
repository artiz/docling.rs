#!/usr/bin/env bash
#
# Per-stage profiler for the PDF/image ML pipeline. Runs the release
# `docling-rs` binary over every source in `tests/data/pdf/sources` (or a
# caller-supplied set) with `DOCLING_RS_TIMING=1`, which makes the pipeline
# print a wall-clock breakdown per named stage (see
# `crates/docling-pdf/src/timing.rs`). The per-file breakdowns are summed into
# one corpus-wide table, so the stage that actually dominates PDF conversion is
# obvious at a glance — the analog of `examples/profile_declarative` for the ML
# path, which no cargo example can cover cheaply (each links onnxruntime).
#
# Usage:
#   scripts/test/profile_pdf.sh [file-or-dir ...]        # defaults to the corpus
#   DOCLING_RS_FP32=1 scripts/test/profile_pdf.sh        # full-precision models
#   PROFILE_TO=json scripts/test/profile_pdf.sh          # export format (md default)
#
# Honours the same model/pdfium env wiring as performance.sh; set the DOCLING_*
# ONNX/pdfium paths yourself to point at a non-default model set.
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../_common.sh"

TO="${PROFILE_TO:-md}"

# Point the pipeline at the fetched libs/models with absolute paths (mirrors
# performance.sh) so it runs the full stack regardless of CWD. Default stack is
# INT8 layout + KV-cached TableFormer decoder; DOCLING_RS_FP32=1 selects fp32.
[[ -e "$WORKSPACE_DIR/.pdfium/lib/libpdfium.so" ]] && export PDFIUM_DYNAMIC_LIB_PATH="${PDFIUM_DYNAMIC_LIB_PATH:-$WORKSPACE_DIR/.pdfium/lib}"
if [[ "${DOCLING_RS_FP32:-0}" != "1" && -e "$WORKSPACE_DIR/.models/layout_heron_int8.onnx" ]]; then
  export DOCLING_LAYOUT_ONNX="${DOCLING_LAYOUT_ONNX:-$WORKSPACE_DIR/.models/layout_heron_int8.onnx}"
  [[ -e "$WORKSPACE_DIR/.models/tableformer/decoder_int8.onnx" ]] && export DOCLING_TABLEFORMER_DECODER="${DOCLING_TABLEFORMER_DECODER:-$WORKSPACE_DIR/.models/tableformer/decoder_int8.onnx}"
fi
[[ -e "$WORKSPACE_DIR/.models/layout_heron.onnx" ]] && export DOCLING_LAYOUT_ONNX="${DOCLING_LAYOUT_ONNX:-$WORKSPACE_DIR/.models/layout_heron.onnx}"
[[ -e "$WORKSPACE_DIR/.models/ocr_rec.onnx" ]] && export DOCLING_OCR_REC_ONNX="${DOCLING_OCR_REC_ONNX:-$WORKSPACE_DIR/.models/ocr_rec.onnx}"
[[ -e "$WORKSPACE_DIR/.models/ocr_det.onnx" ]] && export DOCLING_OCR_DET_ONNX="${DOCLING_OCR_DET_ONNX:-$WORKSPACE_DIR/.models/ocr_det.onnx}"
[[ -e "$WORKSPACE_DIR/.models/ppocr_keys_v1.txt" ]] && export DOCLING_OCR_DICT="${DOCLING_OCR_DICT:-$WORKSPACE_DIR/.models/ppocr_keys_v1.txt}"
[[ -e "$WORKSPACE_DIR/.models/tableformer/encoder.onnx" ]] && export DOCLING_TABLEFORMER_ENCODER="${DOCLING_TABLEFORMER_ENCODER:-$WORKSPACE_DIR/.models/tableformer/encoder.onnx}"
[[ -e "$WORKSPACE_DIR/.models/tableformer/decoder.onnx" ]] && export DOCLING_TABLEFORMER_DECODER="${DOCLING_TABLEFORMER_DECODER:-$WORKSPACE_DIR/.models/tableformer/decoder.onnx}"
[[ -e "$WORKSPACE_DIR/.models/tableformer/bbox.onnx" ]] && export DOCLING_TABLEFORMER_BBOX="${DOCLING_TABLEFORMER_BBOX:-$WORKSPACE_DIR/.models/tableformer/bbox.onnx}"

echo ">> building Rust release binary ..."
RUST_BIN="$(build_rust_release)"

# Collect the input files: caller args (files or dirs) or the PDF corpus.
inputs=()
if [[ $# -gt 0 ]]; then
  for a in "$@"; do
    if [[ -d "$a" ]]; then
      while IFS= read -r f; do inputs+=("$f"); done \
        < <(find "$a" -maxdepth 1 -type f \( -iname '*.pdf' -o -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.tif' -o -iname '*.tiff' \) | sort)
    else
      inputs+=("$a")
    fi
  done
else
  while IFS= read -r f; do inputs+=("$f"); done \
    < <(find "$WORKSPACE_DIR/tests/data/pdf/sources" -maxdepth 1 -type f -iname '*.pdf' | sort)
fi

if [[ ${#inputs[@]} -eq 0 ]]; then
  echo "no input files" >&2
  exit 2
fi

echo ">> profiling ${#inputs[@]} file(s), export=$TO, fp32=${DOCLING_RS_FP32:-0}"

# Accumulate `stage -> (total_ms, calls)` across all files, plus a per-file
# wall-clock line, by parsing each run's timing report off stderr. The report
# lines look like `  layout.predict   1234.5 ms  42.0%  (12 calls)`.
AGG="$(mktemp)"
PERFILE="$(mktemp)"
trap 'rm -f "$AGG" "$PERFILE"' EXIT

for f in "${inputs[@]}"; do
  base="$(basename "$f")"
  err="$(mktemp)"
  start="$(date +%s.%N)"
  DOCLING_RS_TIMING=1 "$RUST_BIN" --to "$TO" "$f" >/dev/null 2>"$err" || {
    echo "  !! convert failed: $base" >&2
  }
  end="$(date +%s.%N)"
  awk -v OFS='\t' '
    /ms +[0-9.]+% +\([0-9]+ calls\)/ {
      # fields: stage ms "ms" pct% (n calls)
      stage=$1; ms=$2; calls=$(NF-1); gsub(/[()]/,"",calls)
      print stage, ms, calls
    }' "$err" >>"$AGG"
  awk -v b="$base" -v s="$start" -v e="$end" 'BEGIN { printf "%s\t%.3f\n", b, (e-s)*1000 }' >>"$PERFILE"
  rm -f "$err"
done

echo
echo "=== per-file wall-clock (end-to-end, incl. model load) ==="
sort -t$'\t' -k2 -nr "$PERFILE" | awk -F'\t' '{ printf "  %-40s %9.1f ms\n", $1, $2 }'

echo
echo "=== per-stage totals across all files (descending) ==="
awk -F'\t' '
  { ms[$1]+=$2; calls[$1]+=$3; total+=$2 }
  END {
    for (s in ms) printf "%s\t%.1f\t%d\n", s, ms[s], calls[s]
    printf "__TOTAL__\t%.1f\t0\n", total
  }' "$AGG" | sort -t$'\t' -k2 -nr | awk -F'\t' -v tot="$(awk -F'\t' '{t+=$2} END{print t}' "$AGG")" '
  $1=="__TOTAL__" { printf "  %-24s %10.1f ms\n", "TOTAL (summed stages)", $2; next }
  { printf "  %-24s %10.1f ms  %5.1f%%  (%d calls)\n", $1, $2, (tot>0?$2/tot*100:0), $3 }'
