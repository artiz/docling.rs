#!/usr/bin/env bash
# Regenerate PDF output for the test corpus and diff it against the committed
# snapshot baseline (tests/snapshots/). The pipeline is deterministic, so a
# clean checkout should report every fixture EXACT; a non-zero diff means the
# output drifted. Run scripts/install/pdf_setup.sh first to fetch the libs/models.
set -euo pipefail
cd "$(dirname "$0")/../.."   # docling.rs/

# Pin the snapshot-baseline pixel path: the scalar image-crate resize (the
# committed snapshots were generated with it; the SIMD default differs by
# ±1/255 per pixel, enough to flip borderline table cells).
export DOCLING_RS_SLOW_RESIZE="${DOCLING_RS_SLOW_RESIZE:-1}"
export DOCLING_LAYOUT_ONNX="${DOCLING_LAYOUT_ONNX:-$(pwd)/.models/layout_heron.onnx}"
export DOCLING_OCR_REC_ONNX="${DOCLING_OCR_REC_ONNX:-$(pwd)/.models/ocr_rec.onnx}"
export DOCLING_OCR_DICT="${DOCLING_OCR_DICT:-$(pwd)/.models/ppocr_keys_v1.txt}"
# The text detector (#429) is part of the OCR baseline: without it bitmap
# pages lose every line the layout model gave no region, and the image /
# figure snapshots change.
export DOCLING_OCR_DET_ONNX="${DOCLING_OCR_DET_ONNX:-$(pwd)/.models/ocr_det.onnx}"
# Optional: falls back to geometric table reconstruction if missing. Exported
# explicitly (not just relying on the binary's relative-path default) so the
# regenerated baseline always reflects TableFormer when it's present locally.
export DOCLING_TABLEFORMER_ENCODER="${DOCLING_TABLEFORMER_ENCODER:-$(pwd)/.models/tableformer/encoder.onnx}"
export DOCLING_TABLEFORMER_DECODER="${DOCLING_TABLEFORMER_DECODER:-$(pwd)/.models/tableformer/decoder.onnx}"
export DOCLING_TABLEFORMER_BBOX="${DOCLING_TABLEFORMER_BBOX:-$(pwd)/.models/tableformer/bbox.onnx}"

for f in "$DOCLING_LAYOUT_ONNX" \
         "$DOCLING_OCR_REC_ONNX" "$DOCLING_OCR_DICT" "$DOCLING_OCR_DET_ONNX"; do
  [ -e "$f" ] || { echo "MISSING: $f  (run scripts/install/pdf_setup.sh)"; exit 1; }
done
# The snapshots are rendered by docling-parse's renderer (#478 — the raster
# docling 2.123+ feeds its models); without the shim the pipeline would fall
# back to the Rust renderer and every ML-borderline fixture would read as drift.
export DOCLING_RS_RENDERER="${DOCLING_RS_RENDERER:-docling-parse}"
export DOCLING_PARSE_RENDER_LIB="${DOCLING_PARSE_RENDER_LIB:-$(pwd)/.docling-parse/lib}"
if [ "$DOCLING_RS_RENDERER" = docling-parse ]; then
  for f in "$DOCLING_PARSE_RENDER_LIB"/libdparse_render.*; do
    [ -e "$f" ] || { echo "MISSING: $DOCLING_PARSE_RENDER_LIB/libdparse_render.so  (run scripts/install/build_docling_parse_render.sh, or DOCLING_RS_RENDERER=rust)"; exit 1; }
  done
fi

# PDF_CONFORMANCE_OUT keeps the regenerated tree (CI uploads it as the
# artifact a maintainer refreshes tests/snapshots from); unset, it is scratch.
if [ -n "${PDF_CONFORMANCE_OUT:-}" ]; then
  tmp="$PDF_CONFORMANCE_OUT"
  mkdir -p "$tmp"
else
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
fi
echo "regenerating PDF output ..."
cargo run --release -q -p docling-pdf --example snapshot -- tests/data "$tmp" 1>&2

exact=0; drift=0; tot=0
while IFS= read -r snap; do
  rel="${snap#tests/snapshots/}"
  gen="$tmp/$rel"
  tot=$((tot + 1))
  if [ -f "$gen" ] && diff -q "$snap" "$gen" >/dev/null 2>&1; then
    exact=$((exact + 1))
  else
    drift=$((drift + 1))
    d=$(diff "$snap" "$gen" 2>/dev/null | grep -cE '^[<>]' || true)
    printf "  %-55s %s\n" "$rel" "${d:-MISSING}"
  fi
done < <(find tests/snapshots -name '*.md' | sort)

echo "PDF snapshot conformance: $exact/$tot exact ($drift drifted)"
[ "$drift" -eq 0 ]
