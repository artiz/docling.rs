#!/usr/bin/env bash
#
# Speed + conformance comparison of docling.rs against an Amazon Bedrock model
# (Nova by default) prompted to extract a PDF as Markdown. For every corpus PDF
# with committed groundtruth (tests/data/pdf/groundtruth/*.md) it measures:
#
#   - docling.rs: one warm batch run of the release CLI (the #205 batch mode —
#     models load once, per-file wall time parsed from its `ok:` lines). Set
#     DOCLING_RS_EP=cuda for a GPU run.
#   - Bedrock: a Converse API call per PDF (document block + extraction
#     prompt), timed around the API call.
#
# and scores both outputs against the same groundtruth with a normalized
# line-similarity percentage (whitespace collapsed, empty lines dropped —
# byte-exactness is meaningless for an LLM, so both sides get the same fuzzy
# metric; docling.rs lands near 100 %).
#
# Credentials/config (env):
#   AWS_BEDROCK_REGION             e.g. eu-central-1        (required)
#   AWS_BEDROCK_ACCESS_KEY_ID                               (required)
#   AWS_BEDROCK_SECRET_ACCESS_KEY                           (required)
#   AWS_BEDROCK_MODEL_ID     default eu.amazon.nova-micro-v1:0 — NOTE: Nova
#                            Micro is text-only per AWS docs; if the API
#                            rejects the document block, use the multimodal
#                            eu.amazon.nova-lite-v1:0 instead
#   AWS_BEDROCK_MAX_TOKENS   default 5000
#   AWS_BEDROCK_PROMPT       default "Extract all the possible info from this
#                            PDF as markdown text."
#
# Needs python3 + boto3 (`pip install boto3`).
#
# Usage: scripts/conformance/bedrock_conformance.sh

set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../.."

for v in AWS_BEDROCK_REGION AWS_BEDROCK_ACCESS_KEY_ID AWS_BEDROCK_SECRET_ACCESS_KEY; do
  if [ -z "${!v:-}" ]; then
    echo "error: $v is not set (see the header of this script)" >&2
    exit 2
  fi
done
if ! python3 -c 'import boto3' 2>/dev/null; then
  echo "error: boto3 is not importable — pip install boto3" >&2
  exit 2
fi

# The same model pins as pdf_groundtruth.sh, so the docling.rs numbers here
# match the committed conformance baseline.
export PDFIUM_DYNAMIC_LIB_PATH="${PDFIUM_DYNAMIC_LIB_PATH:-$(pwd)/.pdfium/lib}"
export DOCLING_RS_SLOW_RESIZE="${DOCLING_RS_SLOW_RESIZE:-1}"
export DOCLING_LAYOUT_ONNX="${DOCLING_LAYOUT_ONNX:-$(pwd)/.models/layout_heron.onnx}"
export DOCLING_OCR_REC_ONNX="${DOCLING_OCR_REC_ONNX:-$(pwd)/.models/ocr_rec.onnx}"
export DOCLING_OCR_DICT="${DOCLING_OCR_DICT:-$(pwd)/.models/ppocr_keys_v1.txt}"
export DOCLING_TABLEFORMER_ENCODER="${DOCLING_TABLEFORMER_ENCODER:-$(pwd)/.models/tableformer/encoder.onnx}"
export DOCLING_TABLEFORMER_DECODER="${DOCLING_TABLEFORMER_DECODER:-$(pwd)/.models/tableformer/decoder.onnx}"
export DOCLING_TABLEFORMER_BBOX="${DOCLING_TABLEFORMER_BBOX:-$(pwd)/.models/tableformer/bbox.onnx}"

# GPU run: DOCLING_RS_EP=cuda needs the cuda feature compiled in.
FEATURES=()
case "${DOCLING_RS_EP:-}" in
  cuda) FEATURES=(--features cuda) ;;
  tensorrt) FEATURES=(--features tensorrt) ;;
esac
cargo build --release --quiet -p docling-cli "${FEATURES[@]}"

exec python3 scripts/conformance/bedrock_nova.py
