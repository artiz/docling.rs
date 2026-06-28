#!/usr/bin/env bash
#
# Per-PDF byte-conformance of the Rust pipeline vs the committed docling
# groundtruth (tests/data/pdf/groundtruth/*.md). Unlike conformance.sh this needs
# no docling install — it diffs against the checked-in reference. Use it to track
# how many groundtruth PDFs are byte-for-byte exact (see PDF_CONFORMANCE.md).
#
# Usage: scripts/pdf_groundtruth.sh

set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.."

export PDFIUM_DYNAMIC_LIB_PATH="${PDFIUM_DYNAMIC_LIB_PATH:-$(pwd)/.pdfium/lib}"
export DOCLING_LAYOUT_ONNX="${DOCLING_LAYOUT_ONNX:-$(pwd)/models/layout_heron.onnx}"
export DOCLING_OCR_REC_ONNX="${DOCLING_OCR_REC_ONNX:-$(pwd)/models/ocr_rec.onnx}"
export DOCLING_OCR_DICT="${DOCLING_OCR_DICT:-$(pwd)/models/ppocr_keys_v1.txt}"

cargo build --release --quiet -p fleischwolf-cli
BIN=./target/release/fleischwolf

exact=0
total=0
printf "%-34s %12s\n" "PDF" "DIFF-LINES"
printf "%-34s %12s\n" "---" "----------"
for gt in tests/data/pdf/groundtruth/*.md; do
  stem="$(basename "$gt" .md)"
  src="tests/data/pdf/sources/$stem.pdf"
  [[ -f "$src" ]] || continue
  total=$((total + 1))
  out="$("$BIN" "$src" 2>/dev/null || echo '<ERROR>')"
  # Compare trailing-newline-insensitively; one changed line counts as 2.
  d="$(diff <(printf '%s' "$out") <(printf '%s' "$(cat "$gt")") | grep -cE '^[<>]' || true)"
  if [[ "$d" -eq 0 ]]; then
    exact=$((exact + 1))
    mark="EXACT"
  else
    mark="$d"
  fi
  printf "%-34s %12s\n" "$stem" "$mark"
done
echo
echo "Fully conformant: $exact / $total"
